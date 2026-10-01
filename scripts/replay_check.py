#!/usr/bin/env python3
"""Re-run the router replay pack with the current code and compare it with the
committed fixtures.

Each case goes through the whole chain on fixed inputs: a committed snapshot,
the current `/swap-instructions` and `/swap` builders (a `server` plan test),
the router and short venue built from this tree, and mainnet's program
bytecode in LiteSVM (`oracle`), run with `ORACLE_OFFLINE=1` so no account can
come from the network. The output must match the committed fixture: payouts,
balances, fees, error kinds and every other recorded field. Compute units and
the router's ELF hash are reported, not compared: an SBF build differs across
machines, and a CU change is measured, not a regression by itself. A replay
that runs out of the builder's CU budget fails and shows up as a difference.

Program bytecode is never fetched here: PROGRAMS_DIR must hold every program
`programs.tsv` lists, with the ELF hash it records.

Each run writes into a new run-* directory under --out and never deletes
anything: plans/, results/ (each result a file the oracle must create; the
committed fixture is only read), replay-report.json and summary.md. The report
records the commit, toolchains, LiteSVM, SDK fork commits, program hashes and
input hashes.

Usage: replay_check.py [--programs DIR] [--router SO] [--short-venue SO] [--out DIR]
                       [FIXTURE]...
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "crates/tx/src/tests/fixtures"
SCENARIO_POOLS = FIXTURES / "scenario_pools.json.gz"

UNCOMPARED = {"compute_units", "v1_compute_units", "v1_router_compute_units", "router_sha256"}
ERROR_KIND = re.compile(r"^[A-Za-z]+\(\d+, [A-Za-z]+(?:\(\d+\))?\)")
SHOWN_DIFFERENCES = 20


def case(fixture, test, plans_env, mode, corpus, extra_env=None, short_venue_last=False):
    return {
        "fixture": fixture,
        "test": test,
        "plans_env": plans_env,
        "mode": mode,
        "corpus": corpus,
        "extra_env": extra_env or {},
        "short_venue_last": short_venue_last,
    }


CASES = [
    case("router_scenarios.json", "router_scenario_plans", "ROUTER_SCENARIO_PLANS",
         "router-scenarios", SCENARIO_POOLS),
    case("router_flow_replay.json", "router_flow_plans", "ROUTER_FLOW_PLANS",
         "router", SCENARIO_POOLS, short_venue_last=True),
    case("router_amm_v4_matrix.json", "router_amm_v4_matrix_plans", "ROUTER_AMM_V4_MATRIX_PLANS",
         "router-matrix", ROOT / "oracle/snapshots/amm-v4-routes.json.gz"),
    case("router_amm_v4_token22.json", "router_amm_v4_token22_plans", "ROUTER_AMM_V4_TOKEN22_PLANS",
         "router-matrix", ROOT / "oracle/snapshots/amm-v4-token22.json.gz"),
    case("router_clmm_cross.json", "router_clmm_cross_plans", "ROUTER_CLMM_CROSS_PLANS",
         "router-matrix", FIXTURES / "clmm_cross_dex.json"),
    case("router_orca_cross.json", "router_orca_cross_plans", "ROUTER_ORCA_CROSS_PLANS",
         "router-matrix", FIXTURES / "orca_cross_dex.json",
         extra_env={"ROUTER_ORCA_CROSS_SNAPSHOT": str(FIXTURES / "orca_cross_dex.json")}),
    case("router_dlmm_two_array.json", "router_dlmm_two_array_plans", "ROUTER_DLMM_TWO_ARRAY_PLANS",
         "router-matrix", ROOT / "crates/quoter/src/tests/fixtures/svm/meteora_dlmm.json.gz"),
]


def run(cmd, env=None, capture=False):
    print("+", " ".join(str(c) for c in cmd), file=sys.stderr, flush=True)
    return subprocess.run(
        [str(c) for c in cmd], cwd=ROOT, env=env, check=True,
        stdout=subprocess.PIPE if capture else None, text=True,
    ).stdout


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def rel(path):
    return str(Path(path).resolve().relative_to(ROOT))


def verify_programs(programs):
    rows = []
    for line in (ROOT / "oracle/programs/programs.tsv").read_text().splitlines():
        program, deploy_slot, expected = line.split("\t")
        path = programs / f"{program}.so"
        if not path.is_file():
            sys.exit(f"{path} is missing; programs.tsv lists it")
        actual = sha256(path)
        if actual != expected:
            sys.exit(f"{path}: sha256 {actual}, programs.tsv records {expected}")
        rows.append({"program": program, "deploy_slot": deploy_slot, "sha256": expected})
    return rows


def lock_packages(lock):
    packages, current = [], {}
    for line in lock.read_text().splitlines():
        if line == "[[package]]":
            current = {}
            packages.append(current)
        elif " = " in line and current is not None:
            key, value = line.split(" = ", 1)
            current[key] = value.strip('"')
    return packages


def provenance(cases, programs, router_so, short_venue_so):
    oracle_lock = lock_packages(ROOT / "oracle/Cargo.lock")
    root_lock = lock_packages(ROOT / "Cargo.lock")
    forks = {
        p["name"]: p["source"].rsplit("#", 1)[-1]
        for p in root_lock
        if p.get("source", "").startswith("git+https://github.com/kaannakiin/")
    }
    inputs = sorted({rel(c["corpus"]) for c in cases})
    return {
        "commit": run(["git", "rev-parse", "HEAD"], capture=True).strip(),
        "dirty": bool(run(["git", "status", "--porcelain", "--untracked-files=no"], capture=True).strip()),
        "rustc": run(["rustc", "-V"], capture=True).strip(),
        "cargo_build_sbf": run(["cargo", "build-sbf", "--version"], capture=True).split("\n")[:2],
        "litesvm": next(p["version"] for p in oracle_lock if p["name"] == "litesvm"),
        "sdk_forks": dict(sorted(forks.items())),
        "programs": programs,
        "router_sha256": sha256(router_so),
        "short_venue_sha256": sha256(short_venue_so),
        "inputs": {path: sha256(ROOT / path) for path in inputs},
    }


def normalized(key, value):
    if isinstance(value, str) and key is not None and "error" in key:
        kind = ERROR_KIND.match(value)
        return kind.group(0) if kind else value
    return value


def compare(recorded, now, path, key, differences, units):
    if key in UNCOMPARED:
        if recorded != now:
            units.append({"path": path, "recorded": recorded, "now": now})
        return
    if isinstance(recorded, dict) and isinstance(now, dict):
        for name in sorted(recorded.keys() | now.keys()):
            if name not in now or name not in recorded:
                found = units if name in UNCOMPARED else differences
                found.append({"path": f"{path}.{name}", "recorded": recorded.get(name),
                              "now": now.get(name)})
            else:
                compare(recorded[name], now[name], f"{path}.{name}", name, differences, units)
    elif isinstance(recorded, list) and isinstance(now, list):
        if len(recorded) != len(now):
            differences.append({"path": f"{path}.length", "recorded": len(recorded),
                                "now": len(now)})
        for index, (old, new) in enumerate(zip(recorded, now)):
            compare(old, new, f"{path}[{index}]", key, differences, units)
    elif normalized(key, recorded) != normalized(key, now):
        differences.append({"path": path, "recorded": normalized(key, recorded),
                            "now": normalized(key, now)})


def run_dir(root):
    root.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix="run-", dir=root))


def failed(fixture, reason):
    return {"fixture": fixture, "status": "failed", "difference_count": 1,
            "differences": [{"path": "oracle", "recorded": "a replay result", "now": reason}],
            "compute_units": []}


def replay(c, oracle, plans_file, programs_dir, router, short_venue, results_dir, env):
    committed = FIXTURES / c["fixture"]
    result = results_dir / c["fixture"]
    if result.exists():
        return failed(c["fixture"], f"{result} existed before the replay")
    if c["mode"] == "router-scenarios":
        tail = [short_venue, result]
    elif c["mode"] == "router-matrix":
        tail = [result, committed]
    elif c["short_venue_last"]:
        tail = [result, short_venue]
    else:
        tail = [result]
    try:
        run([oracle, c["mode"], c["corpus"], plans_file, programs_dir, router, *tail], env=env)
    except subprocess.CalledProcessError as error:
        return failed(c["fixture"], f"exit {error.returncode}")
    if not result.is_file():
        return failed(c["fixture"], "the oracle exited 0 without writing a result")
    differences, units = [], []
    compare(json.loads(committed.read_text()), json.loads(result.read_text()),
            "$", None, differences, units)
    return {"fixture": c["fixture"],
            "status": "match" if not differences else "differs",
            "difference_count": len(differences),
            "differences": differences[:SHOWN_DIFFERENCES],
            "compute_units": units}


def summary(report):
    lines = ["# Router replay check", "", f"Commit `{report['provenance']['commit']}`", "",
             "| Fixture | Result | Differences | CU changes |", "| --- | --- | --- | --- |"]
    for result in report["results"]:
        lines.append(f"| `{result['fixture']}` | {result['status']} "
                     f"| {result['difference_count']} | {len(result['compute_units'])} |")
    for result in report["results"]:
        if result["differences"]:
            lines += ["", f"## {result['fixture']}", ""]
            lines += [f"- `{d['path']}`: recorded `{d['recorded']}`, now `{d['now']}`"
                      for d in result["differences"]]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--programs", type=Path, default=ROOT / "oracle/programs")
    parser.add_argument("--router", type=Path, default=ROOT / "onchain/target/deploy/router.so")
    parser.add_argument("--short-venue", type=Path,
                        default=ROOT / "onchain/target/deploy/short_venue.so")
    parser.add_argument("--out", type=Path, default=ROOT / "target/replay-check")
    parser.add_argument("fixtures", nargs="*", help="fixture names to run; all when none")
    args = parser.parse_args()
    known = {c["fixture"].removesuffix(".json") for c in CASES}
    if unknown := set(args.fixtures) - known:
        sys.exit(f"unknown fixtures {sorted(unknown)}; the pack is {sorted(known)}")
    cases = [c for c in CASES if not args.fixtures
             or c["fixture"].removesuffix(".json") in args.fixtures]

    programs_dir = args.programs.resolve()
    programs = verify_programs(programs_dir)
    out = run_dir(args.out.resolve())
    plans = out / "plans"
    results_dir = out / "results"
    plans.mkdir()
    results_dir.mkdir()

    env = dict(os.environ)
    for c in cases:
        env[c["plans_env"]] = str(plans / c["fixture"])
        env.update(c["extra_env"])
    names = "|".join(c["test"] for c in cases)
    run(["cargo", "nextest", "run", "--locked", "-p", "server", "--run-ignored", "only",
         "--no-fail-fast", "-E", f"test(/::({names})$/)"], env=env)
    run(["cargo", "build", "--locked", "--manifest-path", "oracle/Cargo.toml"])

    offline = dict(os.environ, ORACLE_OFFLINE="1")
    results = [
        replay(c, ROOT / "oracle/target/debug/oracle", plans / c["fixture"], programs_dir,
               args.router, args.short_venue, results_dir, offline)
        for c in cases
    ]

    report = {"provenance": provenance(cases, programs, args.router, args.short_venue),
              "results": results}
    (out / "replay-report.json").write_text(json.dumps(report, indent=2) + "\n")
    (out / "summary.md").write_text(summary(report))
    print(summary(report))
    print(f"report: {out}", file=sys.stderr)
    if any(r["status"] != "match" for r in results):
        sys.exit(1)


if __name__ == "__main__":
    main()

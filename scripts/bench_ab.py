#!/usr/bin/env python3
"""Interleaved A/B of one criterion bench: a git ref against the working tree.

A laptop's timings drift between runs (P/E cores, background load, memory
pressure), so one before/after pair says little. Both binaries are built
first, then run in alternating order for several rounds; the report takes
the median and the minimum of each side's per-round medians. Noise only adds
time, so the minimum is the cleaner signal, and the per-round spread of one
side shows how much of a difference is noise.

The ref is checked out in a temporary worktree with its own target directory,
kept under target/bench-ab-base so later runs reuse its dependencies. It
cannot share the main one: cargo hashes workspace members independently of
their path, so one side's build would pass for the other's and both sides
would run the same binary. Criterion writes its output under the temporary
directory for both sides: written into the repo, it wakes editor and indexer
file watchers during one side's runs only. Quality lines the bench prints to stderr are
compared on the fields both sides print.

Usage: bench_ab.py [--base REF] [--bench NAME] [--rounds N] [--sample-size N]
                   [--measurement-time SECS] [--base-patch FILE] [FILTER]
"""

import argparse
import filecmp
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile

CRATE = "route"
BENCH = "search"
UNIVERSE = "oracle/snapshots/universe.json.gz"
UNIT_US = {"ps": 1e-6, "ns": 1e-3, "µs": 1.0, "us": 1.0, "ms": 1e3, "s": 1e6}
TIME = re.compile(r"^(\S+)\s+time:\s+\[\S+ \S+ (\S+) (\S+) \S+ \S+\]", re.M)


def git(*args, cwd):
    return subprocess.run(
        ["git", *args], cwd=cwd, check=True, capture_output=True, text=True
    ).stdout.strip()


def build(src, env, bench):
    out = subprocess.run(
        [
            "cargo", "bench", "-p", CRATE, "--bench", bench, "--no-run",
            "--message-format=json-render-diagnostics",
        ],
        cwd=src, env=env, check=True, stdout=subprocess.PIPE, text=True,
    ).stdout
    for line in out.splitlines():
        msg = json.loads(line)
        target = msg.get("target", {})
        if (
            msg.get("reason") == "compiler-artifact"
            and target.get("name") == bench
            and "bench" in target.get("kind", [])
            and msg.get("executable")
        ):
            return msg["executable"]
    sys.exit(f"no {bench} bench executable built in {src}")


def run(exe, src, env, args):
    cmd = [
        exe, "--bench", "--noplot", "--discard-baseline",
        "--sample-size", str(args.sample_size),
        "--measurement-time", str(args.measurement_time),
    ]
    if args.filter:
        cmd.append(args.filter)
    done = subprocess.run(cmd, cwd=src, env=env, capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"{exe} failed:\n{done.stderr[-2000:]}")
    times = {
        m[1]: float(m[2]) * UNIT_US[m[3]] for m in TIME.finditer(done.stdout)
    }
    quality = {}
    for line in done.stderr.splitlines():
        if line.startswith("quality "):
            name, _, fields = line[len("quality "):].partition(": ")
            quality[name] = dict(f.split(" ", 1) for f in fields.split(", "))
    return times, quality


def compare_quality(base, head):
    differ = []
    for name in sorted(base.keys() & head.keys()):
        shared = base[name].keys() & head[name].keys()
        if any(base[name][k] != head[name][k] for k in shared):
            differ.append(name)
    return differ


def fmt(us):
    return f"{us / 1e3:.2f}ms" if us >= 1e3 else f"{us:.0f}µs"


def report(samples, bench):
    print(
        f"{'bench':48}{'base med':>10}{'head med':>10}{'Δmed':>8}"
        f"{'base min':>10}{'head min':>10}{'Δmin':>8}{'spread b/h':>12}"
    )
    for name in samples["base"][0]:
        side = {
            s: [r[name] for r in samples[s] if name in r] for s in ("base", "head")
        }
        med = {s: statistics.median(v) for s, v in side.items()}
        low = {s: min(v) for s, v in side.items()}
        spread = {s: 100 * (max(v) - min(v)) / min(v) for s, v in side.items()}
        print(
            f"{name.removeprefix(f'{bench}/'):48}{fmt(med['base']):>10}"
            f"{fmt(med['head']):>10}{100 * (med['head'] / med['base'] - 1):+7.1f}%"
            f"{fmt(low['base']):>10}{fmt(low['head']):>10}"
            f"{100 * (low['head'] / low['base'] - 1):+7.1f}%"
            f"{spread['base']:6.0f}%/{spread['head']:.0f}%"
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base", default="HEAD")
    parser.add_argument("--bench", default=BENCH, help=f"a {CRATE} bench target")
    parser.add_argument("--rounds", type=int, default=4)
    parser.add_argument("--sample-size", type=int, default=30)
    parser.add_argument("--measurement-time", type=int, default=3)
    parser.add_argument(
        "--base-patch",
        help="applied to the base worktree, e.g. a bench change the ref predates",
    )
    parser.add_argument("filter", nargs="?")
    args = parser.parse_args()

    root = git("rev-parse", "--show-toplevel", cwd=os.getcwd())
    env = dict(os.environ)
    env.setdefault("ROUTE_UNIVERSE", os.path.join(root, UNIVERSE))
    target = {
        "base": os.path.join(root, "target", "bench-ab-base"),
        "head": os.path.join(root, "target"),
    }
    with tempfile.TemporaryDirectory(prefix="bench-ab-") as tmp:
        base_dir = os.path.join(tmp, "base")
        git("worktree", "add", "--detach", base_dir, args.base, cwd=root)
        try:
            if args.base_patch:
                git("apply", os.path.abspath(args.base_patch), cwd=base_dir)
            src = {"base": base_dir, "head": root}
            exe = {}
            for side in ("base", "head"):
                built = build(src[side], dict(env, CARGO_TARGET_DIR=target[side]), args.bench)
                exe[side] = os.path.join(tmp, f"{args.bench}-{side}")
                shutil.copy2(built, exe[side])
            if filecmp.cmp(exe["base"], exe["head"], shallow=False):
                sys.exit("base and head built the same binary: nothing to compare")
            samples = {"base": [], "head": []}
            quality = {}
            for i in range(args.rounds):
                order = ("base", "head") if i % 2 == 0 else ("head", "base")
                for side in order:
                    print(f"round {i + 1}/{args.rounds}: {side}", file=sys.stderr)
                    home = dict(env, CRITERION_HOME=os.path.join(tmp, f"criterion-{side}"))
                    times, quality[side] = run(exe[side], src[side], home, args)
                    samples[side].append(times)
        finally:
            git("worktree", "remove", "--force", base_dir, cwd=root)
    report(samples, args.bench)
    differ = compare_quality(quality["base"], quality["head"])
    print(
        f"quality: {len(quality['head'])} lines, "
        + (f"DIFFER: {', '.join(differ)}" if differ else "identical on shared fields")
    )


if __name__ == "__main__":
    main()

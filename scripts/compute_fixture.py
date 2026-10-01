#!/usr/bin/env python3
"""Join `just router-compute-replay` runs into the router compute fixture.

Usage: compute_fixture.py OUT PLANS REPLAY [PLANS REPLAY ...]

Each PLANS file is `target/router-compute-plans.json` and each REPLAY the
`target/router-compute-replay.json` that `oracle router` wrote from it. Keeps
every one-hop swap the router paid in its v1 transaction, with the walk its
quote reported and what the router's instruction spent, so `tx` can test its
compute budget against what the deployed programs spent.
"""

import json
import pathlib
import sys

STEPPED = {"raydium_clmm", "orca_whirlpool", "meteora_dlmm"}


def main(argv):
    if len(argv) < 4 or len(argv) % 2 != 0:
        sys.exit(__doc__)
    out = pathlib.Path(argv[1])
    runs = []
    cases = []
    for plans_path, replay_path in zip(argv[2::2], argv[3::2]):
        plans = json.loads(pathlib.Path(plans_path).read_text())["plans"]
        replay = json.loads(pathlib.Path(replay_path).read_text())
        if len(plans) != len(replay["cases"]):
            sys.exit(f"{plans_path} and {replay_path} differ in length")
        runs.append(replay["provenance"])
        for plan, case in zip(plans, replay["cases"]):
            if plan["dex"] not in STEPPED:
                continue
            spent = case.get("v1_router_compute_units")
            if case.get("v1_paid") is None or spent is None:
                continue
            cases.append(
                {
                    "dex": plan["dex"],
                    "pool": plan["pool"],
                    "amount_in": plan["amountIn"],
                    "crossed": plan["crossed"],
                    "span": plan["span"],
                    "arrays": plan["arraysUsed"],
                    "router_compute_units": spent,
                }
            )
    out.write_text(json.dumps({"runs": runs, "cases": cases}, indent=1) + "\n")
    print(f"{len(cases)} cases from {len(runs)} runs")


if __name__ == "__main__":
    main(sys.argv)

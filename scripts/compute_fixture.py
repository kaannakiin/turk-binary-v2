#!/usr/bin/env python3
"""Join `just router-compute-replay` runs into the router compute fixture.

Usage: compute_fixture.py OUT PLANS REPLAY [PLANS REPLAY ...]

Each PLANS file is `target/router-compute-plans.json` and each REPLAY the
`target/router-compute-replay.json` that `oracle router` wrote from it. Keeps
every one-hop swap the router paid, with the walk its quote reported and what
the router's instruction spent under the largest compute limit, so `tx` can
test its compute budget against what the deployed programs spent. The v1
transaction carries the budget under test and fails a swap that outspends it;
the instruction replay does not, so it measures the swaps a refit needs.
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
        written = json.loads(pathlib.Path(plans_path).read_text())
        plans = written["plans"]
        replay = json.loads(pathlib.Path(replay_path).read_text())
        if len(plans) != len(replay["cases"]):
            sys.exit(f"{plans_path} and {replay_path} differ in length")
        run = len(runs)
        runs.append({"corpus": written["corpus"], **replay["provenance"]})
        for plan, case in zip(plans, replay["cases"]):
            if plan["dex"] not in STEPPED:
                continue
            spent = case.get("router_compute_units")
            if case.get("paid") is None or spent is None:
                continue
            cases.append(
                {
                    "run": run,
                    "dex": plan["dex"],
                    "pool": plan["pool"],
                    "input_mint": plan["inputMint"],
                    "amount_in": plan["amountIn"],
                    "crossed": plan["crossed"],
                    "span": plan["span"],
                    "arrays": plan["arraysUsed"],
                    "tail": plan["tail"],
                    "token_2022": plan["token2022"],
                    "transfer_fee": plan["transferFee"],
                    "steps_changed": case.get("steps_changed"),
                    "router_compute_units": spent,
                }
            )
    out.write_text(json.dumps({"runs": runs, "cases": cases}, indent=1) + "\n")
    print(f"{len(cases)} cases from {len(runs)} runs")


if __name__ == "__main__":
    main(sys.argv)

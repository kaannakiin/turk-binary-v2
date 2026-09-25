#!/usr/bin/env python3
"""Import a simulateTransaction corpus from the previous repo as a quote fixture.

Usage: import_sim_fixture.py SOURCE_JSON OUT_JSON_GZ

The source is one of turk-binary's `crates/dex-adapters/tests/fixtures/*-onchain-sim.json`
files: account bytes dumped at a slot, and for each case the amount `simulateTransaction`
paid (`onchainAmountOut`). That payout is the expected value. The previous port's own
output (`quoteAmountOut`, `relErr`) is dropped, so it can never become one.

The source file's sha256 is recorded next to the data.
"""

import gzip
import hashlib
import json
import pathlib
import sys

DROPPED = ("quoteAmountOut", "relErr")


def normalize(fixture):
    """The DLMM corpora keep a state's accounts at its top level and name
    fields differently; bring them to the shape every other corpus has."""
    for state_id, state in fixture["states"].items():
        if "raw" not in state:
            fixture["states"][state_id] = {"raw": state}
    for case in fixture["cases"]:
        if "stateId" in case:
            case["state_id"] = case.pop("stateId")
        case["slot"] = int(case["slot"])


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    source = pathlib.Path(sys.argv[1])
    out = pathlib.Path(sys.argv[2])
    raw = source.read_bytes()
    fixture = json.loads(raw)
    normalize(fixture)
    for case in fixture["cases"]:
        for key in DROPPED:
            case.pop(key, None)
    fixture["provenance"] = {
        "file": source.name,
        "sha256": hashlib.sha256(raw).hexdigest(),
        "note": fixture.pop("note", ""),
        "source": fixture.pop("source", ""),
    }
    out.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(fixture, separators=(",", ":"), sort_keys=True) + "\n"
    out.write_bytes(gzip.compress(text.encode(), compresslevel=9, mtime=0))
    print(f"{out}: {len(fixture['cases'])} cases over {len(fixture['states'])} states")


if __name__ == "__main__":
    main()

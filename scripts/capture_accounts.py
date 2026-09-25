#!/usr/bin/env python3
"""Capture mainnet accounts as test fixtures.

Usage: capture_accounts.py OUT_DIR PUBKEY [PUBKEY ...]

Writes OUT_DIR/<pubkey>.bin for every account that exists and records every
requested key in OUT_DIR/accounts.tsv (pubkey, owner or "-", data length, slot).
Uses the public mainnet endpoint only, never the project's RPC.
"""

import base64
import json
import pathlib
import sys
import urllib.request

ENDPOINT = "https://api.mainnet-beta.solana.com"
CHUNK = 100


def fetch(keys):
    body = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getMultipleAccounts",
            "params": [keys, {"encoding": "base64", "commitment": "confirmed"}],
        }
    ).encode()
    request = urllib.request.Request(
        ENDPOINT, data=body, headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        reply = json.load(response)
    if "error" in reply:
        sys.exit(f"rpc error: {reply['error']}")
    return reply["result"]["context"]["slot"], reply["result"]["value"]


def load_manifest(path):
    rows = {}
    if path.exists():
        for line in path.read_text().splitlines():
            key, *rest = line.split("\t")
            rows[key] = rest
    return rows


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    out = pathlib.Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    keys = list(dict.fromkeys(sys.argv[2:]))
    manifest_path = out / "accounts.tsv"
    manifest = load_manifest(manifest_path)
    for start in range(0, len(keys), CHUNK):
        chunk = keys[start : start + CHUNK]
        slot, values = fetch(chunk)
        if len(values) != len(chunk):
            sys.exit(f"rpc returned {len(values)} accounts for {len(chunk)} keys")
        for key, value in zip(chunk, values):
            if value is None:
                manifest[key] = ["-", "0", str(slot)]
                (out / f"{key}.bin").unlink(missing_ok=True)
                continue
            data = base64.b64decode(value["data"][0])
            (out / f"{key}.bin").write_bytes(data)
            manifest[key] = [value["owner"], str(len(data)), str(slot)]
    manifest_path.write_text(
        "".join("\t".join([key, *rest]) + "\n" for key, rest in sorted(manifest.items()))
    )


if __name__ == "__main__":
    main()

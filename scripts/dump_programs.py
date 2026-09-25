#!/usr/bin/env python3
"""Dump the deployed bytecode of every program the LiteSVM oracle runs.

Usage: dump_programs.py OUT_DIR

Writes OUT_DIR/<program id>.so and OUT_DIR/programs.tsv (program id, the slot
its current bytecode was deployed at, sha256 of the ELF). Uses the public
mainnet endpoint only, never the project's RPC.
"""

import base64
import hashlib
import json
import pathlib
import sys
import urllib.request

ENDPOINT = "https://api.mainnet-beta.solana.com"
UPGRADEABLE_LOADER = "BPFLoaderUpgradeab1e11111111111111111111111"
PROGRAMDATA_ELF = 45

PROGRAMS = [
    "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
    "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",
    "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK",
    "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",
    "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
    "cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG",
    "Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB",
    "24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi",
    "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA",
    "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ",
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
    "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
    "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
]

B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58encode(raw):
    n = int.from_bytes(raw, "big")
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = B58[r] + out
    return "1" * (len(raw) - len(raw.lstrip(b"\0"))) + out


def account(key):
    body = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getAccountInfo",
            "params": [key, {"encoding": "base64", "commitment": "confirmed"}],
        }
    ).encode()
    request = urllib.request.Request(
        ENDPOINT, data=body, headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        reply = json.load(response)
    if "error" in reply:
        sys.exit(f"rpc error for {key}: {reply['error']}")
    value = reply["result"]["value"]
    if value is None:
        sys.exit(f"{key} does not exist")
    return value["owner"], base64.b64decode(value["data"][0])


def elf_of(program):
    owner, data = account(program)
    if owner != UPGRADEABLE_LOADER:
        return data, "-"
    programdata = b58encode(data[4:36])
    _, data = account(programdata)
    slot = int.from_bytes(data[4:12], "little")
    return data[PROGRAMDATA_ELF:], str(slot)


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    out = pathlib.Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    rows = []
    for program in PROGRAMS:
        elf, slot = elf_of(program)
        (out / f"{program}.so").write_bytes(elf)
        rows.append(f"{program}\t{slot}\t{hashlib.sha256(elf).hexdigest()}")
        print(f"{program}: {len(elf)} bytes, deployed at slot {slot}")
    (out / "programs.tsv").write_text("\n".join(rows) + "\n")


if __name__ == "__main__":
    main()

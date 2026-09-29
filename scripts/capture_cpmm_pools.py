#!/usr/bin/env python3
"""Capture Raydium CPMM pools as a replay corpus, every account at one slot.

Usage: capture_cpmm_pools.py OUT.json.gz POOL [POOL ...]

Writes the corpus shape `just oracle` writes (clock, pools, extra) without
cases: each pool with the accounts its view reads, the accounts only its swap
passes in `extra`, and the Clock read in the same getMultipleAccounts call.
Uses the public mainnet endpoint only, never the project's RPC.
"""

import base64
import gzip
import json
import struct
import sys
import urllib.request

ENDPOINT = "https://api.mainnet-beta.solana.com"
CPMM = "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C"
CLOCK = "SysvarC1ock11111111111111111111111111111111"
# src: crates/quoter/src/raydium_cpmm/mod.rs (AUTHORITY)
AUTHORITY = "GpMZbSM2GgvTKHJirzeGfMFoaZ8UR2X7F4v8vHTvxFbL"
POOL_LEN = 637
# src: raydium-io/raydium-cp-swap@59fb845a9e5bb569c8b2f3415f13b0c0ebcc6b92 programs/cp-swap/src/states/pool.rs
# (PoolState: amm_config, pool_creator, token_0_vault, token_1_vault, lp_mint, token_0_mint,
# token_1_mint, token_0_program, token_1_program, observation_key after the 8-byte discriminator);
# every pool of crates/quoter/src/tests/fixtures/svm/raydium_cpmm.json.gz decodes to its captured view
# accounts and to an observation in its `extra`.
AMM_CONFIG, VAULT_0, VAULT_1, MINT_0, MINT_1, OBSERVATION = 8, 72, 104, 168, 200, 296
ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58(raw):
    n = int.from_bytes(raw, "big")
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = ALPHABET[r] + out
    return "1" * (len(raw) - len(raw.lstrip(b"\0"))) + out


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


def account(key, value):
    if value is None:
        return {"key": key, "lamports": 0}
    return {
        "key": key,
        "owner": value["owner"],
        "lamports": value["lamports"],
        "data": value["data"][0],
    }


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    out, pools = sys.argv[1], list(dict.fromkeys(sys.argv[2:]))
    _, values = fetch(pools)
    layout = {}
    for pool, value in zip(pools, values):
        if value is None or value["owner"] != CPMM:
            sys.exit(f"{pool} is not a CPMM account")
        data = base64.b64decode(value["data"][0])
        if len(data) != POOL_LEN:
            sys.exit(f"{pool}: {len(data)} bytes, a PoolState is {POOL_LEN}")
        key = lambda offset: b58(data[offset : offset + 32])
        layout[pool] = (
            [pool] + [key(o) for o in (AMM_CONFIG, VAULT_0, VAULT_1, MINT_0, MINT_1)],
            [key(OBSERVATION)],
        )

    keys = list(
        dict.fromkeys(
            [k for view, _ in layout.values() for k in view]
            + [k for _, extra in layout.values() for k in extra]
            + [AUTHORITY, CLOCK]
        )
    )
    if len(keys) > 100:
        sys.exit("one getMultipleAccounts call holds 100 accounts; capture fewer pools")
    slot, values = fetch(keys)
    by_key = dict(zip(keys, values))
    clock = base64.b64decode(by_key[CLOCK]["data"][0])
    slot_, epoch_start, epoch, leader_epoch, unix = struct.unpack("<QqQQq", clock[:40])
    corpus = {
        "provenance": {"endpoint": ENDPOINT, "context_slot": slot, "pools": pools},
        "clock": {
            "slot": slot_,
            "epoch_start_timestamp": epoch_start,
            "epoch": epoch,
            "leader_schedule_epoch": leader_epoch,
            "unix_timestamp": unix,
        },
        "pools": [
            {
                "pool": pool,
                "dex": "raydium_cpmm",
                "cross_stream": False,
                "accounts": [account(k, by_key[k]) for k in view],
            }
            for pool, (view, _) in layout.items()
        ],
        "extra": [
            account(k, by_key[k])
            for k in dict.fromkeys(
                [k for _, extra in layout.values() for k in extra] + [AUTHORITY]
            )
        ],
    }
    with gzip.open(out, "wt") as file:
        json.dump(corpus, file)
    print(f"{len(pools)} pools, {len(keys)} accounts at slot {slot} -> {out}")


if __name__ == "__main__":
    main()

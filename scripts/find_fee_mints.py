#!/usr/bin/env python3
"""List Token-2022 mints whose older and newer transfer fees differ.

The public endpoint refuses getProgramAccounts on Token-2022, so this uses
the project's RPC (TB_RPC_URL, loaded by `just find-fee-mints`) and its
paginated getProgramAccountsV2 (Helius). The URL
never reaches the output: failures print only the HTTP status or the error
type. Only mints whose first extension is TransferFeeConfig are matched.
Capture a listed mint as a fixture with scripts/capture_accounts.py, which
uses the public endpoint.
"""

import base64
import json
import os
import re
import struct
import sys
import time
import urllib.error
import urllib.request

# src: spl-token-2022-interface@3.1.2 src/lib.rs
TOKEN_2022 = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb"
# src: spl-token-2022-interface@3.1.2 src/extension/mod.rs (BASE_ACCOUNT_LENGTH,
# AccountType::Mint = 1, ExtensionType::TransferFeeConfig = 1)
ACCOUNT_TYPE_OFFSET = 165
# src: spl-token-2022-interface@3.1.2 src/extension/transfer_fee/mod.rs (TransferFeeConfig:
# two 32-byte authorities, withheld u64, older and newer TransferFee of u64 epoch,
# u64 maximum_fee, u16 basis points)
CONFIG_LENGTH = 108
OLDER = slice(72, 90)
NEWER = slice(90, 108)


def base58(data):
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    n = int.from_bytes(data, "big")
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = alphabet[r] + out
    return "1" * (len(data) - len(data.lstrip(b"\0"))) + out


PAGE = 10_000


def pages(url):
    cursor = None
    while True:
        page = fetch_page(url, cursor)
        yield page["accounts"]
        cursor = page.get("paginationKey")
        # A filtered page can come back empty before the end; only a null key ends it.
        if not cursor:
            return
        time.sleep(0.5)


def fetch_page(url, cursor):
    options = {
        "encoding": "base64",
        "dataSlice": {"offset": ACCOUNT_TYPE_OFFSET + 1, "length": 4 + CONFIG_LENGTH},
        "filters": [{"memcmp": {"offset": ACCOUNT_TYPE_OFFSET, "bytes": base58(bytes([1, 1, 0]))}}],
        "limit": PAGE,
    }
    if cursor:
        options["paginationKey"] = cursor
    request = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getProgramAccountsV2",
        "params": [TOKEN_2022, options],
    }
    http = urllib.request.Request(
        url, data=json.dumps(request).encode(), headers={"Content-Type": "application/json"}
    )
    for attempt in range(6):
        try:
            with urllib.request.urlopen(http, timeout=300) as response:
                body = json.load(response)
            break
        except urllib.error.HTTPError as e:
            if e.code != 429 or attempt == 5:
                sys.exit(f"getProgramAccountsV2 failed: HTTP {e.code}")
            time.sleep(2**attempt)
        except Exception as e:  # the message may carry the URL
            sys.exit(f"getProgramAccountsV2 failed: {type(e).__name__}")
    if "error" in body:
        error = body["error"]
        message = re.sub(r"\S*://\S*", "<url>", str(error.get("message", "")))
        sys.exit(f"getProgramAccountsV2 failed: RPC error {error.get('code')}: {message[:200]}")
    return body["result"]


def schedule(data):
    kind, length = struct.unpack_from("<HH", data, 0)
    if kind != 1 or length < CONFIG_LENGTH:
        return None
    config = data[4 : 4 + CONFIG_LENGTH]
    return struct.unpack("<QQH", config[OLDER]), struct.unpack("<QQH", config[NEWER])


def main():
    url = os.environ.get("TB_RPC_URL")
    if not url:
        sys.exit("TB_RPC_URL is not set: run `just find-fee-mints`")
    want = int(sys.argv[1]) if len(sys.argv) > 1 else 5
    scanned, found = 0, 0
    for accounts in pages(url):
        scanned += len(accounts)
        for account in accounts:
            fees = schedule(base64.b64decode(account["account"]["data"][0]))
            if not fees or fees[0][1:] == fees[1][1:]:
                continue
            (older, newer), found = fees, found + 1
            print(f"{account['pubkey']} older epoch {older[0]} max {older[1]} bps {older[2]} "
                  f"-> newer epoch {newer[0]} max {newer[1]} bps {newer[2]}", flush=True)
            if found == want:
                print(f"stopped after {found} of {scanned} mints", file=sys.stderr)
                return
        print(f"{scanned} mints scanned, {found} with a fee change", file=sys.stderr, flush=True)


if __name__ == "__main__":
    main()

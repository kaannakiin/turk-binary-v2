#!/usr/bin/env python3
"""Send the server's v1 transactions through a local Surfnet's JSON-RPC.

Usage: surfpool_replay.py MANIFEST OUT

MANIFEST is what `oracle surfpool` wrote: per plan, a Surfpool snapshot of the
accounts LiteSVM prepared for it, the payer-signed v1 transaction, the account
its output lands in and what the quote promised. Each plan gets its own offline
Surfnet started from its snapshot, with the capture's Clock written into the
sysvar, and LiteSVM's Rent, while the clock is paused; the transaction goes through
`sendTransaction` with preflight, and the plan passes when it landed as a v1
transaction without error and the output account gained exactly what was
quoted. Nothing leaves the machine: Surfpool runs with
`--offline`, so no account is fetched and nothing reaches a cluster.
"""

import base64
import json
import pathlib
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request

PORT = 18899
CLOCK = "SysvarC1ock11111111111111111111111111111111"
RENT = "SysvarRent111111111111111111111111111111111"
SYSVAR = "Sysvar1111111111111111111111111111111111111"
TOKEN_AMOUNT = slice(64, 72)


def rpc(method, params=None):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or []})
    request = urllib.request.Request(
        f"http://127.0.0.1:{PORT}",
        data=body.encode(),
        headers={"content-type": "application/json"},
    )
    reply = json.loads(urllib.request.urlopen(request, timeout=60).read())
    if "error" in reply:
        raise RuntimeError(f"{method}: {reply['error']}")
    return reply["result"]


def start(snapshot, log):
    surfnet = subprocess.Popen(
        [
            "surfpool", "start", "--ci", "--offline", "--no-deploy",
            "--skip-blockhash-check", "--log-bytes-limit", "0", "--airdrop-amount", "0",
            "-p", str(PORT), "-w", str(PORT + 1), "--snapshot", str(snapshot),
        ],
        stdout=log,
        stderr=subprocess.STDOUT,
        env={"NO_DNA": "1", "PATH": "/usr/bin:/bin:" + str(pathlib.Path.home() / ".cargo/bin")},
    )
    for _ in range(120):
        try:
            rpc("getVersion")
            return surfnet
        except (urllib.error.URLError, ConnectionError, RuntimeError):
            if surfnet.poll() is not None:
                raise RuntimeError(f"surfpool exited with {surfnet.returncode}")
            time.sleep(0.5)
    surfnet.kill()
    raise RuntimeError("surfpool did not answer")


def set_sysvars(clock, snapshot):
    rpc("surfnet_pauseClock")
    rent = json.loads(pathlib.Path(snapshot).read_text())[RENT]
    rpc("surfnet_setAccount", [RENT, {
        "lamports": rent["lamports"],
        "data": base64.b64decode(rent["data"]).hex(),
        "owner": SYSVAR,
    }])
    data = struct.pack(
        "<QqQQq",
        clock["slot"],
        clock["epoch_start_timestamp"],
        clock["epoch"],
        clock["leader_schedule_epoch"],
        clock["unix_timestamp"],
    )
    rpc("surfnet_setAccount", [CLOCK, {"lamports": 1, "data": data.hex(), "owner": SYSVAR}])


def balance(account):
    info = rpc("getAccountInfo", [account, {"encoding": "base64"}])["value"]
    if info is None:
        return None
    data = base64.b64decode(info["data"][0])
    return int.from_bytes(data[TOKEN_AMOUNT], "little")


def paid_exactly(result):
    return (
        result.get("landed") is True
        and result.get("version") == 1
        and result.get("error") is None
        and result.get("paid") == result.get("expected_out")
    )


def all_paid(results):
    return bool(results) and all(paid_exactly(result) for result in results)


def held_before(plan):
    snapshot = json.loads(pathlib.Path(plan["snapshot"]).read_text())
    account = snapshot.get(plan["destination"])
    if account is None:
        return 0
    return int.from_bytes(base64.b64decode(account["data"])[TOKEN_AMOUNT], "little")


def replay(plan, clock, logs):
    before = held_before(plan)
    with open(logs / f"{plan['name']}.log", "w") as log:
        surfnet = start(plan["snapshot"], log)
        try:
            set_sysvars(clock, plan["snapshot"])
            version = rpc("getVersion")["surfnet-version"]
            try:
                signature = rpc(
                    "sendTransaction",
                    [plan["transaction"], {"encoding": "base64", "preflightCommitment": "processed"}],
                )
            except RuntimeError as error:
                return {"name": plan["name"], "surfnet": version, "error": str(error)[:400]}
            for _ in range(60):
                landed = rpc(
                    "getTransaction",
                    [signature, {"encoding": "json", "maxSupportedTransactionVersion": 1}],
                )
                if landed:
                    break
                time.sleep(0.25)
            meta = landed["meta"] if landed else {}
            after = balance(plan["destination"])
            paid = None if after is None else after - before
            result = {
                "name": plan["name"],
                "surfnet": version,
                "landed": landed is not None,
                "version": landed.get("version") if landed else None,
                "error": meta.get("err"),
                "compute_units": meta.get("computeUnitsConsumed"),
                "expected_out": plan["expected_out"],
                "paid": None if paid is None else str(paid),
            }
            result["exact"] = paid_exactly(result)
            return result
        finally:
            surfnet.terminate()
            try:
                surfnet.wait(timeout=10)
            except subprocess.TimeoutExpired:
                surfnet.kill()
                surfnet.wait()


def main(argv):
    if len(argv) != 3:
        sys.exit(__doc__)
    manifest = json.loads(pathlib.Path(argv[1]).read_text())
    out = pathlib.Path(argv[2])
    logs = out.parent / "surfpool-logs"
    logs.mkdir(parents=True, exist_ok=True)
    results = [replay(plan, manifest["clock"], logs) for plan in manifest["plans"]]
    out.write_text(json.dumps({"plans": results}, indent=1) + "\n")
    exact = sum(paid_exactly(result) for result in results)
    for result in results:
        print(json.dumps(result))
    print(f"{exact} of {len(results)} paid exactly what they quoted")
    sys.exit(0 if all_paid(results) else 1)


if __name__ == "__main__":
    main(sys.argv)

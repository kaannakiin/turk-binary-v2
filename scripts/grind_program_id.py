#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import tempfile


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = REPO_ROOT / "target" / "deploy" / "turk_binary-keypair.json"
BASE58_ALPHABET = frozenset(
    "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
)


def base58_prefix(value: str) -> str:
    invalid = sorted(set(value) - BASE58_ALPHABET)
    if not value or invalid:
        rendered = ", ".join(invalid) if invalid else "empty prefix"
        raise argparse.ArgumentTypeError(f"invalid Base58 character: {rendered}")
    return value


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return parsed


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate a vanity Solana program keypair.",
    )
    parser.add_argument("--prefix", type=base58_prefix, default="TURK")
    parser.add_argument("--threads", type=positive_int, default=10)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    output = args.output.expanduser().resolve()
    if output.exists() or output.is_symlink():
        raise SystemExit(f"error: output already exists: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    os.umask(0o077)

    with tempfile.TemporaryDirectory(
        prefix=".vanity-",
        dir=output.parent,
    ) as work_dir:
        subprocess.run(
            [
                "solana-keygen",
                "grind",
                "--starts-with",
                f"{args.prefix}:1",
                "--num-threads",
                str(args.threads),
            ],
            cwd=work_dir,
            check=True,
        )
        matches = list(Path(work_dir).glob("*.json"))
        if len(matches) != 1:
            raise RuntimeError(f"expected one generated keypair, found {len(matches)}")

        generated = matches[0]
        program_id = subprocess.run(
            ["solana-keygen", "pubkey", str(generated)],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        if not program_id.startswith(args.prefix):
            raise RuntimeError("generated public key does not match requested prefix")

        generated.chmod(0o600)
        os.link(generated, output)
        generated.unlink()

    print(f"Program ID: {program_id}")
    print(f"Keypair: {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

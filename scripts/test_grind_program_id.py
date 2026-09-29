from __future__ import annotations

import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import textwrap
import unittest


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "grind_program_id.py"
MATCHING_PROGRAM_ID = "TURK3LQYwMWyM4c9V9xJPAcYnzz5Q4uD1sQVyJgbAbc"


class GrindProgramIdTests(unittest.TestCase):
    def test_writes_matching_keypair_to_requested_path_with_private_permissions(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            fake_bin = root / "bin"
            fake_bin.mkdir()
            invocation_log = root / "invocations.txt"
            self._write_fake_keygen(fake_bin / "solana-keygen")
            output = root / "target" / "deploy" / "turk_binary-keypair.json"
            env = os.environ.copy()
            env["PATH"] = f"{fake_bin}{os.pathsep}{env['PATH']}"
            env["FAKE_KEYGEN_LOG"] = str(invocation_log)

            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--prefix",
                    "TURK",
                    "--threads",
                    "2",
                    "--output",
                    str(output),
                ],
                cwd=REPO_ROOT,
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(output.read_text(encoding="utf-8"), "[1, 2, 3]\n")
            self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o600)
            self.assertIn(f"Program ID: {MATCHING_PROGRAM_ID}", result.stdout)
            self.assertIn(f"Keypair: {output.resolve()}", result.stdout)
            invocations = invocation_log.read_text(encoding="utf-8").splitlines()
            self.assertEqual(
                invocations[0],
                "grind --starts-with TURK:1 --num-threads 2",
            )
            self.assertTrue(invocations[1].startswith("pubkey "))
            self.assertTrue(
                invocations[1].endswith(f"/{MATCHING_PROGRAM_ID}.json")
            )

    def test_rejects_non_base58_prefix_before_starting_grind(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            fake_bin = root / "bin"
            fake_bin.mkdir()
            invocation_log = root / "invocations.txt"
            self._write_fake_keygen(fake_bin / "solana-keygen")
            env = os.environ.copy()
            env["PATH"] = f"{fake_bin}{os.pathsep}{env['PATH']}"
            env["FAKE_KEYGEN_LOG"] = str(invocation_log)

            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--prefix",
                    "TURK0",
                    "--output",
                    str(root / "program-keypair.json"),
                ],
                cwd=REPO_ROOT,
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("invalid Base58 character", result.stderr)
            self.assertFalse(invocation_log.exists())

    def test_refuses_to_overwrite_existing_keypair_before_starting_grind(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            fake_bin = root / "bin"
            fake_bin.mkdir()
            invocation_log = root / "invocations.txt"
            self._write_fake_keygen(fake_bin / "solana-keygen")
            output = root / "turk_binary-keypair.json"
            output.write_text("existing secret\n", encoding="utf-8")
            env = os.environ.copy()
            env["PATH"] = f"{fake_bin}{os.pathsep}{env['PATH']}"
            env["FAKE_KEYGEN_LOG"] = str(invocation_log)

            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--output",
                    str(output),
                ],
                cwd=REPO_ROOT,
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("already exists", result.stderr)
            self.assertEqual(output.read_text(encoding="utf-8"), "existing secret\n")
            self.assertFalse(invocation_log.exists())

    def test_rejects_non_positive_thread_count_before_starting_grind(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            fake_bin = root / "bin"
            fake_bin.mkdir()
            invocation_log = root / "invocations.txt"
            self._write_fake_keygen(fake_bin / "solana-keygen")
            env = os.environ.copy()
            env["PATH"] = f"{fake_bin}{os.pathsep}{env['PATH']}"
            env["FAKE_KEYGEN_LOG"] = str(invocation_log)

            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--threads",
                    "0",
                    "--output",
                    str(root / "program-keypair.json"),
                ],
                cwd=REPO_ROOT,
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("must be at least 1", result.stderr)
            self.assertFalse(invocation_log.exists())

    @staticmethod
    def _write_fake_keygen(path: Path) -> None:
        path.write_text(
            textwrap.dedent(
                f"""\
                #!{sys.executable}
                import os
                from pathlib import Path
                import sys

                log = Path(os.environ["FAKE_KEYGEN_LOG"])
                with log.open("a", encoding="utf-8") as handle:
                    handle.write(" ".join(sys.argv[1:]) + "\\n")

                if sys.argv[1] == "grind":
                    work_dir = Path.cwd()
                    (work_dir / "{MATCHING_PROGRAM_ID}.json").write_text(
                        "[1, 2, 3]\\n",
                        encoding="utf-8",
                    )
                elif sys.argv[1] == "pubkey":
                    print("{MATCHING_PROGRAM_ID}")
                else:
                    raise SystemExit(2)
                """
            ),
            encoding="utf-8",
        )
        path.chmod(0o755)


if __name__ == "__main__":
    unittest.main()

from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import textwrap
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import replay_check  # noqa: E402

SCENARIOS = json.loads((replay_check.FIXTURES / "router_scenarios.json").read_text())
MATRIX_CASE = next(c for c in replay_check.CASES if c["fixture"] == "router_amm_v4_matrix.json")


def differences(now):
    found, units = [], []
    replay_check.compare(SCENARIOS, now, "$", None, found, units)
    return found, units


def refused(fixture):
    return next(s for s in fixture["swaps"] if s.get("error"))


def paid(fixture):
    return next(s for s in fixture["swaps"] if not s.get("error"))


class CompareTests(unittest.TestCase):
    def test_a_payout_one_unit_off_is_a_difference(self) -> None:
        now = copy.deepcopy(SCENARIOS)
        paid(now)["venue_out"][-1] += 1
        found, _ = differences(now)
        self.assertEqual([d["path"] for d in found], ["$.swaps[0].venue_out[1]"])

    def test_venue_accounts_changed_by_a_refused_route_is_a_difference(self) -> None:
        now = copy.deepcopy(SCENARIOS)
        refused(now)["venue_accounts_unchanged"] = not refused(now)["venue_accounts_unchanged"]
        found, _ = differences(now)
        self.assertEqual(len(found), 1)
        self.assertTrue(found[0]["path"].endswith(".venue_accounts_unchanged"))

    def test_another_error_code_is_a_difference_reported_by_kind(self) -> None:
        now = copy.deepcopy(SCENARIOS)
        refused(now)["error"] = refused(now)["error"].replace("Custom(", "Custom(9", 1)
        found, _ = differences(now)
        self.assertEqual(len(found), 1)
        self.assertRegex(found[0]["now"], r"^InstructionError\(\d+, Custom\(9\d+\)\)$")

    def test_a_dropped_case_is_a_difference(self) -> None:
        now = copy.deepcopy(SCENARIOS)
        now["swaps"].pop()
        found, _ = differences(now)
        self.assertIn("$.swaps.length", [d["path"] for d in found])

    def test_log_text_alone_is_not_a_difference(self) -> None:
        now = copy.deepcopy(SCENARIOS)
        refused(now)["error"] += " | Program log: another line"
        self.assertEqual(differences(now), ([], []))

    def test_compute_units_are_reported_not_compared(self) -> None:
        now = copy.deepcopy(SCENARIOS)
        paid(now)["compute_units"] += 7
        found, units = differences(now)
        self.assertEqual(found, [])
        self.assertEqual(len(units), 1)


class ReplayTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.results = self.root / "results"
        self.results.mkdir()
        self.committed = replay_check.FIXTURES / MATRIX_CASE["fixture"]
        self.committed_bytes = self.committed.read_bytes()

    def tearDown(self) -> None:
        self.temp.cleanup()
        self.assertEqual(self.committed.read_bytes(), self.committed_bytes)

    def replay_with(self, body: str) -> dict:
        oracle = self.root / "oracle"
        oracle.write_text(f"#!{sys.executable}\nimport shutil, sys\n{textwrap.dedent(body)}")
        oracle.chmod(oracle.stat().st_mode | stat.S_IEXEC)
        return replay_check.replay(MATRIX_CASE, oracle, self.root / "plans.json",
                                   self.root, self.root / "router.so",
                                   self.root / "short_venue.so", self.results, dict(os.environ))

    def test_an_oracle_that_exits_zero_without_a_result_fails(self) -> None:
        result = self.replay_with("sys.exit(0)")
        self.assertEqual(result["status"], "failed")

    def test_an_oracle_that_fails_fails(self) -> None:
        self.assertEqual(self.replay_with("sys.exit(3)")["status"], "failed")

    def test_the_observation_cache_is_the_committed_fixture_not_the_result(self) -> None:
        result = self.replay_with("""
            out, cache = sys.argv[-2], sys.argv[-1]
            assert cache != out
            shutil.copyfile(cache, out)
        """)
        self.assertEqual(result["status"], "match")

    def test_a_result_that_moves_a_payout_differs(self) -> None:
        result = self.replay_with("""
            import json
            out, cache = sys.argv[-2], sys.argv[-1]
            fixture = json.load(open(cache))
            fixture["swaps"][0]["venue_out"][0] += 1
            json.dump(fixture, open(out, "w"))
        """)
        self.assertEqual(result["status"], "differs")

    def test_a_result_present_before_the_replay_fails(self) -> None:
        (self.results / MATRIX_CASE["fixture"]).write_bytes(self.committed_bytes)
        result = self.replay_with("sys.exit(0)")
        self.assertEqual(result["status"], "failed")


class RunDirTests(unittest.TestCase):
    def test_runs_keep_what_the_out_directory_already_holds(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            out = Path(temp_dir)
            unrelated = out / "notes.txt"
            unrelated.write_text("keep")
            first = replay_check.run_dir(out)
            second = replay_check.run_dir(out)
            self.assertEqual(unrelated.read_text(), "keep")
            self.assertNotEqual(first, second)
            self.assertEqual({first.parent, second.parent}, {out})


if __name__ == "__main__":
    unittest.main()

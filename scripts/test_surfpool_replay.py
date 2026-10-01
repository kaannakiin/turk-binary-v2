#!/usr/bin/env python3
import unittest

import surfpool_replay

PAID = {
    "landed": True,
    "version": 1,
    "error": None,
    "paid": "100",
    "expected_out": "100",
}


class PaidExactly(unittest.TestCase):
    def test_a_landed_v1_transaction_that_paid_the_quote_passes(self):
        self.assertTrue(surfpool_replay.all_paid([PAID]))

    def test_the_quoted_balance_does_not_pass_a_failed_or_missing_transaction(self):
        for broken in (
            {"error": {"InstructionError": [1, {"Custom": 6013}]}},
            {"landed": False},
            {"version": 0},
            {"paid": "99"},
        ):
            with self.subTest(broken=broken):
                self.assertFalse(surfpool_replay.all_paid([{**PAID, **broken}]))

    def test_a_run_without_plans_fails(self):
        self.assertFalse(surfpool_replay.all_paid([]))


if __name__ == "__main__":
    unittest.main()

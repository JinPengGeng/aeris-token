#!/usr/bin/env python3
"""Regression test for the generated gateway environment contract."""
import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "check_gateway_env_reference", ROOT / "tools" / "check_gateway_env_reference.py"
)
CHECKER = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(CHECKER)


class GatewayEnvironmentReferenceTest(unittest.TestCase):
    def test_reference_matches_source(self) -> None:
        clap, runtime = CHECKER.collect()
        expected = clap | runtime
        actual = CHECKER.documented_names()
        self.assertEqual(expected, actual)
        self.assertGreaterEqual(len(expected), 100)


if __name__ == "__main__":
    unittest.main()

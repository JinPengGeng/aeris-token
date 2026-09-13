#!/usr/bin/env python3
"""Regression tests for the bounded gateway environment source reference."""
import importlib.util
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("check_gateway_env_reference", ROOT / "tools" / "check_gateway_env_reference.py")
CHECKER = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(CHECKER)


class GatewayEnvironmentReferenceTest(unittest.TestCase):
    def test_reference_matches_complete_source_projection(self) -> None:
        self.assertTrue(CHECKER.check_document(CHECKER.DOC.read_text(encoding="utf-8")))

    def test_real_scope_and_runtime_helpers(self) -> None:
        clap, runtime = CHECKER.collect()
        self.assertEqual(sum(entry["scope"].startswith("gateway global") for entry in clap.values()), 13)
        self.assertTrue(clap["AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY"]["scope"].startswith("gateway root"))
        self.assertTrue(clap["AETHER_BACKUP_KEYRING_FILE"]["scope"].startswith("standalone aether-backup-restore"))
        self.assertIn("AETHER_UPDATE_DOWNLOAD_TIMEOUT_SECS", runtime)
        self.assertIn("AETHER_UPDATE_DOWNLOAD_IDLE_TIMEOUT_SECS", runtime)
        self.assertIn("AETHER_GATEWAY_REQUEST_CANDIDATE_PERSISTENCE", runtime)

    def test_default_type_scope_and_reader_drift_fail(self) -> None:
        original = '#[arg(long, env = "SAMPLE_TIMEOUT", default_value_t = 10, global = true)]\n/// Timeout in seconds.\ntimeout: u64,\n'
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            file = source / "main.rs"
            file.write_text(original, encoding="utf-8")
            document = CHECKER.render(*CHECKER.collect(source))
            for changed in [original.replace("= 10", "= 11"), original.replace("u64", "u32"),
                            original.replace("global = true", "global = false"),
                            original + '\nupdate_timeout_from_env("NEW_TIMEOUT_SECS", 10);\n']:
                with self.subTest(changed=changed):
                    file.write_text(changed, encoding="utf-8")
                    self.assertFalse(CHECKER.check_document(document, source))
            file.write_text(original, encoding="utf-8")
            self.assertTrue(CHECKER.check_document(document, source))

    def test_nested_defaults_and_unknown_env_syntax(self) -> None:
        values = CHECKER.attribute_values('env = "SAMPLE", default_value_t = function(1, 2), global = true')
        self.assertEqual(values["default_value_t"], "function(1, 2)")
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            (source / "main.rs").write_text('#[arg(env = ENV_CONSTANT)]\nfield: String,\n', encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "unsupported clap env declaration"):
                CHECKER.collect(source)

    def test_repeated_slashes_are_scanned_as_one_comment(self) -> None:
        comment = "///" * 20_000
        self.assertIsNone(CHECKER.field_after_attribute(comment, 0))
        field = CHECKER.field_after_attribute(comment + "\n timeout: u64,\n", 0)
        self.assertIsNotNone(field)
        self.assertEqual(field.group(1), "u64")


if __name__ == "__main__":
    unittest.main()

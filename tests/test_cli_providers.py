"""Offline checks against the compiled CLI; use only fake, isolated credentials."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


BINARY = Path(os.environ.get(
    "WUT_TEST_BINARY", str(Path(__file__).resolve().parents[1] / "target/debug/wut")
)).resolve()


class ProviderCliTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="wut-cli-test-")
        self.addCleanup(self.temporary.cleanup)
        self.environment = {
            "HOME": self.temporary.name,
            "XDG_CONFIG_HOME": self.temporary.name,
            "XDG_CACHE_HOME": self.temporary.name,
            "WUT_NO_UPDATE_CHECK": "1",
        }

    def run_cli(self, *args):
        return subprocess.run(
            [str(BINARY), *args], env=self.environment,
            capture_output=True, text=True, timeout=5,
        )

    def test_models_does_not_require_credentials_or_a_valid_config(self):
        path = Path(self.temporary.name) / "invalid.json"
        path.write_text("invalid config")
        self.environment["WUT_CONFIG"] = str(path)
        result = self.run_cli("--models")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("groq:openai/gpt-oss-20b", result.stdout)
        self.assertIn("GROQ_API_KEY", result.stdout)
        self.assertIn("CEREBRAS_API_KEY", result.stdout)
        self.assertNotIn("gemma-4-31b", result.stdout)

    def test_a_cerebras_key_is_never_used_for_groq(self):
        self.environment["CEREBRAS_API_KEY"] = "fake-cerebras-secret"
        result = self.run_cli("--model", "groq:openai/gpt-oss-20b", "hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no Groq API key", result.stderr)
        self.assertNotIn("fake-cerebras-secret", result.stdout + result.stderr)

    def test_a_groq_key_is_never_used_for_cerebras(self):
        self.environment["GROQ_API_KEY"] = "fake-groq-secret"
        result = self.run_cli("--model", "gpt-oss-120b", "hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no Cerebras API key", result.stderr)
        self.assertNotIn("fake-groq-secret", result.stdout + result.stderr)

    def test_invalid_models_and_effort_fail_before_authentication(self):
        for arguments, expected in [
            (("--model", "typo", "hello"), "unknown model"),
            (("--model", "groq:openai/gpt-oss-20b", "--reasoning", "none", "hello"), "not supported"),
        ]:
            result = self.run_cli(*arguments)
            self.assertEqual(result.returncode, 1)
            self.assertIn(expected, result.stderr)
            self.assertNotIn("no API key", result.stderr)

    def test_error_messages_do_not_emit_terminal_controls(self):
        result = self.run_cli("--model", "bad\x1b]52;c;RkFLRQ==\x07\u009b2J\rspoofed", "hello")
        self.assertEqual(result.returncode, 1)
        for control in ["\x1b", "\x07", "\u009b"]:
            self.assertNotIn(control, result.stderr)
        self.assertIn("unknown model", result.stderr)

    def test_unavailable_benchmarks_do_not_invent_latency_measurements(self):
        result = self.run_cli("--benchmark", "--model", "groq:openai/gpt-oss-20b")
        self.assertEqual(result.returncode, 1)
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["status"], "unavailable")
        self.assertNotIn("first_text_ms", rows[0])
        self.assertNotIn("total_ms", rows[0])

    def test_groq_config_is_honored_and_cli_overrides_leave_defaults_unchanged(self):
        config = Path(self.temporary.name) / "wut/config.json"
        config.parent.mkdir(mode=0o700)
        contents = json.dumps({
            "version": 4, "response_color": "default",
            "model": "groq:openai/gpt-oss-120b", "reasoning": "high",
        })
        config.write_text(contents)
        result = self.run_cli("hello")
        self.assertIn("no Groq API key", result.stderr)
        result = self.run_cli("--model", "gpt-oss-120b", "hello")
        self.assertIn("no Cerebras API key", result.stderr)
        self.assertEqual(config.read_text(), contents)

    def test_explicit_archived_model_reports_a_local_replacement(self):
        result = self.run_cli("--model", "gemma-4-31b", "hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("archived", result.stderr)
        self.assertIn("gpt-oss-120b", result.stderr)


if __name__ == "__main__":
    unittest.main()

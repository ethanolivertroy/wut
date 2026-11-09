"""Hermetic terminal checks for key setup; no real credentials or API requests."""
import json
import os
from pathlib import Path
import pty
import select
import subprocess
import tempfile
import termios
import time
import unittest


BINARY = Path(os.environ.get(
    "WUT_TEST_BINARY", os.environ.get(
        "WUT_BIN", str(Path(__file__).resolve().parents[1] / "target/debug/wut")
    )
)).resolve()


class Terminal:
    """Keep a slave open so its restored terminal flags can be checked."""

    def __init__(self, arguments, environment):
        self.master, self.slave = pty.openpty()
        self.before = termios.tcgetattr(self.slave)
        self.transcript = bytearray()
        self.process = subprocess.Popen(
            [str(BINARY), *arguments], stdin=self.slave, stdout=self.slave,
            stderr=self.slave, env=environment,
        )

    def read_once(self, timeout):
        if select.select([self.master], [], [], timeout)[0]:
            self.transcript.extend(os.read(self.master, 65536))

    def read_until(self, marker, after=0, timeout=8):
        deadline = time.monotonic() + timeout
        while marker not in self.transcript[after:]:
            if time.monotonic() >= deadline:
                raise AssertionError("terminal did not reach expected prompt")
            self.read_once(0.1)
            if self.process.poll() is not None and marker not in self.transcript[after:]:
                raise AssertionError("process exited before expected prompt")

    def send(self, data):
        os.write(self.master, data)

    def finish(self, timeout=8):
        deadline = time.monotonic() + timeout
        while self.process.poll() is None:
            if time.monotonic() >= deadline:
                raise AssertionError("terminal process did not exit")
            self.read_once(0.1)
        while select.select([self.master], [], [], 0)[0]:
            self.read_once(0)
        return (
            self.process.returncode, bytes(self.transcript),
            termios.tcgetattr(self.slave),
        )

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=5)
        os.close(self.master)
        os.close(self.slave)


class CredentialSetupTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="wut-key-setup-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        config = self.root / "config.json"
        config.write_text(json.dumps({
            "version": 4, "response_color": "default",
            "model": "deepseek/deepseek-v4.1-flash-1017-fast", "reasoning": "none",
        }))
        self.environment = {
            "HOME": str(self.root), "XDG_CONFIG_HOME": str(self.root / "config"),
            "XDG_CACHE_HOME": str(self.root / "cache"),
            "XDG_STATE_HOME": str(self.root / "state"),
            "WUT_CONFIG": str(config), "WUT_NO_UPDATE_CHECK": "1",
            "TERM": "xterm-256color",
            "HTTP_PROXY": "http://127.0.0.1:0",
            "HTTPS_PROXY": "http://127.0.0.1:0",
        }

    def credential_path(self, provider):
        directory = self.root / "config/wut"
        if provider != "cerebras":
            directory /= provider
        return directory / "credentials"

    def terminal(self, *arguments):
        terminal = Terminal(arguments, self.environment)
        self.addCleanup(terminal.close)
        return terminal

    def enter_key(self, provider, data, expected_code=0):
        terminal = self.terminal("--set-key", provider)
        terminal.read_until(b"input hidden")
        self.assertFalse(termios.tcgetattr(terminal.slave)[3] & termios.ECHO)
        terminal.send(data)
        code, output, after = terminal.finish()
        self.assertEqual(code, expected_code, output)
        self.assertEqual(after, terminal.before, "terminal mode was not restored")
        self.assertIn(b"\x1b[?2004h", output)
        self.assertIn(b"\x1b[?2004l", output)
        return output

    def test_hidden_entry_and_backspace_save_each_provider_privately(self):
        for provider in ("cerebras", "groq", "ai-gateway"):
            with self.subTest(provider=provider):
                output = self.enter_key(provider, b"fixture-hidden-X\x7fY\r")
                self.assertNotIn(b"fixture-hidden", output)
                path = self.credential_path(provider)
                self.assertEqual(path.read_text(), "fixture-hidden-Y")
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                self.assertEqual(path.parent.stat().st_mode & 0o777, 0o700)

    def test_cancel_preserves_existing_key_and_restores_terminal(self):
        self.enter_key("ai-gateway", b"fixture-original\r")
        for cancel in (b"\x03", b"\x1b", b"\x04"):
            with self.subTest(cancel=cancel):
                output = self.enter_key("ai-gateway", b"fixture-cancelled" + cancel)
                self.assertNotIn(b"fixture-cancelled", output)
                self.assertEqual(
                    self.credential_path("ai-gateway").read_text(), "fixture-original",
                )

    def test_empty_and_multiline_paste_do_not_overwrite_or_echo(self):
        self.enter_key("ai-gateway", b"fixture-original\r")
        for data in (
            b"\r",
            b"\x1b[200~fixture-first\nfixture-second\x1b[201~\r",
        ):
            with self.subTest(data=data):
                output = self.enter_key("ai-gateway", data, expected_code=1)
                self.assertNotIn(b"fixture-first", output)
                self.assertNotIn(b"fixture-second", output)
                self.assertEqual(
                    self.credential_path("ai-gateway").read_text(), "fixture-original",
                )

    def test_paste_with_one_trailing_newline_is_saved_after_enter(self):
        output = self.enter_key(
            "ai-gateway", b"\x1b[200~fixture-pasted\n\x1b[201~\r",
        )
        self.assertNotIn(b"fixture-pasted", output)
        self.assertEqual(self.credential_path("ai-gateway").read_text(), "fixture-pasted")

    def test_ordinary_use_rejects_broad_permissions_and_setup_repairs_them(self):
        self.enter_key("ai-gateway", b"fixture-original\r")
        path = self.credential_path("ai-gateway")
        path.chmod(0o644)
        # This must fail before authentication: no live model request is made.
        result = subprocess.run(
            [str(BINARY), "hello"], env=self.environment, stdin=subprocess.DEVNULL,
            capture_output=True, timeout=5,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn(b"0600", result.stderr)
        self.assertNotIn(b"fixture-original", result.stdout + result.stderr)
        output = self.enter_key("ai-gateway", b"fixture-repaired\r")
        self.assertNotIn(b"fixture-repaired", output)
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        self.assertEqual(path.read_text(), "fixture-repaired")

    def test_pipes_and_command_line_key_values_are_rejected_without_disclosure(self):
        for arguments, input_value, secret in (
            (("--set-key", "ai-gateway"), b"fixture-piped", b"fixture-piped"),
            (("--set-key", "ai-gateway", "fixture-argument"), None, b"fixture-argument"),
        ):
            with self.subTest(arguments=arguments):
                result = subprocess.run(
                    [str(BINARY), *arguments], env=self.environment,
                    input=input_value, capture_output=True, timeout=5,
                )
                self.assertEqual(result.returncode, 1)
                self.assertNotIn(secret, result.stdout + result.stderr)
        self.assertFalse(self.credential_path("ai-gateway").exists())

    def test_provider_chooser_saves_the_selected_provider(self):
        terminal = self.terminal("--set-key")
        terminal.read_until(b"Choose a provider")
        terminal.send(b"\x1b[B\x1b[B\r")
        terminal.read_until(b"input hidden")
        terminal.send(b"fixture-chooser\r")
        code, output, after = terminal.finish()
        self.assertEqual(code, 0, output)
        self.assertEqual(after, terminal.before)
        self.assertNotIn(b"fixture-chooser", output)
        self.assertEqual(self.credential_path("ai-gateway").read_text(), "fixture-chooser")
        self.assertFalse(self.credential_path("cerebras").exists())
        self.assertFalse(self.credential_path("groq").exists())

    def test_settings_saves_current_provider_and_restores_the_menu(self):
        terminal = self.terminal("--settings")
        terminal.read_until(b"Defaults and answer instructions")
        terminal.send(b"\x1b[B" * 4 + b"\r")
        terminal.read_until(b"input hidden")
        after_prompt = len(terminal.transcript)
        terminal.send(b"fixture-settings\r")
        terminal.read_until(b"Defaults and answer instructions", after=after_prompt)
        terminal.send(b"\x1b")
        code, output, after = terminal.finish()
        self.assertEqual(code, 0, output)
        self.assertEqual(after, terminal.before)
        self.assertNotIn(b"fixture-settings", output)
        self.assertEqual(self.credential_path("ai-gateway").read_text(), "fixture-settings")
        self.assertFalse(self.credential_path("cerebras").exists())

    def test_environment_override_notice_names_the_variable_not_the_value(self):
        self.environment["AI_GATEWAY_API_KEY"] = "fixture-environment-secret"
        output = self.enter_key("ai-gateway", b"fixture-saved-secret\r")
        self.assertIn(b"AI_GATEWAY_API_KEY is set and takes precedence", output)
        self.assertNotIn(b"fixture-environment-secret", output)
        self.assertNotIn(b"fixture-saved-secret", output)


if __name__ == "__main__":
    unittest.main()

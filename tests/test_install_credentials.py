"""Exercise installer credential setup with fake binaries and no real keys."""

import os
from pathlib import Path
import pty
import select
import subprocess
import tempfile
import time
import unittest


INSTALLER = (Path(__file__).resolve().parents[1] / "install.sh").read_text()
FUNCTIONS = INSTALLER.rsplit('\nmain "$@"', 1)[0]
COMMAND = FUNCTIONS + '\ninstall_dir="$WUT_TEST_INSTALL_DIR"\ninstall_cerebras_key\n'
FAKE_BINARY = """#!/bin/sh
case "$1" in
  --help)
    printf '%s\\n' "${WUT_TEST_HELP:---set-key}"
    ;;
  --set-key)
    [ "$#" -eq 2 ] && [ "$2" = cerebras ] || exit 8
    [ -t 0 ] && [ -t 1 ] && [ -t 2 ] || exit 9
    printf '%s\\n' "$*" > "$WUT_TEST_LOG"
    exit "${WUT_TEST_STATUS:-0}"
    ;;
  *) exit 10 ;;
esac
"""


class CredentialsInstallerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="wut-installer-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.directory = self.root / "config/wut"
        self.credentials = self.directory / "credentials"
        self.install_directory = self.root / "installed wut"
        self.install_directory.mkdir()
        binary = self.install_directory / "wut"
        binary.write_text(FAKE_BINARY)
        binary.chmod(0o700)
        self.invocations = self.root / "invocations"
        self.environment = {
            "HOME": os.environ.get("HOME", "/nonexistent"),
            "PATH": os.environ["PATH"],
            "XDG_CONFIG_HOME": str(self.root / "config"),
            "WUT_TEST_INSTALL_DIR": str(self.install_directory),
            "WUT_TEST_LOG": str(self.invocations),
        }

    def run_setup(self):
        return subprocess.run(
            ["sh", "-c", COMMAND], env=self.environment,
            capture_output=True, text=True, timeout=5,
        )

    def run_interactive_setup(self, answer="y"):
        pid, terminal = pty.fork()
        if pid == 0:
            os.execve("/bin/sh", ["sh", "-c", COMMAND], self.environment)
        output = b""
        answered = False
        deadline = time.monotonic() + 10
        try:
            while time.monotonic() < deadline:
                ready, _, _ = select.select([terminal], [], [], 0.1)
                if not ready:
                    continue
                try:
                    chunk = os.read(terminal, 4096)
                except OSError:
                    break
                if not chunk:
                    break
                output += chunk
                if b"[y/N]" in output and not answered and answer is not None:
                    os.write(terminal, answer.encode() + b"\n")
                    answered = True
            else:
                self.fail("installer prompt timed out")
        finally:
            os.close(terminal)
            # Ensure a broken prompt cannot leave a hanging test child.
            child, status = os.waitpid(pid, os.WNOHANG)
            if child == 0:
                os.kill(pid, 15)
                _, status = os.waitpid(pid, 0)
        return os.waitstatus_to_exitcode(status), output

    def test_existing_key_is_left_for_binary_validation_without_rewriting(self):
        self.directory.mkdir(parents=True, mode=0o755)
        self.directory.chmod(0o755)
        self.credentials.write_text("fake-existing-key\n")
        self.credentials.chmod(0o644)
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.directory.stat().st_mode & 0o777, 0o755)
        self.assertEqual(self.credentials.stat().st_mode & 0o777, 0o644)
        self.assertEqual(self.credentials.read_text(), "fake-existing-key\n")
        self.assertIn("left unchanged", result.stderr)
        self.assertNotIn("fake-existing-key", result.stdout + result.stderr)
        self.assertFalse(self.invocations.exists())

    def test_rejects_symlinked_directory(self):
        target = self.root / "target"
        target.mkdir(mode=0o755)
        target.chmod(0o755)
        self.directory.parent.mkdir()
        self.directory.symlink_to(target, target_is_directory=True)
        result = self.run_setup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must not be a symlink", result.stderr)
        self.assertEqual(target.stat().st_mode & 0o777, 0o755)
        self.assertFalse(self.invocations.exists())

    def test_rejects_symlinked_file_without_touching_target(self):
        self.directory.mkdir(parents=True)
        target = self.root / "target"
        target.write_text("untouched")
        target.chmod(0o644)
        self.credentials.symlink_to(target)
        result = self.run_setup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must not be a symlink", result.stderr)
        self.assertEqual(target.read_text(), "untouched")
        self.assertEqual(target.stat().st_mode & 0o777, 0o644)
        self.assertFalse(self.invocations.exists())

    def test_rejects_fifo_without_waiting_or_delegating(self):
        self.directory.mkdir(parents=True)
        os.mkfifo(self.credentials)
        result = self.run_setup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be a regular file", result.stderr)
        self.assertFalse(self.invocations.exists())

    def test_environment_key_is_not_saved(self):
        self.environment["CEREBRAS_API_KEY"] = "fake-environment-key"
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.directory.exists())
        self.assertFalse(self.invocations.exists())
        self.assertNotIn("fake-environment-key", result.stdout + result.stderr)

    def test_noninteractive_setup_explains_command_without_creating_files(self):
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("wut --set-key cerebras", result.stderr)
        self.assertFalse(self.directory.exists())
        self.assertFalse(self.invocations.exists())

    def test_interactive_setup_delegates_to_installed_binary_with_tty(self):
        code, output = self.run_interactive_setup()
        self.assertEqual(code, 0, output)
        self.assertIn(b"not encrypted", output)
        self.assertEqual(self.invocations.read_text(), "--set-key cerebras\n")
        # Only the binary owns credential input and storage now.
        self.assertFalse(self.directory.exists())

    def test_declining_setup_does_not_invoke_key_writer(self):
        code, output = self.run_interactive_setup(answer="n")
        self.assertEqual(code, 0, output)
        self.assertIn(b"wut --set-key cerebras later", output)
        self.assertFalse(self.directory.exists())
        self.assertFalse(self.invocations.exists())

    def test_older_binary_has_no_shell_key_writer_fallback(self):
        self.environment["WUT_TEST_HELP"] = "Usage: old wut"
        code, output = self.run_interactive_setup(answer=None)
        self.assertEqual(code, 0, output)
        self.assertIn(b"does not support built-in key setup", output)
        self.assertNotIn(b"[y/N]", output)
        self.assertFalse(self.directory.exists())
        self.assertFalse(self.invocations.exists())

    def test_binary_key_setup_failure_is_not_silently_ignored(self):
        self.environment["WUT_TEST_STATUS"] = "7"
        code, output = self.run_interactive_setup()
        self.assertEqual(code, 7, output)
        self.assertEqual(self.invocations.read_text(), "--set-key cerebras\n")
        self.assertFalse(self.directory.exists())


if __name__ == "__main__":
    unittest.main()

"""Exercise the installer's credential setup without building or installing wut."""

import os
from pathlib import Path
import pty
import select
import subprocess
import tempfile
import termios
import time
import unittest


INSTALLER = (Path(__file__).resolve().parents[1] / "install.sh").read_text()
FUNCTIONS = INSTALLER.rsplit('\nmain "$@"', 1)[0]
COMMAND = FUNCTIONS + "\ninstall_cerebras_key\n"


class CredentialsInstallerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="wut-installer-test-")
        self.addCleanup(self.temporary.cleanup)
        self.home = Path(self.temporary.name)
        self.directory = self.home / ".config/wut"
        self.credentials = self.directory / "credentials"
        self.environment = {"HOME": str(self.home), "PATH": os.environ["PATH"]}

    def run_setup(self):
        return subprocess.run(
            ["sh", "-c", COMMAND], env=self.environment,
            capture_output=True, text=True, timeout=5,
        )

    def test_repairs_permissions_without_changing_existing_key(self):
        self.directory.mkdir(parents=True, mode=0o755)
        self.credentials.write_text("fake-existing-key\n")
        self.credentials.chmod(0o644)
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.directory.stat().st_mode & 0o777, 0o700)
        self.assertEqual(self.credentials.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.credentials.read_text(), "fake-existing-key\n")
        self.assertNotIn("fake-existing-key", result.stdout + result.stderr)

    def test_rejects_symlinked_directory(self):
        target = self.home / "target"
        target.mkdir(mode=0o755)
        self.directory.parent.mkdir()
        self.directory.symlink_to(target, target_is_directory=True)
        result = self.run_setup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must not be a symlink", result.stderr)
        self.assertEqual(target.stat().st_mode & 0o777, 0o755)

    def test_rejects_symlinked_file_without_touching_target(self):
        self.directory.mkdir(parents=True)
        target = self.home / "target"
        target.write_text("untouched")
        target.chmod(0o644)
        self.credentials.symlink_to(target)
        result = self.run_setup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must not be a symlink", result.stderr)
        self.assertEqual(target.read_text(), "untouched")
        self.assertEqual(target.stat().st_mode & 0o777, 0o644)

    def test_rejects_fifo(self):
        self.directory.mkdir(parents=True)
        os.mkfifo(self.credentials)
        result = self.run_setup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be a regular file", result.stderr)

    def test_environment_key_is_not_saved(self):
        self.environment["CEREBRAS_API_KEY"] = "fake-environment-key"
        result = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.credentials.exists())
        self.assertNotIn("fake-environment-key", result.stdout + result.stderr)

    def test_interactive_save_is_private_and_does_not_echo_key(self):
        pid, terminal = pty.fork()
        if pid == 0:
            os.execve("/bin/sh", ["sh", "-c", COMMAND], self.environment)
        output = b""
        answered = entered = False
        deadline = time.monotonic() + 10
        try:
            while time.monotonic() < deadline:
                # The prompt may arrive before stty disables echo, without
                # any further output to wake select once echo is disabled.
                if answered and not entered:
                    if not termios.tcgetattr(terminal)[3] & termios.ECHO:
                        os.write(terminal, b"fake-interactive-key\n")
                        entered = True
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
                if b"[y/N]" in output and not answered:
                    os.write(terminal, b"y\n")
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
        self.assertEqual(os.waitstatus_to_exitcode(status), 0, output)
        self.assertIn(b"not encrypted", output)
        self.assertNotIn(b"fake-interactive-key", output)
        self.assertEqual(self.credentials.read_text(), "fake-interactive-key\n")
        self.assertEqual(self.directory.stat().st_mode & 0o777, 0o700)
        self.assertEqual(self.credentials.stat().st_mode & 0o777, 0o600)
        self.assertEqual(list(self.directory.iterdir()), [self.credentials])


if __name__ == "__main__":
    unittest.main()

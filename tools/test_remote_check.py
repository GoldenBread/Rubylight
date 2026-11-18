import os, pathlib, shutil, subprocess, sys, tempfile, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import build_lock  # noqa: E402

TOOLS = pathlib.Path(__file__).resolve().parent
REMOTE = [TOOLS / 'remote-check/start.ps1', TOOLS / 'remote-check/run.ps1']
SHELLS = [s for s in ('pwsh', 'powershell') if shutil.which(s)]


@unittest.skipUnless(SHELLS, 'PowerShell')
class RemoteCheck(unittest.TestCase):
    def test_the_remote_scripts_parse_in_windows_powershell_and_stay_ascii(self):
        for script in REMOTE + [TOOLS / 'remote-check.ps1']:
            for shell in SHELLS:
                with self.subTest(script=script.name, shell=shell):
                    command = ("$e = $null; [void][System.Management.Automation.Language.Parser]::ParseFile("
                               f"'{script}', [ref]$null, [ref]$e); $e.Count")
                    out = subprocess.run([shell, '-NoProfile', '-Command', command], capture_output=True, text=True, timeout=60)
                    self.assertEqual(out.stdout.strip(), '0', out.stdout + out.stderr)
        for script in REMOTE:
            # Windows PowerShell 5.1 reads a script without BOM in the ANSI code page.
            script.read_bytes().decode('ascii')

    def start(self, locks, pool):
        shell = 'powershell' if 'powershell' in SHELLS else 'pwsh'
        return subprocess.run([shell, '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(REMOTE[0]),
                               '-Branch', 'wip/x', '-Name', 'wip-x', '-Locks', str(locks), '-Pool', str(pool),
                               '-Work', str(locks / 'work'), '-User', 'nobody-here',
                               # Not this machine's real tasks: this test may itself run in a remote check.
                               '-Tasks', 'rl-test-no-such-task-*'],
                              capture_output=True, text=True, timeout=60)

    def test_a_busy_machine_is_refused_before_any_task_exists(self):
        with tempfile.TemporaryDirectory() as directory:
            locks = pathlib.Path(directory)
            (locks / 'BUILD-LOCK').write_text('dev host C:\\src\\rl-check\\wip-y pid=1')
            out = self.start(locks, locks / 'pool')
            self.assertEqual(out.returncode, 4, out.stdout + out.stderr)
            self.assertIn('BUILD-LOCK: dev host', out.stdout)
            (locks / 'BUILD-LOCK').unlink()
            lane = build_lock.HandleSlot(locks / 'pool' / '1.lock')
            self.assertTrue(lane.try_take('rl-quick'))
            try:
                out = self.start(locks, locks / 'pool')
            finally:
                lane.release()
            self.assertEqual(out.returncode, 4, out.stdout + out.stderr)
            self.assertIn('1.lock', out.stdout)
            # Free, but the console user is not the one the task would run as.
            out = self.start(locks, locks / 'pool')
            self.assertEqual(out.returncode, 5, out.stdout + out.stderr)
            self.assertFalse((locks / 'work').exists())


if __name__ == '__main__':
    unittest.main()

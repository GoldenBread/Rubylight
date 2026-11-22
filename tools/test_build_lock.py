import concurrent.futures, json, os, pathlib, queue, shutil, subprocess, sys, tempfile, threading, time, unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import build_lock


class BuildLease(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = pathlib.Path(temp.name)
        self.lock = self.root / 'BUILD-LOCK'
        self.environment = mock.patch.dict(os.environ)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        os.environ.pop(build_lock.OWNER_ENV, None)
        os.environ.pop(build_lock.PATH_ENV, None)

    def cli(self, *command, timeout=0):
        return [sys.executable, str(pathlib.Path(build_lock.__file__)), '--lock-directory', str(self.root),
                '--timeout', str(timeout), '--', *command]

    def run_cli(self, *command, **kwargs):
        return subprocess.run(self.cli(*command), capture_output=True, text=True, timeout=20, **kwargs)

    def test_nested_commands_reuse_the_lease_and_preserve_exit_status(self):
        with build_lock.BuildLock(self.root, 'parent') as lease:
            before = self.lock.read_text()
            # The nested wrapper inherits the custom directory too, without
            # needing another --lock-directory on every child command.
            result = subprocess.run([sys.executable, build_lock.__file__, '--timeout', '0', '--',
                                     sys.executable, '-c', 'import sys; sys.exit(7)'],
                                    capture_output=True, text=True, timeout=15)
            self.assertEqual(result.returncode, 7, result.stderr)
            self.assertEqual(self.lock.read_text(), before)
        self.assertFalse(self.lock.exists())
        self.assertNotIn(build_lock.OWNER_ENV, os.environ)

    def test_unrelated_process_waits_and_names_the_holder(self):
        with build_lock.BuildLock(self.root, 'first'):
            env = dict(os.environ)
            env.pop(build_lock.OWNER_ENV)
            result = self.run_cli(sys.executable, '-c', 'raise AssertionError("must not run")', env=env)
            self.assertEqual(result.returncode, 1)
            self.assertIn('first', result.stderr)
            self.assertIn('No lock removed', result.stderr)
        self.assertFalse(self.lock.exists())

    def test_an_old_or_dead_owners_lock_is_never_removed(self):
        self.lock.write_text('old build pid=4294967')
        os.utime(self.lock, (1, 1))
        result = self.run_cli(sys.executable, '-c', 'pass')
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.lock.read_text(), 'old build pid=4294967')

    def test_waiter_runs_only_after_the_holder_releases(self):
        with build_lock.BuildLock(self.root):
            env = dict(os.environ)
            env.pop(build_lock.OWNER_ENV)
            marker = self.root / 'ran'
            child = subprocess.Popen(self.cli(sys.executable, '-c',
                'import pathlib,sys; pathlib.Path(sys.argv[1]).touch()', str(marker), timeout=10),
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            try:
                time.sleep(.5)
                self.assertIsNone(child.poll())
                self.assertFalse(marker.exists())
            except BaseException:
                child.kill()
                child.communicate()
                raise
        _, error = child.communicate(timeout=15)
        self.assertEqual(child.returncode, 0, error)
        self.assertTrue(marker.exists())

    def test_interrupted_owner_keeps_lock_while_its_child_survives(self):
        pidfile = self.root / 'child'
        owner = subprocess.Popen(self.cli(sys.executable, '-c',
            'import os,pathlib,sys,time; pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(60)', str(pidfile)),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        child_pid = None
        try:
            # The file exists, empty, before the child has written its pid into it.
            deadline, written = time.monotonic() + 15, ''
            while not written and time.monotonic() < deadline:
                time.sleep(.05)
                written = pidfile.read_text() if pidfile.exists() else ''
            self.assertTrue(written, 'the child wrote no pid')
            child_pid = int(written)
            owner.kill()
            owner.wait(timeout=5)
            result = self.run_cli(sys.executable, '-c', 'pass')
            self.assertEqual(result.returncode, 1)
            self.assertTrue(self.lock.exists())
            self.assertTrue(build_lock.priority_alive(f'pid={child_pid}'))
        finally:
            if owner.poll() is None:
                owner.kill()
                owner.wait()
            if child_pid:
                if os.name == 'nt':
                    subprocess.run(['taskkill', '/PID', str(child_pid), '/F'], capture_output=True)
                else:
                    import signal
                    os.kill(child_pid, signal.SIGKILL)

    def test_live_release_priority_blocks_but_reused_pid_does_not(self):
        priority = self.root / 'PRIORITY'
        priority.write_text(f'release pid={os.getpid()}')
        self.assertEqual(self.run_cli(sys.executable, '-c', 'pass').returncode, 1)
        self.assertFalse(self.lock.exists())
        if os.name == 'nt':
            priority.write_text(f'release pid={os.getpid()} start=2001-01-01T00:00:00.0000000Z')
            result = self.run_cli(sys.executable, '-c', 'pass')
            self.assertEqual(result.returncode, 0, result.stderr)

    @unittest.skipUnless(os.name == 'nt', 'Windows delete sharing')
    def test_live_handle_prevents_legacy_expiry_deletion(self):
        with build_lock.BuildLock(self.root):
            with self.assertRaises(PermissionError):
                self.lock.unlink()

    def test_an_ownership_change_is_left_alone(self):
        with self.assertRaisesRegex(RuntimeError, 'ownership changed'):
            with build_lock.BuildLock(self.root):
                self.lock.write_text('other owner')
        self.assertEqual(self.lock.read_text(), 'other owner')

    def test_interrupt_does_not_release_until_the_child_exits(self):
        child = mock.Mock()
        child.wait.side_effect = [KeyboardInterrupt, 0]
        self.assertEqual(build_lock.wait_child(child), 0)
        self.assertEqual(child.wait.call_count, 2)

    @unittest.skipUnless(shutil.which('pwsh'), 'PowerShell 7')
    def test_powershell_builds_share_python_lease_and_observe_release_priority(self):
        driver = self.root / 'lease.ps1'
        driver.write_text("""param([string]$Locks)
$ErrorActionPreference = 'Stop'
. $env:CHECK_STREAMS
Initialize-CheckLocks $Locks
$CheckBuildDeadline = 0.3
$held = Enter-CheckBuildLock -RespectPriority
try { 'acquired' } finally { Exit-CheckLocks $held -NoPause }
""")
        env = dict(os.environ, CHECK_STREAMS=str(pathlib.Path(__file__).resolve().parents[1] / 'rust/release/check_streams.ps1'))
        def run():
            return subprocess.run(['pwsh', '-NoProfile', '-File', str(driver), '-Locks', str(self.root)],
                                  capture_output=True, text=True, env=env, timeout=45)
        with build_lock.BuildLock(self.root):
            before = self.lock.read_text()
            child = run()
            self.assertEqual(child.returncode, 0, child.stderr)
            self.assertEqual(self.lock.read_text(), before)
        (self.root / 'PRIORITY').write_text(f'release pid={os.getpid()}')
        child = run()
        self.assertEqual(child.returncode, 1)
        self.assertIn('release priority', child.stdout)
        self.assertNotIn('acquired', child.stdout)
        self.assertFalse(self.lock.exists())

    @unittest.skipUnless(shutil.which('pwsh'), 'PowerShell 7')
    def test_full_build_lease_preserves_shell_environment_and_failure(self):
        repo = self.root / 'checkout with spaces'
        (repo / 'rust').mkdir(parents=True)
        (repo / 'rust/release').mkdir()
        original = pathlib.Path(__file__).resolve().parents[1] / 'rust/build.ps1'
        # Exercise the real lease setup, substituting only the SDK/build work.
        prefix = original.read_text().split("$toolchain =", 1)[0]
        script = repo / 'rust/build.ps1'
        script.write_text(prefix + """
@{deps=$Dependencies; package=[bool]$Package; stage=$Stage; jobs=$env:CARGO_BUILD_JOBS; pyro=$env:PYROWAVE_JOBS} | ConvertTo-Json -Compress
throw 'synthetic failure'
} finally { Exit-CheckLocks $buildLease -NoPause }
""")
        shutil.copyfile(original.parent / 'release/check_streams.ps1', repo / 'rust/release/check_streams.ps1')
        env = dict(os.environ, RUBYLIGHT_LOCK_DIRECTORY=str(self.root))
        for shell in ('pwsh', 'powershell') if os.name == 'nt' else ('pwsh',):
            with self.subTest(shell=shell):
                result = subprocess.run([shell, '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(script), '-Dependencies', "space's and $literal", '-Package',
                                         '-Stage', 'Build', '-Jobs', '3'], env=env, capture_output=True, text=True, timeout=45)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn('synthetic failure', result.stderr)
                receipt = json.loads(result.stdout)
                self.assertEqual(receipt, dict(deps="space's and $literal", package=True, stage='Build', jobs='3', pyro='3'))
                self.assertFalse(self.lock.exists())


def kill_tree(child):
    """The wrapper and the command it started (which outlives a killed wrapper)."""
    if child.poll() is None and os.name == 'nt':
        subprocess.run(['taskkill', '/PID', str(child.pid), '/T', '/F'], capture_output=True)
    if child.poll() is None:
        child.kill()
    child.wait(timeout=15)


HOLD = (
    'import pathlib,sys,time\n'
    'pathlib.Path(sys.argv[1]).touch()\n'
    'if len(sys.argv) > 3:\n'
    '    stop, deadline = pathlib.Path(sys.argv[3]), time.monotonic() + float(sys.argv[2])\n'
    '    while not stop.exists() and time.monotonic() < deadline:\n'
    '        time.sleep(0.05)\n'
    'else:\n'
    '    time.sleep(float(sys.argv[2]))\n'
)


class Lanes(unittest.TestCase):
    """Per-stage lanes: stages of different kinds run side by side, BUILD-LOCK stays exclusive."""

    def setUp(self):
        temp = tempfile.TemporaryDirectory(ignore_cleanup_errors=True)  # a killed holder closes its lane a moment later
        self.addCleanup(temp.cleanup)
        self.root = pathlib.Path(temp.name)
        self.environment = mock.patch.dict(os.environ, {build_lock.CARGO_SLOTS_ENV: '1'})
        self.environment.start()
        self.addCleanup(self.environment.stop)
        for name in (build_lock.OWNER_ENV, build_lock.PATH_ENV, build_lock.LANES_ENV, build_lock.POOL_ENV,
                     build_lock.GRADLE_ENV, 'RUBYLIGHT_LOCK_DIRECTORY'):
            os.environ.pop(name, None)
        self.children = []

    def tearDown(self):
        for child in self.children:
            kill_tree(child)

    def hold(self, lane, name, seconds=60, directory=None, stop=None):
        """A separate process holding a lane until signaled or killed; returns once it holds it."""
        marker = self.root / f'{name}.held'
        command = [sys.executable, build_lock.__file__, '--lock-directory', str(directory or self.root), '--timeout', '10',
                   *(['--lane', lane] if lane else []), '--', sys.executable, '-c', HOLD, str(marker), str(seconds),
                   *([str(stop)] if stop is not None else [])]
        output = self.root / f'{name}.out'
        with output.open('w') as out:
            child = subprocess.Popen(command, stdout=out, stderr=subprocess.STDOUT)
        self.children.append(child)
        deadline = time.monotonic() + 30
        while not marker.exists():
            self.assertIsNone(child.poll(), output.read_text() if child.poll() is not None else '')
            self.assertLess(time.monotonic(), deadline, 'the holder never started')
            time.sleep(.05)
        return child

    def lease(self, *lanes, timeout=0):
        groups = [g for lane in lanes for g in build_lock.LANE_GROUPS[lane](self.root)]
        return build_lock.StageLease(groups, self.root, 'test', timeout, poll=.05, say=lambda *a, **k: None)

    def test_slots_follow_threads_and_memory(self):
        os.environ.pop(build_lock.CARGO_SLOTS_ENV)
        # This PC: 16 threads, 64 GB. The laptop: 16 threads, 31 GB.
        self.assertEqual(build_lock.cargo_slots(16, 63.9), 4)
        self.assertEqual(build_lock.cargo_slots(16, 31.3), 2)
        self.assertEqual(build_lock.cargo_slots(4, 8), 1)
        os.environ[build_lock.CARGO_SLOTS_ENV] = '3'
        self.assertEqual(build_lock.cargo_slots(16, 8), 3)

    def test_stages_of_different_kinds_run_side_by_side(self):
        self.hold('cargo', 'host')
        with self.lease('gradle') as gradle, self.lease('web') as web:
            self.assertEqual(set(gradle.taken), {'gradle'})
            self.assertEqual(set(web.taken), {'web'})
        with self.assertRaisesRegex(TimeoutError, 'cargo slots busy: build'):
            with self.lease('cargo'):
                pass

    def test_cargo_slots_count(self):
        os.environ[build_lock.CARGO_SLOTS_ENV] = '2'
        with self.lease('cargo') as first, self.lease('cargo') as second:
            self.assertNotEqual(first.taken['cargo'].path, second.taken['cargo'].path)
            with self.assertRaises(TimeoutError):
                with self.lease('cargo'):
                    pass

    def test_a_killed_stage_gives_its_lane_back(self):
        holder = self.hold('cargo', 'host')
        kill_tree(holder)
        with self.lease('cargo', timeout=5) as lease:
            self.assertTrue(lease.taken['cargo'].path.exists())

    def test_a_stage_waits_while_build_lock_is_held_and_takes_no_lane(self):
        self.hold(None, 'exclusive')
        with self.assertRaisesRegex(TimeoutError, 'BUILD-LOCK: build'):
            with self.lease('web', timeout=.3):
                pass
        self.assertFalse(build_lock.HandleSlot(self.root / 'lanes/web-1.lock').busy())

    def test_a_stage_waits_for_a_live_release_priority(self):
        (self.root / 'PRIORITY').write_text(f'release pid={os.getpid()}')
        with self.assertRaisesRegex(TimeoutError, 'release priority'):
            with self.lease('web', timeout=.2):
                pass

    def test_build_lock_waits_until_the_stages_are_done(self):
        stage = self.hold('cargo', 'stage', seconds=1.5)
        started = time.monotonic()
        with build_lock.BuildLock(self.root, timeout=30, poll=.05):
            self.assertGreater(time.monotonic() - started, .5)
            self.assertFalse(build_lock.HandleSlot(self.root / 'lanes/cargo-1.lock').busy())
        self.assertEqual(stage.wait(timeout=30), 0)

    def test_a_gradle_lane_left_by_a_dead_owner_is_named_not_waited_for_or_removed(self):
        lane = self.root / 'gradle-lane'
        lane.mkdir()
        (lane / 'BUILD-LOCK').write_text('dev app C:\\src\\x 2026-11-18 pid=4294967 token=t')
        with build_lock.BuildLock(self.root, timeout=2, poll=.05):
            pass
        self.assertTrue((lane / 'BUILD-LOCK').exists())
        with self.assertRaisesRegex(TimeoutError, 'its owner is gone'):
            with self.lease('gradle'):
                pass

    def test_the_documented_gradle_call_does_not_wait_for_itself(self):
        lane = self.root / 'gradle-lane'
        os.environ[build_lock.GRADLE_ENV] = str(lane)
        result = subprocess.run([sys.executable, build_lock.__file__, '--lock-directory', str(lane), '--timeout', '2',
                                 '--', sys.executable, '-c', 'pass'], capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_build_lock_gives_up_without_removing_a_stage_lane(self):
        self.hold('gradle', 'gradle')
        result = subprocess.run([sys.executable, build_lock.__file__, '--lock-directory', str(self.root), '--timeout', '0.5',
                                 '--', sys.executable, '-c', 'raise SystemExit("must not run")'],
                                capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('stages still running', result.stderr)
        self.assertFalse((self.root / 'BUILD-LOCK').exists())
        self.assertTrue((self.root / 'gradle-lane/BUILD-LOCK').exists())

    def test_an_exclusive_owner_inside_a_stage_does_not_wait_for_its_own_lanes(self):
        with self.lease('cargo'):
            result = subprocess.run([sys.executable, build_lock.__file__, '--lock-directory', str(self.root),
                                     '--timeout', '2', '--', sys.executable, '-c', 'pass'],
                                    capture_output=True, text=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_the_gradle_lane_is_the_one_gradle_callers_take(self):
        # `build_lock.py --lock-directory <gradle lane> -- gradlew ...` outside the dev check.
        lane = self.root / 'gradle-lane'
        manual = self.hold(None, 'manual', directory=lane)
        with self.assertRaisesRegex(TimeoutError, 'gradle slots busy: build'):
            with self.lease('gradle'):
                pass
        kill_tree(manual)
        (lane / 'BUILD-LOCK').unlink(missing_ok=True)  # what a person does after checking the killed owner
        with self.lease('gradle'):
            self.assertIn('test', (lane / 'BUILD-LOCK').read_text())

    def test_private_locks_keep_the_pool_and_gradle_lane_beside_them(self):
        self.assertEqual(build_lock.pool_directory(self.root), self.root.resolve() / 'target-pool')
        self.assertEqual(build_lock.gradle_lane(self.root), self.root.resolve() / 'gradle-lane')
        self.assertEqual(build_lock.pool_directory(build_lock.DEFAULT_DIRECTORY), build_lock.DEFAULT_POOL)

    def test_rl_quick_slots_and_python_slots_exclude_each_other(self):
        pool = self.root / 'pool'
        slot = build_lock.HandleSlot(pool / '1.lock')
        self.assertTrue(slot.try_take('python'))
        try:
            other = build_lock.HandleSlot(pool / '1.lock')
            self.assertFalse(other.try_take('other'))
            if shutil.which('pwsh'):
                # As C:\src\rl-env.ps1's Enter-RlSlot opens it.
                script = f"try {{ [IO.File]::Open('{pool / '1.lock'}', 'OpenOrCreate', 'ReadWrite', 'None').Dispose(); 'free' }} catch {{ 'held' }}"
                out = subprocess.run(['pwsh', '-NoProfile', '-Command', script], capture_output=True, text=True, timeout=45)
                self.assertEqual(out.stdout.strip(), 'held')
        finally:
            slot.release()
        self.assertFalse(build_lock.HandleSlot(pool / '1.lock').busy())

    @unittest.skipUnless(shutil.which('pwsh'), 'PowerShell 7')
    def test_build_ps1_lease_waits_for_the_stages_and_names_them_instead_of_failing(self):
        driver = self.root / 'lease.ps1'
        driver.write_text("""param([string]$Locks)
$ErrorActionPreference = 'Stop'
. $env:CHECK_STREAMS
Initialize-CheckLocks $Locks
$CheckBuildDeadline = 1
$CheckStageWarning = 1
$held = Enter-CheckBuildLock -RespectPriority
try { 'acquired' } finally { Exit-CheckLocks $held -NoPause }
""")
        env = dict(os.environ, CHECK_STREAMS=str(pathlib.Path(__file__).resolve().parents[1] / 'rust/release/check_streams.ps1'))
        stop = self.root / 'stage.stop'
        stage = self.hold('cargo', 'stage', seconds=90, stop=stop)
        child = subprocess.Popen(['pwsh', '-NoProfile', '-File', str(driver), '-Locks', str(self.root)],
                                 stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env)
        lines_q = queue.Queue()
        def pump():
            assert child.stdout is not None
            for line in child.stdout:
                lines_q.put(line)
            lines_q.put(None)
        reader = threading.Thread(target=pump, daemon=True)
        reader.start()
        collected = []
        try:
            deadline = time.monotonic() + 60
            saw_waiting = False
            while not saw_waiting:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    self.fail(f'timed out waiting for dev check warning; output so far:\n{"".join(collected)}')
                try:
                    item = lines_q.get(timeout=remaining)
                except queue.Empty:
                    self.fail(f'timed out waiting for dev check warning; output so far:\n{"".join(collected)}')
                if item is None:
                    self.fail(f'pwsh exited before emitting waiting message; output:\n{"".join(collected)}')
                collected.append(item)
                if 'still waiting for the dev check' in item:
                    saw_waiting = True
            stop.touch()
            self.assertEqual(stage.wait(timeout=30), 0)
            finish_deadline = time.monotonic() + 45
            while True:
                remaining = finish_deadline - time.monotonic()
                if remaining <= 0:
                    self.fail(f'timed out waiting for pwsh to finish; output so far:\n{"".join(collected)}')
                try:
                    item = lines_q.get(timeout=remaining)
                except queue.Empty:
                    self.fail(f'timed out waiting for pwsh to finish; output so far:\n{"".join(collected)}')
                if item is None:
                    break
                collected.append(item)
            returncode = child.wait(timeout=15)
        finally:
            stop.touch()
            if child.poll() is None:
                kill_tree(child)
            if stage.poll() is None:
                kill_tree(stage)
            reader.join(timeout=5)
            if child.stdout is not None:
                child.stdout.close()
        output = ''.join(collected)
        self.assertEqual(returncode, 0, output)
        self.assertIn('still waiting for the dev check', output)
        self.assertIn('acquired', output)
        self.assertLess(output.index('still waiting for the dev check'), output.index('acquired'))
        self.assertFalse((self.root / 'BUILD-LOCK').exists())

    def test_a_manual_gradle_lock_of_a_reused_pid_does_not_count(self):
        # A crashed `build_lock.py --lock-directory <gradle lane>` from before start= was written:
        # its pid now belongs to a process (this one) that started after the file was written.
        lane = self.root / 'gradle-lane'
        lane.mkdir()
        lock = lane / 'BUILD-LOCK'
        lock.write_text(f'build 2026-11-18 pid={os.getpid()} token=t')
        os.utime(lock, (1, 1))
        self.assertFalse(build_lock.FileSlot(lane).busy())
        os.utime(lock, None)
        self.assertTrue(build_lock.FileSlot(lane).busy())

    def test_the_exclusive_lock_names_its_owners_start_as_powershell_does(self):
        with build_lock.BuildLock(self.root):
            text = (self.root / 'BUILD-LOCK').read_text()
        if os.name == 'nt':
            self.assertRegex(text, r' start=\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{7}Z ')
            self.assertTrue(build_lock.priority_alive(text))

    def test_an_interrupted_stage_gives_back_what_it_took(self):
        def interrupt():
            raise KeyboardInterrupt
        groups = build_lock.LANE_GROUPS['cargo'](self.root) + [('broken', interrupt)]
        with self.assertRaises(KeyboardInterrupt):
            with build_lock.StageLease(groups, self.root, 'test', 1, poll=.05, say=lambda *a, **k: None):
                pass
        self.assertFalse(build_lock.HandleSlot(self.root / 'lanes/cargo-1.lock').busy())
        with self.lease('cargo'):
            pass

    def test_a_stage_lists_the_gradle_lane_as_held_by_its_tree(self):
        with self.lease('gradle'):
            self.assertIn(str(self.root.resolve() / 'gradle-lane' / 'BUILD-LOCK'), os.environ[build_lock.LANES_ENV])
        self.assertNotIn(build_lock.LANES_ENV, os.environ)

    def test_missing_default_directory_falls_back_to_localappdata_while_preserving_explicit_default_pool(self):
        local = self.root / 'local'
        with mock.patch.object(build_lock, 'DEFAULT_DIRECTORY', self.root / 'missing-locks'), \
                mock.patch.dict(os.environ, {'LOCALAPPDATA': str(local)}):
            expected = (local / 'ButterpolloRust' / 'locks').resolve()
            if os.name == 'nt':
                self.assertEqual(build_lock.default_directory(), local / 'ButterpolloRust' / 'locks')
                self.assertEqual(build_lock.lock_directory(), expected)
                self.assertEqual(build_lock.pool_directory(), expected / 'target-pool')
                self.assertEqual(build_lock.gradle_lane(), expected / 'gradle-lane')
            self.assertEqual(build_lock.pool_directory(build_lock.DEFAULT_DIRECTORY), build_lock.DEFAULT_POOL)

    def test_concurrent_stage_leases_keep_lanes_held_thread_safe_and_preserve_external_lanes(self):
        os.environ[build_lock.CARGO_SLOTS_ENV] = '2'
        os.environ[build_lock.LANES_ENV] = 'external-parent-lane'
        errors = []

        def worker(idx):
            lane_name = ('cargo', 'gradle', 'web')[idx % 3]
            for _ in range(4):
                with self.lease(lane_name, timeout=5) as lease:
                    held = build_lock.held_lanes()
                    if 'external-parent-lane' not in held:
                        errors.append(f'lost external lane: {held}')
                    for slot in lease.taken.values():
                        if str(slot.path) not in held:
                            errors.append(f'missing own lane {slot.path} in {held}')
                    time.sleep(0.02)

        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            futures = [pool.submit(worker, i) for i in range(4)]
            for future in futures:
                future.result()
        self.assertEqual(errors, [])
        self.assertEqual(os.environ.get(build_lock.LANES_ENV), 'external-parent-lane')

        os.environ.pop(build_lock.LANES_ENV, None)
        errors.clear()
        def clean_worker(lane_name):
            with self.lease(lane_name, timeout=5) as lease:
                held = build_lock.held_lanes()
                for slot in lease.taken.values():
                    if str(slot.path) not in held:
                        errors.append(f'missing {slot.path}')
                time.sleep(0.02)
        with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
            for f in [pool.submit(clean_worker, n) for n in ('cargo', 'gradle', 'web')]:
                f.result()
        self.assertEqual(errors, [])
        self.assertNotIn(build_lock.LANES_ENV, os.environ)

    def test_environ_copy_is_thread_safe_under_concurrent_lane_transitions(self):
        errors = []
        stop = [False]

        def copy_worker():
            try:
                while not stop[0]:
                    snap = build_lock.environ_copy()
                    self.assertIsInstance(snap, dict)
            except Exception as exc:
                errors.append(repr(exc))

        def lease_worker(lane_name):
            for _ in range(10):
                with self.lease(lane_name, timeout=5):
                    time.sleep(0.005)

        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            copier = pool.submit(copy_worker)
            leasers = [pool.submit(lease_worker, n) for n in ('cargo', 'gradle', 'web')]
            for f in leasers:
                f.result()
            stop[0] = True
            copier.result()
        self.assertEqual(errors, [])

if __name__ == '__main__':
    unittest.main()

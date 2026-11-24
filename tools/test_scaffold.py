"""Tests for tools/scaffold.py and .agents/skills/ integrity."""

import contextlib
import io
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import arch_check  # noqa: E402
import scaffold  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[1]
EXPECTED_SKILLS = (
    'add-setting',
    'add-control-message',
    'add-native-endpoint',
    'check-nvenc',
    'isolated-e2e',
    'release-prep',
)


def run_cli(argv):
    stdout = io.StringIO()
    stderr = io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        rc = scaffold.main(argv)
    return rc, stdout.getvalue(), stderr.getvalue()


class ScaffoldTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)

    def _assert_rust_conventions(self, path):
        raw = path.read_bytes()
        self.assertNotIn(b'\r\n', raw, f'{path} must use LF line endings')
        text = raw.decode('utf-8')
        self.assertTrue(arch_check.has_rust_module_header(text), f'{path} missing //! header')
        self.assertLessEqual(arch_check.count_loc(text), arch_check.DEFAULT_MAX_LOC, f'{path} exceeds 800 LOC')

    def test_control_message_dry_run_collision_and_compile(self):
        # Collision check against real repo's ids.rs (0x5508 is clipboard sync)
        rc, _, err = run_cli(['--root', str(ROOT), 'control-message', 'dup_msg', '--id', '0x5508', '--dry-run'])
        self.assertEqual(rc, 1)
        self.assertIn('already registered', err)

        # Dry run writes nothing
        rc, out, err = run_cli([
            '--root', str(self.root), 'control-message', 'sample_ping',
            '--id', '0x5599', '--launch-param', 'rlSamplePing', '--dry-run',
        ])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('[dry-run]', out)
        target = self.root / 'client/protocol/core/src/sample_ping.rs'
        self.assertFalse(target.exists())

        # Real write creates valid Rust module
        rc, out, err = run_cli([
            '--root', str(self.root), 'control-message', 'sample_ping',
            '--id', '0x5599', '--launch-param', 'rlSamplePing',
        ])
        self.assertEqual(rc, 0, out + err)
        self.assertTrue(target.is_file())
        self._assert_rust_conventions(target)
        text = target.read_text(encoding='utf-8')
        self.assertIn('pub const SAMPLE_PING_MESSAGE_TYPE: u16 = 0x5599;', text)
        self.assertIn('pub const LAUNCH_PARAMETER: &str = "rlSamplePing";', text)

        # Refuses overwrite without --force, allows with --force
        rc, _, err = run_cli(['--root', str(self.root), 'control-message', 'sample_ping', '--id', '0x5599'])
        self.assertEqual(rc, 1)
        self.assertIn('refusing to overwrite', err)
        rc, _, err = run_cli(['--root', str(self.root), 'control-message', 'sample_ping', '--id', '0x5599', '--force'])
        self.assertEqual(rc, 0, err)

        # Compile and run generated unit tests if rustc is installed
        rustc = shutil.which('rustc') or shutil.which('rustc', path=str(pathlib.Path.home() / '.cargo/bin'))
        if rustc:
            bin_path = self.root / ('sample_ping_test.exe' if sys.platform == 'win32' else 'sample_ping_test')
            comp = subprocess.run(
                [rustc, '--edition=2021', '--test', str(target), '-o', str(bin_path)],
                capture_output=True,
                text=True,
            )
            self.assertEqual(comp.returncode, 0, comp.stderr)
            run = subprocess.run([str(bin_path)], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)

    def test_native_endpoint_get_and_post(self):
        # GET dry-run
        rc, out, err = run_cli(['--root', str(self.root), 'native-endpoint', 'display_info', '--dry-run'])
        self.assertEqual(rc, 0, out + err)
        self.assertEqual(list(self.root.rglob('*')), [])

        # POST real write
        rc, out, err = run_cli([
            '--root', str(self.root), 'native-endpoint', 'audio_boost',
            '--method', 'post', '--permission', 'host_control',
        ])
        self.assertEqual(rc, 0, out + err)
        expected_files = (
            'client/protocol/native-api/src/audio_boost.rs',
            'client/protocol/native-api/tests/fixtures/audio_boost.json',
            'rust/host/web/src/native_api/audio_boost.rs',
            'tests/host/tests/nvhttp_parity/native_tests/audio_boost.rs',
        )
        for rel in expected_files:
            p = self.root / rel
            self.assertTrue(p.is_file(), f'missing {rel}')
            self.assertNotIn(b'\r\n', p.read_bytes())
            if rel.endswith('.rs'):
                self._assert_rust_conventions(p)
        wire = (self.root / expected_files[0]).read_text(encoding='utf-8')
        self.assertIn('#[serde(deny_unknown_fields)]', wire)
        self.assertIn('#[serde(other)]', wire)
        fixture = json.loads((self.root / expected_files[1]).read_text(encoding='utf-8'))
        self.assertEqual(fixture['state'], 'ready')
        handler = (self.root / expected_files[2]).read_text(encoding='utf-8')
        self.assertIn('require_host_control(&client)?;', handler)
        self.assertIn('read_body(request).await?', handler)
        self.assertIn('ErrorCode::InvalidRequest', handler)
        self.assertNotIn('parse_body', handler)
        parity = (self.root / expected_files[3]).read_text(encoding='utf-8')
        self.assertIn('s.host_control(true);', parity)
        self.assertIn('send(&s.f, true, &s.connection, "POST", &format!("{PREFIX}/audio-boost")', parity)
        self.assertIn('.assert_error(403, "host_control_required");', parity)
        self.assertNotIn('post_json', parity)
        self.assertIn('.route(&path("/audio-boost"), post(audio_boost::write).fallback(unknown))', out)

    def test_setting_types_validation_and_note(self):
        # Bool + note dry-run with valid CategoryId mapped to basics.ts
        rc, out, err = run_cli([
            '--root', str(self.root), 'setting', 'fast_resume',
            '--type', 'bool', '--default', 'true', '--category', 'general',
            '--label', 'Fast resume', '--description', 'Resumes streams without resetting capture.',
            '--with-note', '--dry-run',
        ])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('pub fn fast_resume(&self) -> bool', out)
        self.assertIn('rust/web/src/lib/schema/basics.ts', out)
        self.assertIn('category: "general"', out)
        self.assertFalse((self.root / 'rust/release/notes.d/fast_resume.md').exists())

        # Choice + real note write with frame-limiting -> display.ts and --extension -> extensions.ts
        rc, out, err = run_cli([
            '--root', str(self.root), 'setting', 'capture_mode',
            '--type', 'choice', '--choices', 'auto,wgc,dd', '--default', 'auto',
            '--category', 'frame-limiting',
            '--label', 'Capture mode', '--description', 'Selects capture API.',
            '--with-note',
        ])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('Choice(&["auto", "wgc", "dd"])', out)
        self.assertIn('rust/web/src/lib/schema/display.ts', out)
        self.assertIn('category: "frame-limiting"', out)
        note = self.root / 'rust/release/notes.d/capture_mode.md'
        self.assertTrue(note.is_file())
        self.assertNotIn(b'\r\n', note.read_bytes())

        rc, out, err = run_cli([
            '--root', str(self.root), 'setting', 'ext_flag',
            '--type', 'bool', '--default', 'false', '--category', 'input', '--extension',
            '--label', 'Ext flag', '--description', 'Extension option.', '--dry-run',
        ])
        self.assertEqual(rc, 0, out + err)
        self.assertIn('rust/web/src/lib/schema/extensions.ts', out)
        self.assertIn('category: "input"', out)

        # Validation failures (including invalid CategoryId like schema filenames 'basics'/'extensions')
        with self.assertRaises(ValueError):
            scaffold.generate_setting_stub('x', 'bool', 'true', 'X', 'Y', category='basics')
        with self.assertRaises(ValueError):
            scaffold.generate_setting_stub('x', 'bool', 'true', 'X', 'Y', category='extensions')
        rc, _, _ = run_cli([
            '--root', str(self.root), 'setting', 'BadKey',
            '--type', 'bool', '--default', 'true', '--label', 'X', '--description', 'Y',
        ])
        self.assertEqual(rc, 1)
        rc, _, _ = run_cli([
            '--root', str(self.root), 'setting', 'bad_int',
            '--type', 'int', '--default', '500', '--min', '1', '--max', '100', '--label', 'X', '--description', 'Y',
        ])
        self.assertEqual(rc, 1)

    def test_test_scenario_all_suites(self):
        suites = {
            'reconnect': 'tests/host/tests/reconnect_tests/my_reconnect.rs',
            'native': 'tests/host/tests/nvhttp_parity/native_tests/my_native.rs',
            'config': 'rust/core/config/src/config/tests/my_config.rs',
            'interop': 'tests/interop/tests/my_interop.rs',
        }
        for suite, rel in suites.items():
            name = pathlib.PurePosixPath(rel).stem
            rc, out, err = run_cli(['--root', str(self.root), 'test-scenario', name, '--suite', suite])
            self.assertEqual(rc, 0, out + err)
            p = self.root / rel
            self.assertTrue(p.is_file(), f'missing {rel}')
            self._assert_rust_conventions(p)


def expand_braces(ref):
    m = re.search(r'\{([^{}]+)\}', ref)
    if not m:
        return [ref]
    head, tail = ref[:m.start()], ref[m.end():]
    expanded = []
    for part in m.group(1).split(','):
        expanded.extend(expand_braces(head + part.strip() + tail))
    return expanded


class SkillsIntegrityTests(unittest.TestCase):
    def test_all_skills_exist_with_valid_frontmatter_lf_and_grounded_paths(self):
        skills_dir = ROOT / '.agents/skills'
        path_re = re.compile(r'`((?:rust|client|docs|tools|tests|\.agents)/[\w./{},-]+)`')
        for name in EXPECTED_SKILLS:
            path = skills_dir / name / 'SKILL.md'
            self.assertTrue(path.is_file(), f'missing skill {path}')
            raw = path.read_bytes()
            self.assertNotIn(b'\r\n', raw, f'{path} must use LF line endings')
            text = raw.decode('utf-8')
            self.assertLessEqual(len(text.splitlines()), 150, f'{path} should stay concise (<=150 LOC)')
            m = re.match(r'^---\n(.*?)\n---\n', text, re.S)
            self.assertIsNotNone(m, f'{path} missing YAML frontmatter')
            fm = m.group(1)
            self.assertIn(f'name: {name}', fm)
            self.assertIn('description:', fm)
            # Expand {a,b} brace groups and verify every concrete cited repo path exists!
            for ref in path_re.findall(text):
                if any(tok in ref for tok in ('*', '<', '>', 'spec.json')):
                    continue
                for candidate in expand_braces(ref):
                    target = ROOT / candidate.rstrip('/')
                    self.assertTrue(target.exists(), f'{path}: cited path does not exist: {candidate} (from {ref})')

        # Spot-check isolated-e2e CLI invocations match real scripts
        e2e_skill = (skills_dir / 'isolated-e2e/SKILL.md').read_text(encoding='utf-8')
        self.assertIn('python rust/release/protocol.py --package <pkg-dir> --work <work-dir>', e2e_skill)
        self.assertIn('python rust/release/timing_report.py <frames.csv> --host-record <host-record>', e2e_skill)


if __name__ == '__main__':
    unittest.main()

import contextlib, io, json, pathlib, shlex, sys, tomllib, unittest
from unittest import mock

import yaml

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import ci  # noqa: E402


class Filters(unittest.TestCase):
    def test_glob_syntax_matches_githubs(self):
        self.assertTrue(ci.glob_regex('rust/**').match('rust/core/src/lib.rs'))
        self.assertTrue(ci.glob_regex('Cargo.*').match('Cargo.lock'))
        self.assertFalse(ci.glob_regex('Cargo.*').match('rust/Cargo.lock'))
        self.assertTrue(ci.glob_regex('!rust/**/*.md'.lstrip('!')).match('rust/core/README.md'))
        self.assertTrue(ci.glob_regex('wip/**').match('wip/faster-checks'))
        self.assertFalse(ci.glob_regex('wip/**').match('main'))

    def test_wip_branches_run_what_main_runs_and_docs_only_runs_nothing(self):
        for key in ci.WORKFLOWS:
            branches, _ = ci.path_filter(ci.WORKFLOWS[key])
            self.assertEqual(branches[0], 'main', key)
            self.assertTrue(ci.branch_matches(branches, 'wip/x'), key)
            self.assertFalse(ci.branch_matches(branches, 'RamazanKara/nj-redesign'), key)
        self.assertEqual(ci.expected('wip/x', ['docs/building.md', 'rust/RELEASE_NOTES.md', 'client/docs/settings.md']), [])
        self.assertEqual(ci.expected('wip/x', ['rust/core/src/lib.rs']), ['host', 'tools'])
        self.assertEqual(ci.expected('wip/x', ['client/protocol/core/src/lib.rs']), ['host', 'android', 'macos', 'tools'])
        self.assertEqual(ci.expected('wip/x', ['client/app/src/main/java/A.java']), ['android', 'tools'])
        self.assertEqual(ci.expected('wip/x', ['tools/dev.py']), ['tools'])
        self.assertEqual(ci.expected('feature/x', ['rust/core/src/lib.rs']), [])

    def test_main_keeps_its_triggers(self):
        # The lane adds wip/**; main, tags and the path filters stay as they were.
        branches, paths = ci.path_filter('rust-windows.yml')
        self.assertEqual(branches, ['main', 'wip/**'])
        self.assertIn('!rust/**/*.md', paths)
        self.assertIn('!client/docs/**', ci.path_filter('nightjar-android.yml')[1])


class Watch(unittest.TestCase):
    def runs(self, *items):
        return json.dumps([dict(databaseId=i, workflowName=name, headSha='abc', status=status, conclusion=conclusion,
                                event=event, url=f'https://x/{i}', createdAt='2026-11-18T00:00:00Z',
                                updatedAt='2026-11-18T00:10:00Z') for i, (name, status, conclusion, event) in enumerate(items)])

    def main(self, gh_outputs, *argv):
        calls = []

        def gh(*args, check=True):
            calls.append(args)
            if args[0] == 'run' and args[1] == 'list':
                return gh_outputs.pop(0) if len(gh_outputs) > 1 else gh_outputs[0]
            if args[0] == 'run' and args[1] == 'view':
                return json.dumps(dict(jobs=[dict(name='lint', conclusion='failure'), dict(name='test', conclusion='success')]))
            return ''
        out = io.StringIO()
        with mock.patch.object(ci, 'gh', side_effect=gh), mock.patch.object(ci, 'remote_head', return_value='abc'), \
                mock.patch.object(ci, 'branch_files', getattr(ci.branch_files, 'return_value', None) and ci.branch_files
                                  or mock.Mock(return_value=['rust/core/src/lib.rs'])), \
                mock.patch.object(ci.time, 'sleep'), contextlib.redirect_stdout(out):
            code = ci.main(['wip/x', '--poll', '0', *(['--settle', '0'] if '--settle' not in argv else []), *argv])
        return code, out.getvalue(), calls

    def test_passed_runs_exit_zero(self):
        code, out, _ = self.main([self.runs(('Rubylight Windows', 'in_progress', '', 'push'),
                                            ('Developer workflow checks', 'completed', 'success', 'push')),
                                  self.runs(('Rubylight Windows', 'completed', 'success', 'push'),
                                            ('Developer workflow checks', 'completed', 'success', 'push'))])
        self.assertEqual(code, 0, out)
        self.assertIn('Rubylight Windows            success', out)

    def test_a_failed_run_names_its_jobs(self):
        code, out, _ = self.main([self.runs(('Rubylight Windows', 'completed', 'failure', 'push'))])
        self.assertEqual(code, 1)
        self.assertIn('failed job: lint', out)
        self.assertNotIn('failed job: test', out)

    def test_no_run_says_how_to_start_one(self):
        code, out, _ = self.main(['[]'])
        self.assertEqual(code, 2)
        self.assertIn('--dispatch host', out)

    def test_dispatch_starts_the_workflows_and_watches_only_the_new_runs(self):
        earlier = ('Nightjar Android', 'completed', 'failure', 'workflow_dispatch')
        code, out, calls = self.main([self.runs(earlier),  # before dispatching: an earlier dispatch
                                      self.runs(earlier),  # the new run has not appeared yet
                                      self.runs(earlier, ('Nightjar Android', 'completed', 'success', 'workflow_dispatch'))],
                                     '--dispatch', 'android', '--settle', '5')
        self.assertIn(('workflow', 'run', 'nightjar-android.yml', '--ref', 'wip/x'), calls)
        self.assertEqual(code, 0, out)
        self.assertNotIn('failure', out)

    def test_an_expected_workflow_that_did_not_run_is_no_pass(self):
        with mock.patch.object(ci, 'branch_files', return_value=['rust/core/src/lib.rs', 'tools/dev.py']):
            code, out, _ = self.main([self.runs(('Developer workflow checks', 'completed', 'success', 'push'))])
        self.assertEqual(code, 2, out)
        self.assertIn('Not started: Rubylight Windows', out)

    def test_an_unpushed_branch_pushes_nothing(self):
        out = io.StringIO()
        with mock.patch.object(ci, 'remote_head', return_value=None), mock.patch.object(ci, 'gh') as gh, \
                contextlib.redirect_stdout(out):
            self.assertEqual(ci.main(['wip/none']), 2)
        gh.assert_not_called()
        self.assertIn('push it first', out.getvalue())


def _split_shell_clauses(script):
    text = script.replace('\r\n', '\n').replace('\\\n', ' ')
    clauses, buf = [], []
    quote, escape, i, n = None, False, 0, len(text)
    while i < n:
        ch = text[i]
        if escape:
            buf.append(ch)
            escape = False
            i += 1
            continue
        if quote:
            if ch == '\\' and quote == '"':
                escape = True
            elif ch == quote:
                quote = None
            buf.append(ch)
            i += 1
            continue
        if ch in ("'", '"'):
            quote = ch
            buf.append(ch)
            i += 1
        elif ch == '\\':
            escape = True
            buf.append(ch)
            i += 1
        elif ch == '#' and (not buf or buf[-1].isspace()):
            while i < n and text[i] != '\n':
                i += 1
        elif ch in ('\n', ';'):
            clauses.append(''.join(buf))
            buf.clear()
            i += 1
        elif text[i:i + 2] in ('&&', '||'):
            clauses.append(''.join(buf))
            buf.clear()
            i += 2
        else:
            buf.append(ch)
            i += 1
    if buf:
        clauses.append(''.join(buf))
    return clauses


def _has_doctestable_lib(pkg_dir, pkg_toml):
    lib = pkg_toml.get('lib') or {}
    if lib.get('doctest') is False:
        return False
    crate_types = lib.get('crate-type') or []
    if crate_types and all(ct in ('cdylib', 'staticlib') for ct in crate_types):
        return False
    lib_path = pkg_dir / (lib.get('path') or 'src/lib.rs')
    return lib_path.is_file()


def _cargo_manifest_packages(manifest_path):
    manifest = tomllib.loads(manifest_path.read_text(encoding='utf-8'))
    base = manifest_path.parent
    ws = manifest.get('workspace')
    if ws is None:
        pkg = manifest.get('package', {})
        name = pkg.get('name')
        return ({name: _has_doctestable_lib(base, manifest)} if name else {}), ([name] if name else [])
    members = ws.get('members') or []
    member_dirs = []
    for pattern in members:
        if any(ch in pattern for ch in ('*', '?', '[')):
            member_dirs.extend(sorted(p for p in base.glob(pattern) if (p / 'Cargo.toml').is_file()))
        else:
            member_dirs.append(base / pattern)
    packages = {}
    dir_to_name = {}
    for directory in member_dirs:
        cargo_toml = directory / 'Cargo.toml'
        if not cargo_toml.is_file():
            continue
        pkg = tomllib.loads(cargo_toml.read_text(encoding='utf-8'))
        name = pkg.get('package', {}).get('name')
        if name:
            packages[name] = _has_doctestable_lib(directory, pkg)
            dir_to_name[directory.resolve()] = name
    default_members = ws.get('default-members')
    if default_members is not None:
        defaults = [dir_to_name[(base / m).resolve()] for m in default_members if (base / m).resolve() in dir_to_name]
    else:
        defaults = list(packages)
    return packages, defaults


def _find_manifest(cwd, manifest_arg):
    if manifest_arg:
        return (cwd / manifest_arg).resolve()
    current = cwd.resolve()
    while True:
        candidate = current / 'Cargo.toml'
        if candidate.is_file():
            return candidate
        if current.parent == current:
            raise FileNotFoundError(f'No Cargo.toml found from {cwd}')
        current = current.parent


def _workflow_doctest_violations(workflow_data, workflow_name, root):
    violations = []
    wf_cwd = (workflow_data.get('defaults') or {}).get('run', {}).get('working-directory', '')
    for job_name, job in (workflow_data.get('jobs') or {}).items():
        if not isinstance(job, dict):
            continue
        job_cwd = (job.get('defaults') or {}).get('run', {}).get('working-directory', wf_cwd)
        for idx, step in enumerate(job.get('steps') or [], start=1):
            if not isinstance(step, dict) or not isinstance(step.get('run'), str):
                continue
            step_cwd = step.get('working-directory', job_cwd)
            cwd = (root / step_cwd).resolve() if step_cwd else root.resolve()
            step_label = step.get('name') or f'step #{idx}'
            for clause in _split_shell_clauses(step['run']):
                stripped = clause.strip().lstrip('(').strip()
                if not stripped:
                    continue
                try:
                    tokens = shlex.split(stripped)
                except ValueError:
                    tokens = stripped.split()
                if not tokens:
                    continue
                if tokens[0] == 'cd' and len(tokens) >= 2 and not tokens[1].startswith('-'):
                    target = tokens[1]
                    if not target.startswith(('$', '~')):
                        cwd = (cwd / target).resolve()
                    continue
                if tokens[0] != 'cargo':
                    continue
                sub_idx = 2 if len(tokens) > 1 and tokens[1].startswith('+') else 1
                if len(tokens) <= sub_idx + 1 or tokens[sub_idx] != 'test' or '--doc' not in tokens[sub_idx + 1:]:
                    continue
                manifest_arg, pkg_args, exclude_args, workspace_flag = None, [], [], False
                i = sub_idx + 1
                while i < len(tokens):
                    tok = tokens[i]
                    if tok == '--':
                        break
                    if tok == '--manifest-path' and i + 1 < len(tokens):
                        manifest_arg = tokens[i + 1]
                        i += 2
                        continue
                    if tok.startswith('--manifest-path='):
                        manifest_arg = tok.split('=', 1)[1]
                    elif tok in ('-p', '--package') and i + 1 < len(tokens):
                        pkg_args.append(tokens[i + 1])
                        i += 2
                        continue
                    elif tok.startswith(('--package=', '-p=')):
                        pkg_args.append(tok.split('=', 1)[1])
                    elif tok == '--exclude' and i + 1 < len(tokens):
                        exclude_args.append(tokens[i + 1])
                        i += 2
                        continue
                    elif tok.startswith('--exclude='):
                        exclude_args.append(tok.split('=', 1)[1])
                    elif tok in ('--workspace', '--all'):
                        workspace_flag = True
                    i += 1
                manifest_path = _find_manifest(cwd, manifest_arg)
                packages, defaults = _cargo_manifest_packages(manifest_path)
                if pkg_args:
                    targets = pkg_args
                elif workspace_flag:
                    targets = [p for p in packages if p not in exclude_args]
                else:
                    targets = [p for p in defaults if p not in exclude_args]
                if targets and not any(packages.get(t, False) for t in targets):
                    violations.append(
                        f'{workflow_name} job={job_name} step="{step_label}": '
                        f'cargo test --doc targets bin-only/non-doctest crate(s) {targets} ({stripped})'
                    )
    return violations


class WorkflowDoctests(unittest.TestCase):
    def test_no_workflow_runs_cargo_doc_on_bin_only_crates(self):
        workflows_dir = ci.ROOT / '.github' / 'workflows'
        violations = []
        for path in sorted(workflows_dir.glob('*.yml')):
            data = yaml.safe_load(path.read_text(encoding='utf-8')) or {}
            violations.extend(_workflow_doctest_violations(data, path.name, ci.ROOT))
        self.assertEqual(violations, [], '\n'.join(violations))

    def test_bin_only_doctest_detector_catches_desktop_only_and_allows_mixed_lib_targets(self):
        bad_wf = yaml.safe_load("""
defaults:
  run:
    working-directory: client
jobs:
  desktop:
    steps:
      - name: Test and lint the desktop client
        run: |
          cd protocol
          cargo test --doc -p nightjar-desktop --locked
""")
        bad = _workflow_doctest_violations(bad_wf, 'synthetic.yml', ci.ROOT)
        self.assertEqual(len(bad), 1, bad)
        self.assertIn('nightjar-desktop', bad[0])

        ok_wf = yaml.safe_load("""
jobs:
  desktop:
    defaults:
      run:
        working-directory: client/protocol
    steps:
      - name: Mixed lib and bin targets plus lib manifest
        run: |
          cargo test --doc --locked -p nightjar-desktop -p nightjar-core -p nightjar-policy
          cargo test --doc --locked --manifest-path core/Cargo.toml
""")
        self.assertEqual(_workflow_doctest_violations(ok_wf, 'synthetic-ok.yml', ci.ROOT), [])


class NightjarPlayWorkflow(unittest.TestCase):
    def test_release_calls_play_workflow_for_ci_published_non_prereleases(self):
        client_wf = ci.ROOT / 'client' / '.github' / 'workflows'
        play = yaml.safe_load((client_wf / 'play.yml').read_text(encoding='utf-8'))
        release = yaml.safe_load((client_wf / 'release.yml').read_text(encoding='utf-8'))

        # PyYAML 1.1 parses bare `on:` as boolean True.
        play_on = play.get('on') or play.get(True)
        self.assertIn('workflow_call', play_on)
        call_inputs = play_on['workflow_call']['inputs']
        self.assertEqual(set(call_inputs), {'tag', 'check_only', 'track'})
        self.assertTrue(call_inputs['tag']['required'])
        upload_if = play['jobs']['upload']['if']
        self.assertIn("github.event_name == 'workflow_call'", upload_if)
        self.assertIn("'-rc.'", upload_if)

        release_job = release['jobs']['release']
        self.assertIn('published', release_job['outputs'])
        play_job = release['jobs']['play']
        self.assertEqual(play_job['uses'], './.github/workflows/play.yml')
        self.assertEqual(play_job['secrets'], 'inherit')
        self.assertEqual(play_job['needs'], ['preflight', 'release'])
        self.assertIn("needs.release.outputs.published == 'true'", play_job['if'])
        self.assertIn("!contains(needs.preflight.outputs.version, '-')", play_job['if'])


if __name__ == '__main__':
    unittest.main()

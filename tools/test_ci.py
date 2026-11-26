import contextlib, http.server, io, json, os, pathlib, shlex, shutil, subprocess, sys, tempfile, threading, tomllib, unittest
from unittest import mock

import yaml

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import ci  # noqa: E402


class Filters(unittest.TestCase):
    def test_glob_syntax_matches_githubs(self):
        self.assertTrue(ci.glob_regex('rust/**').match('rust/core/session/src/lib.rs'))
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
        self.assertEqual(ci.expected('wip/x', ['rust/core/session/src/lib.rs']), ['host', 'tools'])
        self.assertEqual(ci.expected('wip/x', ['client/protocol/core/src/lib.rs']), ['host', 'android', 'macos', 'tools'])
        self.assertEqual(ci.expected('wip/x', ['client/app/src/main/java/A.java']), ['android', 'tools'])
        self.assertEqual(ci.expected('wip/x', ['tools/dev.py']), ['tools'])
        self.assertEqual(ci.expected('feature/x', ['rust/core/session/src/lib.rs']), [])

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
                                  or mock.Mock(return_value=['rust/core/session/src/lib.rs'])), \
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
        with mock.patch.object(ci, 'branch_files', return_value=['rust/core/session/src/lib.rs', 'tools/dev.py']):
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
        self.assertNotIn("github.event_name == 'workflow_call'", upload_if)
        self.assertIn("github.workflow == 'Google Play'", upload_if)
        self.assertIn("!github.event.release.prerelease", upload_if)
        self.assertIn("!contains(inputs.tag, '-')", upload_if)
        self.assertIn("'-rc.'", upload_if)

        release_job = release['jobs']['release']
        self.assertIn('published', release_job['outputs'])
        play_job = release['jobs']['play']
        self.assertEqual(play_job['uses'], './.github/workflows/play.yml')
        self.assertEqual(play_job['secrets'], 'inherit')
        self.assertEqual(play_job['needs'], ['preflight', 'release'])
        self.assertIn("needs.release.outputs.published == 'true'", play_job['if'])
        self.assertIn("!contains(needs.preflight.outputs.version, '-')", play_job['if'])


class MirrorAndReleaseWorkflows(unittest.TestCase):
    def test_mirror_gate_watches_nextest_and_returns_from_function(self):
        mirror = yaml.safe_load((ci.ROOT / '.github/workflows/nightjar-mirror.yml').read_text(encoding='utf-8'))
        gate = next(s['run'] for s in mirror['jobs']['mirror']['steps'] if s.get('id') == 'gate')
        android = gate[gate.index('check nightjar-android.yml'):gate.index('check nightjar-macos.yml')]
        macos = gate[gate.index('check nightjar-macos.yml'):]
        self.assertIn('.config/nextest.toml', android)
        self.assertIn('.config/nextest.toml', macos)
        body = gate[gate.index('check()'):gate.index('check nightjar-android.yml')]
        self.assertNotIn('exit 0', body)
        self.assertIn('return 1', body)

    def test_desktop_media_paths_and_dry_run_signing_environment(self):
        release_text = (ci.ROOT / 'client/.github/workflows/release.yml').read_text(encoding='utf-8')
        agents_text = (ci.ROOT / 'client/AGENTS.md').read_text(encoding='utf-8')
        self.assertNotIn('protocol/desktop/ffmpeg-decode-only.sh', release_text)
        self.assertIn('protocol/desktop-media/ffmpeg-decode-only.sh', release_text)
        self.assertNotIn('protocol/desktop/src/metal/generate.py', agents_text)
        self.assertIn('protocol/desktop-media/src/metal/generate.py', agents_text)
        release = yaml.safe_load(release_text)
        sign_win = release['jobs']['sign-windows']
        self.assertIn("refs/heads/main", sign_win['environment'])
        self.assertIn("refs/tags/v", sign_win['environment'])
        self.assertIn("refs/heads/main", sign_win['env']['SIGN'])

    def test_rc_workflow_deletes_draft_and_tag_on_dispatch_failure_and_keeps_scripts_synced(self):
        rc_wf = yaml.safe_load((ci.ROOT / 'client/.github/workflows/rc.yml').read_text(encoding='utf-8'))
        last_step = rc_wf['jobs']['rc']['steps'][-1]['run']
        self.assertIn('gh release create', last_step)
        self.assertIn('gh workflow run release.yml', last_step)
        self.assertLess(last_step.index('gh workflow run release.yml'), last_step.rindex('refs/tags/$TAG'))
        self.assertEqual(
            (ci.ROOT / '.github/scripts/rc.mjs').read_bytes(),
            (ci.ROOT / 'client/.github/scripts/rc.mjs').read_bytes(),
        )


def _git(cwd, *args):
    env = dict(os.environ, GIT_AUTHOR_NAME='t', GIT_AUTHOR_EMAIL='t@t',
               GIT_COMMITTER_NAME='t', GIT_COMMITTER_EMAIL='t@t',
               GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM='1')
    out = subprocess.run(['git', '-C', str(cwd), *args], env=env, capture_output=True, text=True)
    if out.returncode:
        raise AssertionError(f'git {args}: {out.stderr}')
    return out.stdout.strip()


class NightjarAutomationScripts(unittest.TestCase):
    def test_rc_prune_removes_superseded_failed_draft_and_keeps_full_release(self):
        requests = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                requests.append(('GET', self.path))
                if self.path.startswith('/repos/o/r/releases'):
                    data = [
                        {'id': 1, 'tag_name': 'v6.4.1', 'draft': False, 'prerelease': False, 'created_at': '2026-11-01T00:00:00Z'},
                        {'id': 2, 'tag_name': 'v6.4.2-rc.10', 'draft': False, 'prerelease': True, 'created_at': '2026-11-10T00:00:00Z'},
                        # Failed draft created after rc.10 whose version key sorts below rc.10.
                        {'id': 3, 'tag_name': 'v6.4.2-rc.9', 'draft': True, 'prerelease': True, 'created_at': '2026-11-11T00:00:00Z'},
                    ]
                    raw = json.dumps(data).encode()
                    self.send_response(200)
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(raw)))
                    self.end_headers()
                    self.wfile.write(raw)
                    return
                self.send_response(404)
                self.end_headers()

            def do_DELETE(self):
                requests.append(('DELETE', self.path))
                self.send_response(204)
                self.end_headers()

            def log_message(self, *_args):
                pass

        srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.addCleanup(srv.server_close)
        self.addCleanup(srv.shutdown)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        env = dict(os.environ, GITHUB_API_URL=f'http://127.0.0.1:{srv.server_address[1]}',
                   GITHUB_REPOSITORY='o/r', GITHUB_TOKEN='tok')
        res = subprocess.run(['node', str(ci.ROOT / 'client/.github/scripts/rc.mjs'), 'prune'],
                             capture_output=True, text=True, env=env, timeout=20)
        self.assertEqual(res.returncode, 0, res.stderr)
        deletes = [p for m, p in requests if m == 'DELETE']
        self.assertEqual(deletes, ['/repos/o/r/releases/3', '/repos/o/r/git/refs/tags/v6.4.2-rc.9'])

    def test_device_results_writes_data_branch_never_main_and_comments_on_failure(self):
        comments = []
        patches = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def _json(self, code, payload=None):
                raw = b'' if payload is None else json.dumps(payload).encode()
                self.send_response(code)
                if payload is not None:
                    self.send_header('Content-Type', 'application/json')
                    self.send_header('Content-Length', str(len(raw)))
                self.end_headers()
                if payload is not None:
                    self.wfile.write(raw)

            def do_GET(self):
                if '/comments' in self.path:
                    return self._json(200, comments)
                self._json(404, {})

            def do_POST(self):
                length = int(self.headers.get('Content-Length', '0'))
                body = json.loads(self.rfile.read(length))
                if self.path.endswith('/comments'):
                    entry = {'id': len(comments) + 1, 'user': {'login': 'github-actions[bot]'}, 'body': body['body']}
                    comments.append(entry)
                    return self._json(201, entry)
                self._json(404, {})

            def do_PATCH(self):
                length = int(self.headers.get('Content-Length', '0'))
                body = json.loads(self.rfile.read(length))
                patches.append((self.path, body))
                self._json(200, {})

            def log_message(self, *_args):
                pass

        srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.addCleanup(srv.server_close)
        self.addCleanup(srv.shutdown)
        threading.Thread(target=srv.serve_forever, daemon=True).start()

        with tempfile.TemporaryDirectory() as td:
            root = pathlib.Path(td)
            origin = root / 'origin.git'
            origin.mkdir()
            _git(origin, 'init', '-q', '--bare', '-b', 'main')
            seed = root / 'seed'
            seed.mkdir()
            _git(seed, 'init', '-q', '-b', 'main')
            (seed / 'site/data').mkdir(parents=True)
            (seed / 'docs').mkdir(parents=True)
            shutil.copyfile(ci.ROOT / 'client/site/data/device-results.json', seed / 'site/data/device-results.json')
            shutil.copyfile(ci.ROOT / 'client/docs/devices.md', seed / 'docs/devices.md')
            _git(seed, 'add', '-A')
            _git(seed, 'commit', '-q', '-m', 'seed main')
            _git(seed, 'remote', 'add', 'origin', str(origin))
            _git(seed, 'push', '-q', 'origin', 'main')
            main_before = _git(origin, 'rev-parse', 'refs/heads/main')

            pages = root / 'pages'
            pages.mkdir()
            _git(pages, 'init', '-q', '-b', 'gh-pages')
            (pages / 'index.html').write_text('ok\n', encoding='utf-8')
            _git(pages, 'add', '-A')
            _git(pages, 'commit', '-q', '-m', 'seed pages')
            _git(pages, 'remote', 'add', 'origin', str(origin))
            _git(pages, 'push', '-q', 'origin', 'gh-pages')

            work = root / 'work'
            _git(root, 'clone', '-q', str(origin), 'work')

            sample = json.loads((ci.ROOT / 'client/site/data/device-results.json').read_text(encoding='utf-8'))['entries'][0]['results']
            fresh_results = json.loads(json.dumps(sample))
            fresh_results['model'] = 'Pixel Test Device'
            fresh_results['host'] = 'Synthetic Host'
            fresh_results['decoderBenchmark']['results'][0]['decoder'] = 'c2.test.synth.dec'
            issue_body = '\n'.join([
                '### Device', 'Pixel Test Device',
                '### Results text', '```json', json.dumps(fresh_results), '```',
                '### Stream on this device (optional)', '_No response_',
                '### Publishing', '- [x] OK',
            ])
            event = root / 'event.json'
            event.write_text(json.dumps({
                'issue': {'number': 99, 'state': 'open', 'labels': [{'name': 'device-results'}], 'body': issue_body}
            }), encoding='utf-8')

            env = dict(os.environ,
                       GITHUB_API_URL=f'http://127.0.0.1:{srv.server_address[1]}',
                       GITHUB_SERVER_URL='https://github.com',
                       GITHUB_REPOSITORY='o/r', GITHUB_RUN_ID='777', GITHUB_TOKEN='tok',
                       RUNNER_TEMP=str(root / 'rtmp'))
            (root / 'rtmp').mkdir()
            script = str(ci.ROOT / 'client/.github/scripts/device-results-issue.mjs')
            ok = subprocess.run(['node', script, '--event', str(event), '--today', '2026-11-25'],
                                cwd=str(work), env=env, capture_output=True, text=True, timeout=30)
            self.assertEqual(ok.returncode, 0, ok.stdout + ok.stderr)
            # main stayed untouched while device-results branch & gh-pages were updated.
            self.assertEqual(_git(origin, 'rev-parse', 'refs/heads/main'), main_before)
            data_tip = _git(origin, 'rev-parse', 'refs/heads/device-results')
            self.assertNotEqual(data_tip, main_before)
            self.assertIn('Pixel Test Device', _git(origin, 'show', f'{data_tip}:site/data/device-results.json'))
            self.assertIn('Pixel Test Device', _git(origin, 'show', 'gh-pages:data/device-results.json'))
            self.assertEqual(len(comments), 1)
            self.assertIn('<!-- device-results-bot -->', comments[0]['body'])
            self.assertTrue(any(p[0].endswith('/issues/99') and p[1]['state'] == 'closed' for p in patches))

            # Now test unexpected push/pages failure posts an explanatory failure comment.
            _git(origin, 'branch', '-D', 'gh-pages')
            fail_results = json.loads(json.dumps(fresh_results))
            fail_results['model'] = 'Pixel Fail Case'
            fail_results['decoderBenchmark']['results'][0]['medianDecodeMs'] = 3.7
            fail_body = '\n'.join([
                '### Device', 'Pixel Fail Case',
                '### Results text', '```json', json.dumps(fail_results), '```',
                '### Stream on this device (optional)', '_No response_',
                '### Publishing', '- [x] OK',
            ])
            event2 = root / 'event2.json'
            event2.write_text(json.dumps({
                'issue': {'number': 100, 'state': 'open', 'labels': [{'name': 'device-results'}],
                          'body': fail_body}
            }), encoding='utf-8')
            comments.clear()
            fail = subprocess.run(['node', script, '--event', str(event2), '--today', '2026-11-25'],
                                  cwd=str(work), env=env, capture_output=True, text=True, timeout=30)
            self.assertNotEqual(fail.returncode, 0)
            self.assertEqual(len(comments), 1)
            self.assertIn('<!-- device-results-bot -->', comments[0]['body'])
            self.assertIn('/actions/runs/777', comments[0]['body'])


if __name__ == '__main__':
    unittest.main()

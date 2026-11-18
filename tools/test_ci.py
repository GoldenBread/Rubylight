import contextlib, io, json, pathlib, sys, unittest
from unittest import mock

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
        self.assertEqual(ci.expected('wip/x', ['rust/core/src/lib.rs']), ['host'])
        self.assertEqual(ci.expected('wip/x', ['client/protocol/core/src/lib.rs']), ['host', 'android', 'macos'])
        self.assertEqual(ci.expected('wip/x', ['client/app/src/main/java/A.java']), ['android'])
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
        code, out, _ = self.main([self.runs(('Rubylight Windows', 'in_progress', '', 'push')),
                                  self.runs(('Rubylight Windows', 'completed', 'success', 'push'))])
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


if __name__ == '__main__':
    unittest.main()

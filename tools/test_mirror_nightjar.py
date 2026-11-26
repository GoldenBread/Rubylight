"""Tests for mirror_nightjar.py against a small monorepo built with `git subtree add`."""
import contextlib
import io
import os
import subprocess
import tempfile
import unittest

import mirror_nightjar as m

ENV = {**os.environ, 'GIT_AUTHOR_NAME': 't', 'GIT_AUTHOR_EMAIL': 't@t',
       'GIT_COMMITTER_NAME': 't', 'GIT_COMMITTER_EMAIL': 't@t',
       'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_CONFIG_NOSYSTEM': '1'}


class Repo:
    def __init__(self, path):
        self.path = path
        self.n = 0
        os.makedirs(path)
        self.git('init', '-q', '-b', 'main')

    def git(self, *args):
        self.n += 1
        env = {**ENV, 'GIT_AUTHOR_DATE': f'{1700000000 + self.n} +0000',
               'GIT_COMMITTER_DATE': f'{1700000000 + self.n} +0000'}
        p = subprocess.run(['git', '-C', self.path, *args], env=env,
                           capture_output=True, text=True)
        if p.returncode:
            raise AssertionError(f'git {args}: {p.stderr}')
        return p.stdout.strip()

    def commit(self, rel, text, msg):
        full = os.path.join(self.path, rel)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, 'w', newline='') as f:
            f.write(text)
        self.git('add', '-A')
        self.git('commit', '-q', '-m', msg)
        return self.git('rev-parse', 'HEAD')


class MirrorTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.nj = Repo(os.path.join(self.tmp.name, 'nj'))
        self.nj.commit('app/a.txt', '1', 'nj one')
        self.nj.commit('app/b.txt', '2', 'nj two')
        self.mono = Repo(os.path.join(self.tmp.name, 'mono'))
        self.mono.commit('rust/r.txt', 'r', 'host one')
        self.mono.git('subtree', 'add', '-q', '--prefix', 'client', self.nj.path, 'main')
        self.git = m.Git(self.mono.path)

    def tearDown(self):
        self.tmp.cleanup()

    def published(self):
        self.mono.git('fetch', '-q', self.nj.path, 'main')
        return self.mono.git('rev-parse', 'FETCH_HEAD')

    def check(self, result, published, subjects):
        self.assertTrue(self.git.is_ancestor(published, result))
        self.assertEqual(self.mono.git('rev-parse', f'{result}^{{tree}}'),
                         self.mono.git('rev-parse', 'HEAD:client'))
        log = self.mono.git('log', '--format=%s', f'{published}..{result}').splitlines()
        self.assertEqual(sorted(log), sorted(subjects))

    def test_up_to_date_after_subtree_add(self):
        pub = self.published()
        self.assertEqual(m.split(self.git, 'HEAD', pub), pub)

    def test_host_only_commits_are_dropped(self):
        pub = self.published()
        self.mono.commit('client/app/a.txt', '3', 'client change')
        self.mono.commit('rust/r.txt', 'r2', 'host change')
        result = m.split(self.git, 'HEAD', pub)
        self.check(result, pub, ['client change'])
        # Author, committer and message come across unchanged.
        src = self.mono.git('log', '-1', '--format=%an %ae %ad %cn %ce %cd %B', 'HEAD~1')
        self.assertEqual(self.mono.git('log', '-1', '--format=%an %ae %ad %cn %ce %cd %B', result), src)

    def test_incremental_from_last_push_matches_subtree_split(self):
        self.mono.commit('client/app/a.txt', '3', 'first')
        first = m.split(self.git, 'HEAD', self.published())
        self.nj.git('fetch', '-q', self.mono.path, f'{first}:refs/heads/tmp')
        self.nj.git('merge', '-q', '--ff-only', 'tmp')
        self.mono.commit('client/app/a.txt', '4', 'second')
        pub = self.published()
        self.assertEqual(pub, first)
        result = m.split(self.git, 'HEAD', pub)
        self.check(result, pub, ['second'])
        # The very commits git subtree split makes.
        ref = self.mono.git('subtree', 'split', '-q', '--prefix', 'client', 'HEAD')
        self.assertEqual(result, ref)

    def test_side_branch_commits_are_kept(self):
        pub = self.published()
        self.mono.git('checkout', '-q', '-b', 'side')
        self.mono.commit('client/app/c.txt', 'c', 'side client')
        self.mono.commit('rust/s.txt', 's', 'side host')
        self.mono.git('checkout', '-q', 'main')
        self.mono.commit('client/app/a.txt', '5', 'main client')
        self.mono.git('merge', '-q', '--no-ff', '-m', 'merge side', 'side')
        result = m.split(self.git, 'HEAD', pub)
        self.check(result, pub, ['side client', 'main client', 'merge side'])

    def test_subtree_pull_keeps_nightjar_commits(self):
        self.nj.commit('app/t.txt', 'tr', 'translation')
        self.mono.git('subtree', 'pull', '-q', '--prefix', 'client', self.nj.path, 'main',
                      '-m', 'pull nightjar')
        self.mono.commit('client/app/a.txt', '6', 'after pull')
        pub = self.published()
        result = m.split(self.git, 'HEAD', pub)
        self.check(result, pub, ['after pull'])

    def test_fetch_survives_a_concurrent_fetch_overwriting_fetch_head(self):
        pub = m.fetch_published(self.git, self.nj.path, 'main')
        self.assertEqual(pub, self.nj.git('rev-parse', 'main'))
        # another fetch in the same checkout rewrites FETCH_HEAD
        other = Repo(os.path.join(self.tmp.name, 'other'))
        other.commit('x.txt', 'x', 'unrelated')
        self.mono.git('fetch', '-q', '--no-tags', other.path, 'main')
        self.assertNotEqual(self.mono.git('rev-parse', 'FETCH_HEAD'), pub)
        # the private ref was read before that and is gone again
        self.assertEqual(self.mono.git('for-each-ref', 'refs/mirror/'), '')
        self.assertEqual(m.fetch_published(self.git, self.nj.path, 'main'), pub)

    def test_default_ref_is_fetched_origin_main_not_local_head(self):
        origin = os.path.join(self.tmp.name, 'origin.git')
        subprocess.run(['git', 'init', '-q', '--bare', '-b', 'main', origin], env=ENV, check=True)
        self.mono.git('remote', 'add', 'origin', origin)
        self.mono.git('push', '-q', 'origin', 'main')
        # origin gets a client commit the local checkout does not have
        clone = Repo(os.path.join(self.tmp.name, 'clone'))
        clone.git('remote', 'add', 'origin', origin)
        clone.git('pull', '-q', 'origin', 'main')
        new = clone.commit('client/app/a.txt', 'new', 'client on origin')
        clone.git('push', '-q', 'origin', 'main')
        self.assertNotEqual(self.mono.git('rev-parse', 'HEAD'), new)
        dest = os.path.join(self.tmp.name, 'dest.git')
        subprocess.run(['git', 'clone', '-q', '--bare', self.nj.path, dest], env=ENV, check=True)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = m.main(['--repo', self.mono.path, '--remote', dest, '--dry-run'])
        self.assertEqual(rc, 0)
        self.assertIn(f'monorepo {new}', out.getvalue())
        self.assertIn('fast-forward by 1 commit(s)', out.getvalue())

    def _publish(self, result):
        self.nj.git('fetch', '-q', self.mono.path, f'{result}:refs/heads/tmp')
        self.nj.git('merge', '-q', '--ff-only', 'tmp')
        return self.published()

    def test_reverting_published_commit_fast_forwards_and_keeps_subsequent_commits(self):
        c1 = self.mono.commit('client/app/a.txt', '3', 'publish a3')
        pub = self._publish(m.split(self.git, 'HEAD', self.published()))
        # Revert restores the prior published tree (nj two).
        self.mono.git('revert', '--no-edit', c1)
        reverted = m.split(self.git, 'HEAD', pub)
        self.check(reverted, pub, ['Revert "publish a3"'])
        pub = self._publish(reverted)
        # Subsequent client commit after a host-only commit still builds cleanly on pub.
        self.mono.commit('rust/r.txt', 'r2', 'host note')
        self.mono.commit('client/app/a.txt', '4', 'reapply a4')
        next_split = m.split(self.git, 'HEAD', pub)
        self.check(next_split, pub, ['reapply a4'])
        self.assertEqual(next_split, self.mono.git('subtree', 'split', '-q', '--prefix', 'client', 'HEAD'))

    def test_revert_followed_by_new_commit_before_mirror_keeps_both(self):
        c1 = self.mono.commit('client/app/a.txt', '3', 'publish a3')
        pub = self._publish(m.split(self.git, 'HEAD', self.published()))
        self.mono.git('revert', '--no-edit', c1)
        self.mono.commit('client/app/b.txt', '4', 'after revert')
        result = m.split(self.git, 'HEAD', pub)
        self.check(result, pub, ['Revert "publish a3"', 'after revert'])

    def test_commit_and_revert_before_mirror_matches_subtree_split(self):
        pub = self.published()
        c_unpub = self.mono.commit('client/app/a.txt', '3', 'unmirrored a3')
        self.mono.git('revert', '--no-edit', c_unpub)
        self.mono.commit('client/app/a.txt', '4', 'after reverted unmirrored')
        result = m.split(self.git, 'HEAD', pub)
        self.check(result, pub, ['unmirrored a3', 'Revert "unmirrored a3"', 'after reverted unmirrored'])
        self.assertEqual(result, self.mono.git('subtree', 'split', '-q', '--prefix', 'client', 'HEAD'))

    def test_side_branch_after_published_revert_keeps_true_fork_parent(self):
        self.mono.commit('client/app/a.txt', '3', 'a3')
        self.mono.git('checkout', '-q', '-b', 'side')
        self.mono.commit('client/app/side.txt', 's', 'side change')
        self.mono.git('checkout', '-q', 'main')
        c_pub2 = self.mono.commit('client/app/a.txt', '4', 'a4')
        pub = self._publish(m.split(self.git, 'HEAD', self.published()))
        self.mono.git('revert', '--no-edit', c_pub2)
        pub = self._publish(m.split(self.git, 'HEAD', pub))
        self.mono.git('merge', '-q', '--no-ff', '-m', 'merge side', 'side')
        result = m.split(self.git, 'HEAD', pub)
        self.check(result, pub, ['side change', 'merge side'])
        # Side branch forked from a3, not from the newer revert commit sharing a3's tree.
        nj_a3 = self.mono.git('log', '-1', '--format=%H', '--grep=^a3$', pub)
        self.assertEqual(self.mono.git('rev-parse', f'{result}^2^'), nj_a3)

    def test_unmirrored_commit_plus_subtree_pull_and_revert_to_pulled_tree(self):
        # Publish c_base first, then diverge with an unmirrored commit on mono + direct commit on nj.
        self.mono.commit('client/app/a.txt', '3', 'base a3')
        pub = self._publish(m.split(self.git, 'HEAD', self.published()))
        c_un = self.mono.commit('client/app/u.txt', 'u1', 'unmirrored client')
        self.nj.commit('app/t.txt', 'tr', 'direct nj commit')
        self.mono.git('subtree', 'pull', '-q', '--prefix', 'client', self.nj.path, 'main', '-m', 'pull nj')
        pub = self.published()
        self.mono.git('revert', '--no-edit', c_un)
        result = m.split(self.git, 'HEAD', pub)
        self.check(result, pub, ['unmirrored client', 'pull nj', 'Revert "unmirrored client"'])


if __name__ == '__main__':
    unittest.main()

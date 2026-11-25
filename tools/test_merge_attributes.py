"""Merges of files that conflict for no reason: string tables (union) and generated docs (driver)."""
import contextlib, io, os, pathlib, re, shutil, subprocess, sys, tempfile, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import generated_docs  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[1]
TABLE = 'client/protocol/desktop-config/src/strings/de.rs'
ENTRY = re.compile(r'\(\s*"((?:[^"\\]|\\.)*)",\s*"((?:[^"\\]|\\.)*)"\s*\),', re.S)


class Repo:
    def __init__(self, path):
        self.path = path
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.email', 'test@example.invalid')
        self.git('config', 'user.name', 'test')
        self.git('config', 'core.autocrlf', 'false')

    def git(self, *args, check=True):
        out = subprocess.run(['git', '-C', str(self.path), *args], capture_output=True, text=True, encoding='utf-8')
        if check and out.returncode:
            raise AssertionError(f'git {" ".join(args)}: {out.stdout}{out.stderr}')
        return out

    def write(self, rel, text):
        path = self.path / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding='utf-8', newline='\n')

    def read(self, rel):
        return (self.path / rel).read_text(encoding='utf-8')

    def commit(self, message):
        self.git('add', '-A')
        self.git('commit', '-q', '-m', message)


def append_entry(text, english, german):
    head, tail = text.rsplit('];', 1)
    return head + f'    ("{english}", "{german}"),\n];' + tail


class StringTables(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.repo = Repo(pathlib.Path(temp.name))
        shutil.copyfile(ROOT / '.gitattributes', self.repo.path / '.gitattributes')
        self.repo.write(TABLE, (ROOT / TABLE).read_text(encoding='utf-8'))
        self.repo.commit('base')
        self.base = self.repo.read(TABLE)

    def branches(self, first, second):
        self.repo.git('checkout', '-q', '-b', 'a')
        self.repo.write(TABLE, first(self.base))
        self.repo.commit('a')
        self.repo.git('checkout', '-q', '-b', 'b', 'main')
        self.repo.write(TABLE, second(self.base))
        self.repo.commit('b')

    def entries(self, text):
        """The English keys, checking that the table body is nothing but comments and entries
        (an entry may span lines: its strings can hold line breaks)."""
        body = text.split('= &[\n', 1)[1].rsplit('\n];', 1)[0]
        body = re.sub(r'(?m)^\s*//.*$', '', body)
        keys = [m[1] for m in ENTRY.finditer(body)]
        rest = ENTRY.sub('', body)
        self.assertEqual(rest.strip(), '', 'the table holds something that is not an entry')
        return keys

    def test_two_branches_appending_merge_and_rebase_cleanly_into_a_valid_table(self):
        self.branches(lambda t: append_entry(t, 'Invite a guest', 'Gast einladen'),
                      lambda t: append_entry(t, 'Measured frame time', 'Gemessene Bildzeit'))
        for how in ('merge', 'rebase'):
            with self.subTest(how=how):
                self.repo.git('checkout', '-q', '-B', 'work', 'a')
                if how == 'merge':
                    self.repo.git('merge', '-q', '--no-edit', 'b')
                else:
                    self.repo.git('checkout', '-q', '-B', 'work', 'b')
                    self.repo.git('rebase', '-q', 'a')
                merged = self.repo.read(TABLE)
                keys = self.entries(merged)
                self.assertEqual(keys[-2:] if how == 'merge' else keys[-2:], ['Invite a guest', 'Measured frame time'])
                self.assertEqual(len(keys), len(self.entries(self.base)) + 2)
                self.assertEqual(len(keys), len(set(keys)))
                rustc = shutil.which('rustc') or shutil.which('rustc', path=str(pathlib.Path.home() / '.cargo/bin'))
                if rustc:
                    out = subprocess.run([rustc, '--crate-type=lib', '--edition=2021', '--emit=metadata', '-A', 'dead_code',
                                          '--out-dir', str(self.repo.path / 'out'), str(self.repo.path / TABLE)],
                                         capture_output=True, text=True)
                    self.assertEqual(out.returncode, 0, out.stderr)

    def test_an_entry_both_change_appears_twice_for_the_table_test_to_reject(self):
        first_key = self.entries(self.base)[0]
        line = next(l for l in self.base.split('\n') if l.startswith(f'    ("{first_key}"'))
        self.branches(lambda t: t.replace(line, line[:-3] + ' A"),'),
                      lambda t: t.replace(line, line[:-3] + ' B"),'))
        self.repo.git('checkout', '-q', 'a')
        self.repo.git('merge', '-q', '--no-edit', 'b')
        keys = self.entries(self.repo.read(TABLE))
        # strings/tests.rs every_table_is_clean: "{english:?} is in the {lang:?} table twice".
        self.assertEqual(keys.count(first_key), 2)


GENERATED = """# Settings

Intro {intro}.

<!-- BEGIN GENERATED SETTINGS -->
| Setting | Default |
| A | {a} |
| B | {b} |
<!-- END GENERATED SETTINGS -->

Outro {outro}.
"""


class GeneratedDocs(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.repo = Repo(pathlib.Path(temp.name))
        shutil.copyfile(ROOT / '.gitattributes', self.repo.path / '.gitattributes')
        (self.repo.path / 'tools').mkdir()
        shutil.copyfile(ROOT / 'tools/generated_docs.py', self.repo.path / 'tools/generated_docs.py')
        with contextlib.redirect_stdout(io.StringIO()):
            generated_docs.install(self.repo.path)  # the driver as `install` registers it
        self.doc = 'client/docs/settings.md'
        self.repo.write(self.doc, GENERATED.format(intro='base', a='1', b='1', outro='base'))
        self.repo.commit('base')

    def two(self, first, second):
        self.repo.git('checkout', '-q', '-b', 'a')
        self.repo.write(self.doc, GENERATED.format(**first))
        self.repo.commit('a')
        self.repo.git('checkout', '-q', '-b', 'b', 'main')
        self.repo.write(self.doc, GENERATED.format(**second))
        self.repo.commit('b')
        self.repo.git('checkout', '-q', 'a')
        return self.repo.git('merge', '--no-edit', 'b', check=False)

    def test_a_conflict_inside_the_table_merges_as_this_sides_table_and_keeps_their_text(self):
        out = self.two(dict(intro='base', a='2', b='1', outro='base'), dict(intro='base', a='3', b='1', outro='theirs'))
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertIn('generated_docs.py', out.stderr)
        merged = self.repo.read(self.doc)
        self.assertIn('| A | 2 |', merged)
        self.assertIn('Outro theirs.', merged)
        self.assertNotIn('<<<<<<<', merged)

    def test_a_conflict_in_the_written_text_stays_a_conflict(self):
        out = self.two(dict(intro='ours', a='2', b='1', outro='base'), dict(intro='theirs', a='3', b='1', outro='base'))
        self.assertNotEqual(out.returncode, 0)
        self.assertIn('<<<<<<<', self.repo.read(self.doc))

    def test_a_worktree_without_the_tool_gets_gits_own_conflict(self):
        (self.repo.path / 'tools/generated_docs.py').unlink()
        self.repo.commit('a branch from before the tool')
        out = self.two(dict(intro='base', a='2', b='1', outro='base'), dict(intro='base', a='3', b='1', outro='base'))
        self.assertNotEqual(out.returncode, 0)
        merged = self.repo.read(self.doc)
        self.assertIn('<<<<<<< ours', merged)
        self.assertIn('| A | 2 |', merged)
        self.assertIn('| A | 3 |', merged)

    def test_the_conflict_scan(self):
        markers = generated_docs.DOCS[0].markers
        inside = f'x\n{markers[0]}\n<<<<<<< ours\na\n=======\nb\n>>>>>>> theirs\n{markers[1]}\n'
        self.assertTrue(generated_docs.conflicts_inside(inside, markers))
        self.assertFalse(generated_docs.conflicts_inside('<<<<<<< ours\n=======\n>>>>>>> theirs\n' + inside, markers))
        self.assertFalse(generated_docs.conflicts_inside(f'{markers[0]}\n{markers[1]}\n', markers))

    def test_dev_check_runs_the_doc_check_for_its_sources(self):
        paths = ['client/app/src/main/res/xml/preferences.xml', 'client/site/data/device-results.json', 'docs/configuration.md']
        self.assertEqual(
            [d.path for d in generated_docs.affected(paths)],
            ['client/docs/settings.md', 'client/docs/devices.md', 'docs/configuration.md'],
        )
        self.assertEqual([d.path for d in generated_docs.affected(['docs/configuration.md'])], ['docs/configuration.md'])
        self.assertEqual(generated_docs.affected(['docs/architecture.md']), [])
        toml_targets = [d.path for d in generated_docs.affected(['rust/core/config/settings.toml'])]
        self.assertIn('rust/core/config/src/config/catalog_gen.rs', toml_targets)
        self.assertIn('rust/web/src/lib/schema/rules.gen.ts', toml_targets)
        self.assertIn('docs/configuration.md', toml_targets)
        nj_targets = [d.path for d in generated_docs.affected(['client/protocol/policy/settings.toml'])]
        self.assertEqual(
            set(nj_targets),
            {
                'client/docs/settings.md',
                'client/protocol/policy/src/settings_catalog_gen.rs',
                'client/protocol/desktop-config/src/config/defaults_gen.rs',
                'client/app/src/main/java/com/limelight/preferences/SettingsCatalogGen.java',
            },
        )

    def test_configuration_md_conflict_inside_markers_merges_cleanly(self):
        self.doc = 'docs/configuration.md'
        self.repo.write(self.doc, GENERATED.format(intro='base', a='1', b='1', outro='base'))
        self.repo.commit('add configuration.md')
        out = self.two(dict(intro='base', a='2', b='1', outro='base'), dict(intro='base', a='3', b='1', outro='theirs'))
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        merged = self.repo.read(self.doc)
        self.assertIn('| A | 2 |', merged)
        self.assertIn('Outro theirs.', merged)
        self.assertNotIn('<<<<<<<', merged)

    def test_whole_file_generated_module_conflict_merges_as_ours(self):
        for rel in (
            'rust/core/config/src/config/retired_gen.rs',
            'client/protocol/policy/src/settings_catalog_gen.rs',
            'client/protocol/desktop-config/src/config/defaults_gen.rs',
            'client/app/src/main/java/com/limelight/preferences/SettingsCatalogGen.java',
        ):
            with self.subTest(rel=rel):
                self.repo.git('checkout', '-q', 'main')
                self.repo.write(rel, '//! Generated.\nconst X = "base";\n')
                self.repo.commit(f'add {rel}')
                self.repo.git('checkout', '-q', '-B', 'a')
                self.repo.write(rel, '//! Generated.\nconst X = "ours";\n')
                self.repo.commit('a')
                self.repo.git('checkout', '-q', '-B', 'b', 'main')
                self.repo.write(rel, '//! Generated.\nconst X = "theirs";\n')
                self.repo.commit('b')
                self.repo.git('checkout', '-q', 'a')
                out = self.repo.git('merge', '--no-edit', 'b', check=False)
                self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
                merged = self.repo.read(rel)
                self.assertIn('"ours"', merged)
                self.assertNotIn('<<<<<<<', merged)


if __name__ == '__main__':
    unittest.main()

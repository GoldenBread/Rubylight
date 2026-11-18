"""Generated documentation: regenerate it, check it, and merge it without conflicts.

python tools/generated_docs.py            # regenerate every generated doc from this worktree
python tools/generated_docs.py --check    # fail when one is stale (the file is left as it was)
python tools/generated_docs.py install    # register the merge driver in this clone's git config
python tools/generated_docs.py merge %O %A %B %P   # the merge driver git calls

Two branches that both change what a generated table is made from conflict in the
table, never in what it is made from. The merge driver (.gitattributes:
merge=rubylight-generated) merges the file normally and, when every conflict lies
inside the generated block, takes this side's block and reports success; run this
tool afterwards to regenerate the block from the merged sources. A conflict in the
hand-written text around the block stays a normal conflict. tools/dev.py runs
--check when a generated doc or its sources change; Nightjar's CI docs job checks
settings.md too.
"""
import argparse, pathlib, re, shutil, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
DRIVER = 'rubylight-generated'


class Doc:
    def __init__(self, path, command, cwd, markers, inputs):
        self.path, self.command, self.cwd, self.markers, self.inputs = path, command, cwd, markers, inputs


DOCS = [
    Doc('client/docs/settings.md', [sys.executable, 'scripts/generate-settings.py'], 'client',
        ('<!-- BEGIN GENERATED SETTINGS -->', '<!-- END GENERATED SETTINGS -->'),
        ('client/scripts/generate-settings.py', 'client/app/src/main/res/', 'client/app/src/main/java/')),
    Doc('client/docs/devices.md', ['node', 'scripts/device-docs.mjs'], 'client',
        ('<!-- device-results:start -->', '<!-- device-results:end -->'),
        ('client/scripts/device-docs.mjs', 'client/site/data/device-results.json')),
]
# The docs (docs/configuration.md) are written by hand.


def affected(paths):
    """The generated docs whose file, generator or sources are among paths."""
    found = []
    for doc in DOCS:
        if any(p == doc.path or any(p == i or (i.endswith('/') and p.startswith(i)) for i in doc.inputs) for p in paths):
            found.append(doc)
    return found


def regenerate(doc, root=ROOT):
    program = shutil.which(doc.command[0])
    if not program:
        raise RuntimeError(f'{doc.command[0]} is not on PATH (needed for {doc.path})')
    out = subprocess.run([program, *doc.command[1:]], cwd=root / doc.cwd, capture_output=True, text=True)
    if out.returncode:
        raise RuntimeError(f'{" ".join(doc.command)} failed for {doc.path}: {(out.stderr or out.stdout).strip()}')


def run_all(check, docs=None, root=ROOT):
    stale = []
    for doc in docs or DOCS:
        path = root / doc.path
        before = path.read_bytes()
        try:
            regenerate(doc, root)
            if path.read_bytes() != before:
                stale.append(doc.path)
        finally:
            if check:
                path.write_bytes(before)
    return stale


def conflicts_inside(text, markers):
    """Whether every conflict in git merge-file output lies inside the generated block."""
    start, end = markers
    inside, in_conflict, any_conflict = False, False, False
    for line in text.splitlines():
        if line.startswith(start):
            inside = True
        elif line.startswith(end):
            if in_conflict:
                return False
            inside = False
        elif line.startswith('<<<<<<< '):
            any_conflict = True
            if not inside:
                return False
            in_conflict = True
        elif line.startswith('>>>>>>> '):
            in_conflict = False
    return any_conflict and not in_conflict


def merge(base, ours, theirs, name):
    """git's merge driver: 0 and the merged file in ours, or 1 and a conflicted file."""
    doc = next((d for d in DOCS if d.path == name.replace('\\', '/')), None)
    labels = ['-L', 'ours', '-L', 'base', '-L', 'theirs']
    marked = subprocess.run(['git', 'merge-file', '-p', *labels, ours, base, theirs], capture_output=True)
    if marked.returncode < 0 or marked.returncode > 127:
        return 1  # merge-file failed: git keeps this side's file as a conflict
    if marked.returncode == 0:
        pathlib.Path(ours).write_bytes(marked.stdout)
        return 0
    if doc is None or not conflicts_inside(marked.stdout.decode('utf-8', 'replace'), doc.markers):
        pathlib.Path(ours).write_bytes(marked.stdout)
        return 1
    resolved = subprocess.run(['git', 'merge-file', '-p', '--ours', ours, base, theirs], capture_output=True)
    pathlib.Path(ours).write_bytes(resolved.stdout)
    print(f'{name}: generated table merged as this side\'s; run `python tools/generated_docs.py` '
          'after the merge to regenerate it from the merged sources.', file=sys.stderr)
    return 0


def install(root=ROOT):
    # The config is shared by every worktree of the clone: one on a branch from before this
    # tool falls back to git's own merge, conflict markers included.
    script = 'tools/generated_docs.py'
    driver = (f'if test -f {script}; then python {script} merge %O %A %B %P; '
              'else git merge-file -L ours -L base -L theirs %A %O %B; fi')
    for key, value in ((f'merge.{DRIVER}.name', 'Rubylight generated docs (regenerate after merging)'),
                       (f'merge.{DRIVER}.driver', driver)):
        subprocess.run(['git', '-C', str(root), 'config', key, value], check=True)
    print(f'merge driver {DRIVER} registered for this clone and its worktrees')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('action', nargs='?', default='regen', choices=['regen', 'install', 'merge'])
    parser.add_argument('files', nargs='*')
    parser.add_argument('--check', action='store_true', help='fail when a generated doc is stale; change nothing')
    argv = list(sys.argv[1:] if argv is None else argv)
    if not any(a in ('regen', 'install', 'merge') for a in argv):
        argv.insert(0, 'regen')  # `generated_docs.py --check client/docs/settings.md`
    args = parser.parse_args(argv)
    if args.action == 'install':
        install()
        return 0
    if args.action == 'merge':
        if len(args.files) != 4:
            parser.error('merge takes %O %A %B %P')
        return merge(*args.files)
    docs = [d for d in DOCS if not args.files or d.path in {f.replace('\\', '/') for f in args.files}]
    if not docs:
        parser.error('no generated doc among ' + ', '.join(args.files))
    try:
        stale = run_all(args.check, docs)
    except RuntimeError as error:
        print(error, file=sys.stderr)
        return 1
    if args.check:
        if stale:
            print('Stale: ' + ', '.join(stale) + '. Run `python tools/generated_docs.py` and commit the result.')
            return 1
        print('Generated docs are current.')
        return 0
    print('Regenerated: ' + (', '.join(stale) if stale else 'nothing changed'))
    return 0


if __name__ == '__main__':
    sys.exit(main())

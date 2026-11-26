"""Bump the release version and open its release notes section.

Writes LF line endings on any platform.

usage: python rust/release/bump.py 3.0.0-alpha.1 [--notes FILE]

Updates the workspace version (Cargo.toml and the workspace crates in
Cargo.lock); the full version, "Download <label>" and "For <label>," in the
READMEs and docs; the title, history and installer names in
rust/RELEASE_NOTES.md; and adds a "## New in <label>" section above the
previous one: "## Unreleased", the fragments in rust/release/notes.d
(fragments.py; deleted once they are in), then the contents of --notes, or
the bullets to fill in.

The label is the version, except that 2.0.0 release candidates were "rc.N".
Alpha and beta versions keep their full version so that 3.0.0-alpha.1 and
3.1.0-alpha.1 get different sections.
"""
import argparse, pathlib, re, sys, tomllib

import fragments

parser = argparse.ArgumentParser()
parser.add_argument('version')
parser.add_argument('--notes', type=pathlib.Path)
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parents[2]
new = args.version.removeprefix('v')
VERSION = r'\d+\.\d+\.\d+(?:-(?:alpha|beta|rc)\.\d+)?'
assert re.fullmatch(VERSION, new), new
# Read before anything is written, so a malformed fragment changes nothing.
bullets, fragment_files = fragments.read(root / fragments.NOTES_D)


def label(version):
    m = re.fullmatch(r'2\.0\.0-(rc\.\d+)', version)
    return m[1] if m else version


def anchor(version):
    return '#new-in-' + label(version).replace('.', '')


writes = []

cargo = root / 'Cargo.toml'
text = cargo.read_text(encoding='utf-8')
old = re.search(r'(?m)^version = "([^"]+)"', text)[1]
assert old != new, f'already {new}'
writes.append((cargo, text.replace(f'version = "{old}"', f'version = "{new}"', 1)))

# Cargo.lock: only the workspace's own crates carry the workspace version.
workspace_members = tomllib.loads(text)['workspace']['members']
members = {
    pkg['name']
    for rel in workspace_members
    for pkg in [tomllib.loads((root / rel / 'Cargo.toml').read_text(encoding='utf-8'))['package']]
    if pkg.get('version') == {'workspace': True}
}
lock = root / 'Cargo.lock'
lock_text, count = re.subn(r'(?m)^(name = "(?:%s)"\nversion = )"%s"$' % ('|'.join(map(re.escape, members)), re.escape(old)),
                           lambda m: f'{m[1]}"{new}"', lock.read_text(encoding='utf-8'))
assert count == len(members), f'updated {count} of {len(members)} workspace crates in Cargo.lock'
writes.append((lock, lock_text))

short = re.compile(r'(?<![\w.])%s(?![\w])' % re.escape(label(old)))
# Only the short labels that name the release to download: a sentence about
# what an older release measured or changed keeps its version.
current = re.compile(rf'(?<=Download )(?:{VERSION}|rc\.\d+)(?![\w])|(?<=For )(?:{VERSION}|rc\.\d+)(?=,)')
for name in ('README.md', 'docs/README.md', 'docs/getting-started.md', 'rust/README.md'):
    path = root / name
    doc = path.read_text(encoding='utf-8')
    doc = re.sub(r'(\[(?:Download [^]]+|release)\]\(https://github.com/RamazanKara/Rubylight/releases/tag/)[^)]+\)',
                 lambda m: f'{m[1]}v{new})', doc)
    # Releases since rc.30 are named Rubylight; earlier ones Butterpollo.
    for setup in ('butterpollo-setup', 'rubylight-setup'):
        doc = re.sub(rf'{setup}-{VERSION}\.exe', f'rubylight-setup-{new}.exe', doc)
    for package in ('butterpollo-rust', 'rubylight'):
        doc = re.sub(rf'{package}-{VERSION}-windows-x64\.zip', f'rubylight-{new}-windows-x64.zip', doc)
    doc = doc.replace(f'The workspace version is **{old}**', f'The workspace version is **{new}**')
    writes.append((path, current.sub(label(new), doc)))

for name in ('package.json', 'package-lock.json'):
    path = root / 'rust/web' / name
    pkg = re.sub(r'("version": )"[^"]+"', lambda m: f'{m[1]}"{new}"',
                 path.read_text(encoding='utf-8'), count=2 if name == 'package-lock.json' else 1)
    writes.append((path, pkg))

notes = root / 'rust/RELEASE_NOTES.md'
lines = fragments.merge(notes.read_text(encoding='utf-8').split('\n'), bullets)
assert old in lines[0], lines[0]
lines[0] = lines[0].replace(old, new)
history = next(i for i, l in enumerate(lines) if l.startswith('**Release history:** '))
entry = f'[{label(new)}]({anchor(new)})'
if entry not in lines[history]:
    lines[history] = lines[history].replace('**Release history:** ',
                                            f'**Release history:** {entry} · ', 1)
intro = next(i for i, l in enumerate(lines) if i > history and l.strip())
for setup in ('butterpollo-setup', 'rubylight-setup'):
    lines[intro] = lines[intro].replace(f'{setup}-{old}.exe', f'rubylight-setup-{new}.exe')
lines[intro] = short.sub(label(new), lines[intro].replace(old, new))
if f'## New in {label(new)}' not in lines:
    # Pending changes belong to the next release, including when release
    # preparation supplies additional notes with --notes.
    pending = []
    if '## Unreleased' in lines:
        start = lines.index('## Unreleased')
        end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith('## ')), len(lines))
        pending = '\n'.join(lines[start + 1:end]).strip().splitlines()
        del lines[start:end]
    at = lines.index(f'## New in {label(old)}')
    body = args.notes.read_text(encoding='utf-8').strip().split('\n') if args.notes else []
    body = (pending + [''] + body) if pending and body else (pending or body or ['- '])
    lines[at:at] = [f'## New in {label(new)}', '', *body, '']
notes_text = '\n'.join(lines)
for setup in ('butterpollo-setup', 'rubylight-setup'):
    notes_text = notes_text.replace(f'{setup}-{old}.exe` installs', f'rubylight-setup-{new}.exe` installs')
for package in ('butterpollo-rust', 'rubylight'):
    notes_text = notes_text.replace(f'{package}-{old}-windows-x64.zip`', f'rubylight-{new}-windows-x64.zip`')
writes.append((notes, notes_text))

for path, content in writes:
    path.write_text(content, encoding='utf-8', newline='')
for path in fragment_files:
    path.unlink()
if fragment_files:
    print(f'{len(fragment_files)} fragments from {fragments.NOTES_D} are in the notes and deleted')
print(f'{old} -> {new}; {count} crates in Cargo.lock; fill in "## New in {label(new)}" in rust/RELEASE_NOTES.md'
      if not args.notes else f'{old} -> {new}; {count} crates in Cargo.lock')

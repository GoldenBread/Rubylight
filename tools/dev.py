"""Run the smallest set of checks for what changed in this worktree.

usage: python tools/dev.py check [--base REF] [--dry-run] [--log-dir DIR]
                                 [--target-dir DIR] [--paths PATH ...]

The changed files are those between the merge base of HEAD and --base
(default origin/main) and the working tree, plus untracked files. Each maps to
checks (see plan()):

- host workspace crates, and the crates that depend on them, to
  C:\\src\\rl-quick.ps1 (fmt, clippy, tests) in a warm target slot;
- client/protocol crates, and their dependants, to `cargo test` in Nightjar's
  workspace, plus `cargo clippy -D warnings` for the crates CI lints
  (CLIENT_CLIPPY), plus the interop tests once tests/interop exists;
- client/app sources to the Gradle unit tests of the touched classes;
- rust/service, the Windows service's own workspace, to its fmt, clippy and
  tests (and the release suites, which check its version rule);
- rust/web to the web console checks;
- rust/release scripts to their unittest suites, tools/ to its own.

Markdown is documentation and maps to nothing. One PASS/FAIL table is printed;
each check's output goes to <log dir>/<run>/<check>.log with summary.json.
Stops after failure unless --keep-going. Exits 1 if any check fails.

Each build stage takes its own lanes (tools/build_lock.py), not one lock for
the whole check: the Cargo stages a Cargo slot and a warm target slot from the
pool (host: <pool>\\<n>, client: <pool>\\client-<n>), the app stage the Gradle
lane, the web stage the web lane. A target slot remembers its last checkout;
when another checkout built there, the fingerprints of the repository's own
packages are removed first (claim_target), because Cargo judges path packages
by modification time and would reuse that checkout's build.
"""
import argparse, dataclasses, datetime, hashlib, json, os, pathlib, re, shutil, subprocess, sys, time, tomllib, uuid
import build_lock, generated_docs
from build_lock import StageLease, wait_child

ROOT = pathlib.Path(__file__).resolve().parents[1]
RL_QUICK = pathlib.Path(r'C:\src\rl-quick.ps1')
LOG_ROOT = pathlib.Path(r'C:\src\butterpollo-release\unified\dev-check')
CLIENT_WORKSPACE = 'client/protocol'
# Files that change how every host crate builds.
HOST_WIDE = ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'clippy.toml', '.cargo/',
             'rust/assets/version_info.rs')
# The Windows service's workspace and what else goes into its executable
# (rust/release/service_inputs.py), plus the toolchain and Cargo settings its
# tests and clippy run with here.
SERVICE_WORKSPACE = 'rust/service'
SERVICE_INPUTS = ('rust/service/', 'rust/core/src/update_recovery.rs', 'rust/core/src/update_files.rs',
                  'rust/core/src/state/file.rs', 'rust/assets/version_info.rs', 'rust/assets/butterpollo.ico',
                  'rust-toolchain.toml', '.cargo/')
# The suites CI runs from rust/release.
RELEASE_SUITES = ('test_release.py', 'test_gpu_hang.py', 'test_timing_report.py', 'test_cadence_matrix.py',
                  'test_user_idle.py')
RELEASE_INPUTS = ('rust/release/', 'tools/nightjar_notes.py', '.github/workflows/rust-windows.yml') + SERVICE_INPUTS
GRADLE_WIDE = ('client/build.gradle', 'client/settings.gradle', 'client/gradle.properties', 'client/gradle/',
               'client/app/build.gradle', 'client/app/proguard-rules.pro', 'client/app/src/main/res/',
               'client/app/src/main/AndroidManifest.xml')
# Client workspace files outside its members that change every member's build.
CLIENT_WIDE = ('client/protocol/Cargo.toml', 'client/protocol/Cargo.lock', 'client/protocol/vendor/')
# Client crates that nightjar-android.yml lints with `clippy --all-targets -D warnings`.
CLIENT_CLIPPY = ('nightjar-desktop',)
GRADLE_TASK = ':app:testNonRootDebugUnitTest'
GRADLE_LINT_TASK = ':app:lintNonRootDebug'


@dataclasses.dataclass
class Check:
    name: str
    kind: str
    items: list = dataclasses.field(default_factory=list)
    skip: str = ''

    def describe(self):
        return self.skip or (', '.join(self.items) if self.items else 'all')


def posix(path):
    return str(path).replace('\\', '/')


def workspace(root, manifest_dir):
    """Package name, directory and path dependencies of each workspace member."""
    base = root / manifest_dir
    manifest = tomllib.loads((base / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']
    members, defaults = {}, set()
    for member in manifest['members']:
        directory = (base / member).resolve()
        package = tomllib.loads((directory / 'Cargo.toml').read_text(encoding='utf-8'))
        deps = set()
        tables = [package] + list(package.get('target', {}).values())
        for table in tables:
            for kind in ('dependencies', 'dev-dependencies', 'build-dependencies'):
                for spec in table.get(kind, {}).values():
                    if isinstance(spec, dict) and 'path' in spec:
                        deps.add(posix((directory / spec['path']).resolve().relative_to(root.resolve())))
        rel = posix(directory.relative_to(root.resolve()))
        members[rel] = (package['package']['name'], deps)
        if member in manifest.get('default-members', manifest['members']):
            defaults.add(rel)
    return members, defaults


def dependants(members, dirs):
    """The members in dirs plus every member that depends on them, transitively."""
    found, todo = set(), list(dirs)
    while todo:
        current = todo.pop()
        for rel, (_, deps) in members.items():
            if rel not in found and (rel == current or current in deps):
                found.add(rel)
                todo.append(rel)
    return found


def owner(path, dirs):
    """The deepest of dirs containing path."""
    best = None
    for d in dirs:
        if (path == d or path.startswith(d + '/')) and (best is None or len(d) > len(best)):
            best = d
    return best


def under(path, entries):
    """Whether path is one of the files, or inside one of the directories (ending in /), in entries."""
    return any(path.startswith(e) if e.endswith('/') else path == e for e in entries)


def gradle_tests(root, sources):
    """Gradle --tests filters for the unit tests covering the changed app sources."""
    tests = [p for p in (root / 'client/app/src').glob('test*/**/*') if p.suffix in ('.java', '.kt')]
    names = {p: p.stem for p in tests}

    def qualified(path):
        parts = path.relative_to(root / 'client/app/src').parts
        # src/<set>/<java|kotlin>/<package...>/<Class>.<ext>
        return '.'.join(parts[2:-1] + (path.stem,))

    filters, uncovered = set(), []
    for source in sources:
        path = root / source
        if posix(source).split('/')[3].startswith('test'):  # a test itself
            filters.add(qualified(path))
            continue
        # FooTest, else FooSomethingTest, else the tests that name Foo.
        cls = path.stem
        direct = [t for t, n in names.items() if re.fullmatch(re.escape(cls) + r'Tests?', n)]
        if not direct:
            direct = [t for t, n in names.items() if re.fullmatch(re.escape(cls) + r'[A-Z]\w*Tests?', n)]
        if not direct:
            word = re.compile(r'\b%s\b' % re.escape(cls))
            direct = [t for t in tests if re.fullmatch(r'\w*Tests?', t.stem)
                      and word.search(t.read_text(encoding='utf-8', errors='replace'))]
        if direct:
            filters.update(qualified(t) for t in direct)
        else:
            uncovered.append(cls)
    return sorted(filters), uncovered


def plan(paths, root=ROOT):
    """The checks for a list of repository-relative paths, and the paths no check covers."""
    paths = sorted({posix(p) for p in paths})
    host, host_default = workspace(root, '.')
    client, client_default = workspace(root, CLIENT_WORKSPACE)
    # Directories outside the host workspace that host crates take by path (client/protocol/core).
    host_external = {d for _, deps in host.values() for d in deps if d not in host}
    interop = root / 'tests/interop'
    interop_in_host = 'tests/interop' in host

    host_dirs, client_dirs, client_all, service = set(), set(), False, False
    app_sources, gradle_all, web, release, tools, interop_hit, notes = [], False, False, set(), False, False, []
    for path in paths:
        hit = False
        if path.endswith('.md'):
            notes.append(path)
            continue
        if under(path, HOST_WIDE):
            host_dirs.update(host)
            hit = True
        d = owner(path, set(host) | host_external)
        if d:
            host_dirs.add(d)
            hit = True
            if d.startswith('rust/core') or d.startswith('rust/host'):
                interop_hit = True
        d = owner(path, set(client))
        if d or under(path, CLIENT_WIDE):
            if d:
                client_dirs.add(d)
            else:
                client_all = True
            interop_hit = True
            hit = True
        if path.startswith('tests/interop/'):
            interop_hit = True
            hit = True
        if path.startswith('client/app/src/') and path.endswith(('.java', '.kt')):
            app_sources.append(path)
            hit = True
        elif under(path, GRADLE_WIDE):
            gradle_all = True
            hit = True
        if under(path, SERVICE_INPUTS):
            service = True
            hit = True
        if path.startswith('rust/web/'):
            web = True
            hit = True
        if under(path, RELEASE_INPUTS):
            release.update(RELEASE_SUITES)
            # The script's own suite, or the suite itself.
            name = pathlib.PurePosixPath(path).name
            for suite in (name, f'test_{name}'):
                if path.startswith('rust/release/') and re.fullmatch(r'test_\w+\.py', suite) \
                        and (root / 'rust/release' / suite).is_file():
                    release.add(suite)
            hit = True
        if path.startswith('tools/'):
            tools = True
            hit = True
        if not hit:
            notes.append(path)

    checks = []
    if host_dirs:
        crates = sorted(host[d][0] for d in dependants(host, host_dirs) if d in host)
        if interop_in_host and interop_hit:
            crates = sorted(set(crates) | {host['tests/interop'][0]})
        checks.append(Check('host', 'host', crates))
    if client_dirs or client_all:
        # A lock file change checks the default members; a crate changed with it still gets its own check.
        dirs = dependants(client, client_dirs) | (set(client_default) if client_all else set())
        crates = sorted(client[d][0] for d in dirs)
        checks.append(Check('client-protocol', 'client', crates))
    if interop_hit and not interop_in_host:
        if (interop / 'Cargo.toml').is_file():
            checks.append(Check('interop', 'interop', ['tests/interop']))
        else:
            checks.append(Check('interop', 'interop', skip='tests/interop does not exist yet'))
    if service:
        checks.append(Check('service', 'service', ['butterpollo-service']))
    if gradle_all:
        checks.append(Check('app', 'gradle', []))
    elif app_sources:
        filters, uncovered = gradle_tests(root, app_sources)
        if filters:
            checks.append(Check('app', 'gradle', filters))
        if uncovered:
            checks.append(Check('app-uncovered', 'gradle',
                                skip='no unit tests cover ' + ', '.join(sorted(set(uncovered)))))
    if web:
        checks.append(Check('web', 'web', ['check', 'node --test', 'build']))
    if release:
        checks.append(Check('release-py', 'python', sorted(release)))
    if tools:
        checks.append(Check('tools-py', 'tools', ['tools/test_*.py']))
    # A generated doc, its generator or its sources: is the doc still what they generate?
    generated = generated_docs.affected(paths)
    if generated:
        checks.append(Check('generated-docs', 'generated', [d.path for d in generated]))
    return checks, notes


def git(*args, root=ROOT):
    out = subprocess.run(['git', '-C', str(root), *args], capture_output=True, text=True, encoding='utf-8')
    if out.returncode:
        raise SystemExit(f'git {" ".join(args)} failed: {out.stderr.strip()}')
    return out.stdout


def changed(base, root=ROOT):
    merge_base = git('merge-base', 'HEAD', base, root=root).strip()
    files = git('diff', '--name-only', '--no-renames', merge_base, root=root).splitlines()
    files += git('ls-files', '--others', '--exclude-standard', root=root).splitlines()
    return merge_base, [f for f in files if f]


def worktree_name(root=ROOT):
    return root.resolve().name


STAMP = '.rubylight-checkout'


def same_path(a, b):
    return os.path.normcase(os.path.normpath(str(a).strip())) == os.path.normcase(os.path.normpath(str(b).strip()))


def local_packages(root=ROOT):
    """The names of every Cargo package in this checkout and its submodules: the ones Cargo
    builds from paths here and judges by modification time."""
    names = set()
    out = subprocess.run(['git', '-C', str(root), 'ls-files', '--recurse-submodules', '--', 'Cargo.toml', '*/Cargo.toml'],
                         capture_output=True, text=True, encoding='utf-8')
    for rel in out.stdout.splitlines():
        try:
            name = tomllib.loads((root / rel).read_text(encoding='utf-8')).get('package', {}).get('name')
        except (OSError, ValueError):
            continue
        if isinstance(name, str):
            names.add(name)
    return names


def fingerprint_dirs(target):
    """Cargo's .fingerprint folders in a target dir: <profile>, <triple>/<profile>, and the
    same below a nested target (the service's <target>/service)."""
    target = pathlib.Path(target)
    found = []
    for pattern in ('*/.fingerprint', '*/*/.fingerprint', '*/*/*/.fingerprint'):
        found += [p for p in target.glob(pattern) if p.is_dir()]
    return found


def invalidate(target, names):
    """Remove the fingerprints of the named packages so Cargo rebuilds them; the
    dependencies' (registry and git crates, keyed by version and source) stay warm."""
    if not names:
        return 0
    pattern = re.compile('^(?:%s)-[0-9a-f]{16}$' % '|'.join(re.escape(n) for n in sorted(names)))
    removed, left = 0, []
    for directory in fingerprint_dirs(target):
        for entry in directory.iterdir():
            if entry.is_dir() and pattern.match(entry.name):
                shutil.rmtree(entry, ignore_errors=True)
                if entry.exists():
                    left.append(str(entry))
                else:
                    removed += 1
    if left:
        raise RuntimeError(f'could not reset {len(left)} fingerprints in {target} (is another build using it?): {left[0]}')
    return removed


def claim_target(target, root=ROOT, previous_owner=None):
    r"""Make a reused target dir safe for this checkout; returns how many fingerprints it removed.

    Cargo judges a path package fresh when none of its files is newer than its last build,
    with the files listed relative to the package root, and hashes the package's path
    relative to the workspace. Two checkouts of the same version therefore share artifact
    names in one target dir, and a checkout whose files are older than another checkout's
    last build there gets that build (2026-11-18 in C:\src\rl-target: `cannot find value
    MAX_BITRATE_KBPS` and `unresolved import rubylight_protocol::invite_uri`, both present
    in the checkout's source). So when the folder's last checkout was another one, or is
    unknown while it holds builds, this checkout's packages lose their fingerprints first.
    """
    target = pathlib.Path(target)
    stamp = target / STAMP
    last = build_lock.read(stamp)
    mine = str(pathlib.Path(root).resolve())
    others = [o for o in (last, previous_owner) if o and o.strip() and not same_path(o, mine)]
    unknown = last is None and target.is_dir() and bool(fingerprint_dirs(target))
    removed = invalidate(target, local_packages(root)) if others or unknown else 0
    target.mkdir(parents=True, exist_ok=True)
    stamp.write_text(mine, encoding='utf-8')
    return removed


def by_affinity(slots, root=ROOT):
    """This checkout's own slots first, then other checkouts' least recently used ones (their
    dependencies are warm), then never used ones."""
    def key(slot):
        owner_file = slot.path.with_suffix('.owner')
        owner = build_lock.read(owner_file) if owner_file.exists() else None
        if owner and same_path(owner, root):
            return (0, 0)
        if owner and owner.strip():
            return (1, owner_file.stat().st_mtime)
        return (2, 0)
    return sorted(slots, key=key)


def stage_groups(check, root, args):
    """The lanes a check takes, as (name, candidates) for StageLease."""
    lock_dir, pool = args.lock_directory, args.pool
    cargo = ('cargo', lambda: build_lock.cargo_lanes(lock_dir))
    if check.kind in ('host', 'service', 'client', 'interop'):
        if args.target_dir:
            return [cargo]
        if check.kind in ('host', 'service'):
            return [cargo, ('host target', lambda: by_affinity(build_lock.host_slots(lock_dir, pool), root))]
        return [cargo, ('client target', lambda: by_affinity(build_lock.client_slots(lock_dir, pool), root))]
    if check.kind == 'gradle':
        return [('gradle', lambda: build_lock.gradle_lanes(lock_dir))]
    if check.kind == 'web':
        return [('web', lambda: build_lock.web_lanes(lock_dir))]
    return []


def stage_target(check, taken, root, target_dir=None):
    """The target dir of a Cargo check, claimed for this checkout: its pool slot, else
    --target-dir (the client's beside it, the service's inside it). None for other checks."""
    if check.kind not in ('host', 'service', 'client', 'interop'):
        return None
    slot = taken.get('host target') or taken.get('client target')
    previous = None
    if slot is not None:
        owner_file = slot.path.with_suffix('.owner')
        previous = build_lock.read(owner_file) if owner_file.exists() else None
        target = slot.path.with_suffix('')
        # C:\src\rl-quick.ps1's Enter-RlSlot reads and writes the same owner file.
        owner_file.write_text(str(pathlib.Path(root).resolve()), encoding='utf-8')
    else:
        target = pathlib.Path(target_dir)
        if check.kind in ('client', 'interop'):
            target = pathlib.Path(f'{target}-client')
    claim_target(target, root, previous)
    return target


def client_env(target):
    env = dict(os.environ)
    env['PATH'] = r'C:\msys64\ucrt64\bin;' + str(pathlib.Path.home() / '.cargo/bin') + ';' + env.get('PATH', '')
    env['LIBCLANG_PATH'] = r'C:\msys64\ucrt64\bin'
    env['CARGO_TARGET_DIR'] = str(target)
    return env


def run(command, log, cwd, env=None, shell=False):
    with open(log, 'a', encoding='utf-8', errors='replace') as f:
        f.write(f'\n== {command if isinstance(command, str) else " ".join(map(str, command))}\n')
        f.flush()
        if not shell:
            # Windows looks the program up on this process's PATH, not env's (client_env adds cargo).
            program = shutil.which(command[0], path=(env or os.environ).get('PATH'))
            if not program:
                f.write(f'{command[0]} is not on PATH\n')
                return 127
            command = [program, *command[1:]]
        try:
            return wait_child(subprocess.Popen(command, cwd=cwd, env=env, shell=shell, stdout=f, stderr=subprocess.STDOUT))
        except OSError as error:
            f.write(f'{error}\n')
            return 127


def client_specs(root, names):
    """Package ID specs: `-p rubylight-protocol` is ambiguous, the client workspace also has it from git."""
    by_name = {package: rel for rel, (package, _) in workspace(root, CLIENT_WORKSPACE)[0].items()}
    return [f'path+file:///{posix((root / by_name[name]).resolve())}#{name}' for name in names]


def client_commands(root, crates):
    """cargo test for the client crates, then clippy as CI runs it for those of them it lints."""
    args = [a for spec in client_specs(root, crates) for a in ('-p', spec)]
    commands = [['cargo', 'test', '-j', '4', '--locked', *args]]
    linted = [c for c in crates if c in CLIENT_CLIPPY]
    if linted:
        lint_args = [a for spec in client_specs(root, linted) for a in ('-p', spec)]
        commands.append(['cargo', 'clippy', '-j', '4', '--all-targets', '--locked', *lint_args, '--', '-D', 'warnings'])
    return commands


def web_dependencies(web, log, refresh=False):
    """Reuse only installs made successfully by this checker, for these inputs."""
    stamp = web / 'node_modules/.rubylight-dependencies.json'
    versions = {}
    for program in ('node', 'npm'):
        result = subprocess.run(f'{program} --version', cwd=web, shell=True, capture_output=True, text=True)
        if result.returncode:
            return False
        versions[program] = result.stdout.strip()
    inputs = {name: hashlib.sha256((web / name).read_bytes()).hexdigest()
              for name in ('package.json', 'package-lock.json')}
    config = {k: v for k, v in os.environ.items() if k.lower().startswith('npm_config_')}
    receipt = dict(inputs=inputs, versions=versions, platform=sys.platform,
                   node_env=os.environ.get('NODE_ENV'),
                   npm_config=hashlib.sha256(json.dumps(config, sort_keys=True).encode()).hexdigest())
    configs = [web / '.npmrc', pathlib.Path.home() / '.npmrc']
    if os.environ.get('NPM_CONFIG_USERCONFIG'):
        configs.append(pathlib.Path(os.environ['NPM_CONFIG_USERCONFIG']))
    receipt['npmrc'] = [hashlib.sha256(p.read_bytes()).hexdigest() if p.exists() else None for p in configs]
    installed = web / 'node_modules/.package-lock.json'
    receipt['installed'] = hashlib.sha256(installed.read_bytes()).hexdigest() if installed.exists() else None
    try:
        if not refresh and receipt['installed'] and json.loads(stamp.read_text()) == receipt:
            return True
    except (OSError, ValueError):
        pass
    stamp.unlink(missing_ok=True)
    if run('npm ci --no-audit --no-fund', log, web, shell=True):
        return False
    receipt['installed'] = hashlib.sha256(installed.read_bytes()).hexdigest() if installed.exists() else None
    stamp.write_text(json.dumps(receipt, sort_keys=True), encoding='utf-8')
    return True


def read_quick(path):
    """rl-quick's log as text, and whether it passed: EXIT 0, tests that ran and none failed.

    Windows PowerShell appends cargo's output (`*>>`) as UTF-16LE between its own UTF-8
    lines, so the file mixes both: drop the NULs and byte order marks, keep the ASCII.
    A command PowerShell can't find (cargo off PATH, a missing rl-env.ps1) leaves
    $LASTEXITCODE alone, so rl-quick can end with EXIT 0 having run nothing: that fails too.
    """
    data = path.read_bytes() if path.exists() else b''
    text = data.replace(b'\x00', b'').decode('utf-8', errors='replace')
    text = text.replace('﻿', '').replace('��', '').replace('\r\n', '\n')
    passed = (re.search(r'(?m)^EXIT 0\s*$', text) is not None
              and re.search(r'(?m)^test result: ok\.', text) is not None
              and re.search(r'(?m)^test result: FAILED', text) is None
              and 'CommandNotFoundException' not in text)
    return text, passed


def execute(check, root, log, target, fresh_dependencies=False):
    if not log.exists():
        log.write_text('', encoding='utf-8')
    if check.kind == 'host':
        crates = ','.join(f"'{c}'" for c in check.items)
        command = (f"& '{RL_QUICK}' -Repo '{root}' -Crates {crates} -TargetDir '{target}' "
                   f"-Jobs 4 -Log '{log}.quick'")
        code = run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', command], log, root)
        quick = pathlib.Path(f'{log}.quick')
        text, passed = read_quick(quick)
        with open(log, 'a', encoding='utf-8') as f:
            f.write(text)
        quick.unlink(missing_ok=True)
        return code == 0 and passed
    if check.kind == 'client':
        env = client_env(target)
        return all(run(command, log, root / CLIENT_WORKSPACE, env) == 0
                   for command in client_commands(root, check.items))
    if check.kind == 'service':
        # Its own workspace, in a folder of this worktree's target dir; without a
        # developer's linker flags, as its release build (rust/service/build.ps1).
        env = client_env(pathlib.Path(target) / 'service')
        for name in ('RUSTFLAGS', 'CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS'):
            env.pop(name, None)
        manifest = ['--manifest-path', str(root / SERVICE_WORKSPACE / 'Cargo.toml')]
        return all(run(command, log, root, env) == 0 for command in (
            ['cargo', 'fmt', *manifest, '--', '--check'],
            ['cargo', 'clippy', '-j', '4', '--all-targets', '--locked', *manifest, '--', '-D', 'warnings'],
            ['cargo', 'test', '-j', '4', '--locked', *manifest],
        ))
    if check.kind == 'interop':
        env = client_env(target)
        return run(['cargo', 'test', '-j', '4', '--locked', '--manifest-path', str(root / 'tests/interop/Cargo.toml')],
                   log, root, env) == 0
    if check.kind == 'gradle':
        env = {k: v for k, v in os.environ.items() if k.upper() != 'CARGO_TARGET_DIR'}
        filters = [a for t in check.items for a in ('--tests', t)]
        # By its full path: with NoDefaultCurrentDirectoryInExePath set, cmd
        # doesn't look in its working folder for a bare gradlew.bat.
        # A full run lints too, as CI does: a NewApi error once passed here and failed CI.
        lint = [] if filters else [GRADLE_LINT_TASK]
        return run(['cmd', '/c', str(root / 'client' / 'gradlew.bat'), '--no-daemon', '--max-workers=2', GRADLE_TASK, *lint, *filters],
                   log, root / 'client', env) == 0
    if check.kind == 'web':
        web = root / 'rust/web'
        if not web_dependencies(web, log, fresh_dependencies):
            return False
        for command in ('npm run check', 'node --test "*.test.mjs"', 'npm run build'):
            if run(command, log, web, shell=True):
                return False
        return True
    if check.kind == 'python':
        return all(run([sys.executable, '-m', 'unittest', 'discover', '-s', 'rust/release', '-p', suite],
                       log, root) == 0 for suite in check.items)
    if check.kind == 'tools':
        return run([sys.executable, '-m', 'unittest', 'discover', '-s', 'tools', '-p', 'test_*.py'], log, root) == 0
    if check.kind == 'generated':
        return run([sys.executable, str(root / 'tools/generated_docs.py'), '--check', *check.items], log, root) == 0
    raise ValueError(check.kind)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('command', choices=['check'])
    parser.add_argument('--base', default='origin/main', help='compare against the merge base with this ref')
    parser.add_argument('--dry-run', action='store_true', help='print the plan only')
    parser.add_argument('--log-dir', type=pathlib.Path)
    parser.add_argument('--target-dir', type=pathlib.Path)
    parser.add_argument('--paths', nargs='*', help='plan for these paths instead of the changed files')
    parser.add_argument('--keep-going', action='store_true', help='run remaining independent checks after a failure')
    parser.add_argument('--fresh-dependencies', action='store_true', help='force npm ci for web checks')
    parser.add_argument('--lock-directory', type=pathlib.Path, help='shared machine lock directory')
    parser.add_argument('--lock-timeout', type=float, default=1800, help='maximum build wait in seconds')
    parser.add_argument('--pool', type=pathlib.Path,
                        help='warm target pool (default: C:\\src\\rl-target-pool with the machine locks, else beside them)')
    args = parser.parse_args(argv)
    if args.lock_timeout < 0:
        parser.error('--lock-timeout must be nonnegative')
    root = ROOT
    if args.paths is None:
        merge_base, paths = changed(args.base, root)
        print(f'{len(paths)} changed files against {args.base} (merge base {merge_base[:9]})')
    else:
        paths = args.paths
    checks, unchecked = plan(paths, root)
    where = str(args.target_dir) if args.target_dir else f'a warm slot of {args.pool or build_lock.pool_directory(args.lock_directory)}'
    run_id = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ') + '-' + uuid.uuid4().hex[:8]
    log_dir = (args.log_dir or LOG_ROOT / worktree_name(root)) / run_id
    if unchecked:
        shown = ', '.join(unchecked[:8]) + (f' and {len(unchecked) - 8} more' if len(unchecked) > 8 else '')
        print(f'No check for {len(unchecked)} files (docs and the like): {shown}')
    if not checks:
        print('Nothing to check.')
        return 0
    if args.dry_run:
        print(f'Plan (Cargo target: {where}; logs in {log_dir}):')
        for check in checks:
            print(f'  {check.name:<16} {"SKIP" if check.skip else "run":<5} {check.describe()}')
        return 0
    log_dir.mkdir(parents=True, exist_ok=True)
    rows, failed, targets = [], False, {}
    for check in checks:
        log = log_dir / f'{check.name}.log'
        if check.skip:
            rows.append((check.name, 'SKIP', '', '', check.describe()))
            continue
        if failed and not args.keep_going:
            rows.append((check.name, 'NOT RUN', '', '', 'earlier check failed'))
            continue
        print(f'-- {check.name}: {check.describe()}', flush=True)
        log.write_text('', encoding='utf-8')
        waited, start = 0.0, time.monotonic()
        try:
            # Each stage takes its own lanes; pure Python suites take none.
            groups = stage_groups(check, root, args)
            if groups:
                with StageLease(groups, args.lock_directory, f'dev {check.name} {root}', args.lock_timeout) as lease:
                    waited, start = lease.waited, time.monotonic()
                    target = stage_target(check, lease.taken, root, args.target_dir)
                    if target is not None:
                        targets[check.name] = str(target)
                        with log.open('a', encoding='utf-8') as out:
                            out.write(f'== target {target}\n')
                    ok = execute(check, root, log, target, args.fresh_dependencies)
            else:
                ok = execute(check, root, log, None, args.fresh_dependencies)
        except (OSError, RuntimeError) as error:
            with log.open('a', encoding='utf-8') as out:
                out.write(str(error) + '\n')
            ok = False
        failed |= not ok
        rows.append((check.name, 'PASS' if ok else 'FAIL', f'{time.monotonic() - start:.0f} s',
                     f'{waited:.0f} s' if waited >= 1 else '', check.describe() if ok else str(log)))
    print()
    print(f'{"check":<16} {"result":<7} {"time":>6} {"wait":>6}  detail')
    for name, result, took, wait, detail in rows:
        print(f'{name:<16} {result:<7} {took:>6} {wait:>6}  {detail}')
    print(f'Logs: {log_dir}')
    summary = dict(head=git('rev-parse', 'HEAD', root=root).strip(), worktree=str(root), paths=paths,
                   targets=targets, passed=not failed, checks=[dict(name=n, result=r, time=t, wait=w, detail=d)
                   for n, r, t, w, d in rows])
    (log_dir / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n', encoding='utf-8')
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())

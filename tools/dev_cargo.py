"""`dev.py cargo`: one hand-run Cargo command in a warm target slot, leased and claimed as in `dev.py check`.

usage: python tools/dev.py cargo [--client] [--fast] [--label L] [--target-dir DIR] -- <cargo args...>

Relative paths are the checkout root's. A client command (--client, or a
--manifest-path below client/) takes a client slot and runs in Nightjar's
workspace; any other takes a host slot and runs in the checkout root.
"""
import argparse, pathlib, shutil, subprocess, sys

import build_lock, dev


def manifest_paths(cargo_args, root):
    """(index, prefix, resolved path) of each --manifest-path among Cargo's own arguments."""
    found = []
    for index, arg in enumerate(cargo_args):
        if arg == '--':  # the rest is for the test binary or the tool
            break
        if arg == '--manifest-path' and index + 1 < len(cargo_args):
            found.append((index + 1, '', (root / cargo_args[index + 1]).resolve()))
        elif arg.startswith('--manifest-path='):
            found.append((index, '--manifest-path=', (root / arg.split('=', 1)[1]).resolve()))
    return found


def main(argv, root=None):
    root = pathlib.Path(root or dev.ROOT).resolve()
    parser = argparse.ArgumentParser(prog='dev.py cargo', description=__doc__.split('\n')[0])
    parser.add_argument('--client', action='store_true', help="Nightjar's workspace and a client slot")
    parser.add_argument('--fast', action='store_true', help='allow untranslated Nightjar strings, as check --fast')
    parser.add_argument('--label', help='name shown to those waiting for the lanes')
    parser.add_argument('--target-dir', type=pathlib.Path, help='use this folder instead of a pool slot')
    parser.add_argument('--lock-directory', type=pathlib.Path, help='shared machine lock directory')
    parser.add_argument('--lock-timeout', type=float, default=1800, help='maximum build wait in seconds')
    parser.add_argument('--pool', type=pathlib.Path, help='warm target pool')
    split = argv.index('--') if '--' in argv else len(argv)
    args = parser.parse_args(argv[:split])
    cargo_args = list(argv[split + 1:])
    if not cargo_args:
        parser.error('provide the cargo arguments after --')
    if args.lock_timeout < 0:
        parser.error('--lock-timeout must be nonnegative')
    if args.target_dir:
        args.target_dir = args.target_dir.resolve()
    manifests = manifest_paths(cargo_args, root)
    client = args.client or any(path.is_relative_to(root / 'client') for _, _, path in manifests)
    cwd = root
    if client:
        # Cargo reads .cargo/config.toml from its working folder, not from the manifest's.
        cwd = root / dev.CLIENT_WORKSPACE
        for index, prefix, path in manifests:
            cargo_args[index] = f'{prefix}{path}'
    check = dev.Check('cargo', 'client' if client else 'host', [])
    label = args.label or f'dev cargo {root}'

    def say(*a, **k):
        print(*a, file=sys.stderr, **k)  # stdout is Cargo's

    build_lock.end_children_with_me()
    try:
        with build_lock.StageLease(dev.stage_groups(check, root, args), args.lock_directory, label,
                                   args.lock_timeout, say=say) as lease:
            target = dev.stage_target(check, lease.taken, root, args.target_dir)
            if client:
                env = dev.client_env(target, fast=args.fast, lock_directory=args.lock_directory)
            else:
                env = dev.host_env(target, lock_directory=args.lock_directory, fast=args.fast)
            say(f'dev cargo: target {target}', flush=True)
            # Windows looks the program up on this process's PATH, not env's.
            program = shutil.which('cargo', path=env.get('PATH'))
            if not program:
                say('cargo is not on PATH')
                return 127
            return build_lock.wait_child(subprocess.Popen([program, *cargo_args], cwd=cwd, env=env))
    except KeyboardInterrupt:
        return 130
    except (OSError, RuntimeError) as error:
        say(str(error))
        return 1

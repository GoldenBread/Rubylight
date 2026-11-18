"""GitHub CI as a check lane for a pushed wip/** branch: start or find its runs, watch them, print the result.

python tools/ci.py <branch> [--dispatch host,android,macos,tools|all] [--timeout SECONDS] [--no-wait]

Pushes nothing: push the branch yourself (`git push origin HEAD:wip/<topic>`).
A push to wip/** runs the host (rust-windows.yml), Nightjar (nightjar-android.yml,
nightjar-macos.yml) and tools (developer-tools.yml) workflows whose path filter
matches what the push changed, as on main: docs-only pushes run nothing. GitHub
compares a push with the branch's previous head, so a later push that changed
only docs starts nothing even when the branch as a whole needs CI; --dispatch
starts the named workflows on the branch anyway (workflow_dispatch ignores path
filters). Exit codes: 0 all runs passed, 1 a run failed or was cancelled,
2 no run to watch, 3 timed out.
"""
import argparse, json, pathlib, re, subprocess, sys, time

ROOT = pathlib.Path(__file__).resolve().parents[1]
REPO = 'RamazanKara/Rubylight'
WORKFLOWS = {
    'host': 'rust-windows.yml',
    'android': 'nightjar-android.yml',
    'macos': 'nightjar-macos.yml',
    'tools': 'developer-tools.yml',
}


def run(*command, check=True):
    out = subprocess.run(command, capture_output=True, text=True, encoding='utf-8', cwd=ROOT)
    if check and out.returncode:
        raise SystemExit(f'{" ".join(command)} failed: {(out.stderr or out.stdout).strip()}')
    return out.stdout


def gh(*args, check=True):
    return run('gh', '-R', REPO, *args, check=check)


def glob_regex(pattern):
    """GitHub's filter pattern syntax: ** crosses folders (**/ also matches none), * and ? do not."""
    out, i = '', 0
    while i < len(pattern):
        if pattern.startswith('**/', i):
            out += '(?:.*/)?'
            i += 3
        elif pattern.startswith('**', i):
            out += '.*'
            i += 2
        elif pattern[i] == '*':
            out += '[^/]*'
            i += 1
        elif pattern[i] == '?':
            out += '[^/]'
            i += 1
        else:
            out += re.escape(pattern[i])
            i += 1
    return re.compile(out + r'\Z')


def path_filter(workflow):
    """The push trigger's branch patterns and path patterns (None: every path)."""
    import yaml
    data = yaml.safe_load((ROOT / '.github/workflows' / workflow).read_text(encoding='utf-8'))
    on = data.get('on', data.get(True)) or {}
    push = on.get('push') or {}
    return push.get('branches') or [], push.get('paths')


def triggers(patterns, files):
    """Whether a push changing files runs a workflow with these path patterns (the last match wins)."""
    if patterns is None:
        return bool(files)
    compiled = [(p.startswith('!'), glob_regex(p.lstrip('!'))) for p in patterns]
    for name in files:
        included = False
        for negated, regex in compiled:
            if regex.match(name):
                included = not negated
        if included:
            return True
    return False


def branch_matches(patterns, branch):
    return any(glob_regex(p).match(branch) for p in patterns)


def expected(branch, files):
    """The workflows a push of these files to branch starts, by their push filters."""
    found = []
    for key, workflow in WORKFLOWS.items():
        branches, paths = path_filter(workflow)
        if branch_matches(branches, branch) and triggers(paths, files):
            found.append(key)
    return found


def remote_head(branch):
    line = run('git', 'ls-remote', 'origin', f'refs/heads/{branch}').strip()
    return line.split()[0] if line else None


def branch_files(sha):
    """Files the branch changes against main (a hint: GitHub filters each push on its own diff)."""
    run('git', 'fetch', '-q', 'origin', 'main', check=False)
    base = run('git', 'merge-base', 'origin/main', sha, check=False).strip()
    if not base:
        return None
    return [f for f in run('git', 'diff', '--name-only', '--no-renames', base, sha).splitlines() if f]


def runs_for(branch, sha):
    fields = 'databaseId,workflowName,headSha,status,conclusion,event,url,createdAt,updatedAt'
    data = json.loads(gh('run', 'list', '--branch', branch, '--limit', '50', '--json', fields) or '[]')
    return [r for r in data if r['headSha'] == sha]


def failed_jobs(run_id):
    data = json.loads(gh('run', 'view', str(run_id), '--json', 'jobs', check=False) or '{}')
    return [j['name'] for j in data.get('jobs', []) if j.get('conclusion') not in ('success', 'skipped', None)]


def seconds(run_info):
    from datetime import datetime
    try:
        start = datetime.fromisoformat(run_info['createdAt'].replace('Z', '+00:00'))
        end = datetime.fromisoformat(run_info['updatedAt'].replace('Z', '+00:00'))
        return int((end - start).total_seconds())
    except (KeyError, ValueError):
        return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('branch')
    parser.add_argument('--dispatch', default='', help='start these workflows on the branch: ' + ','.join(WORKFLOWS) + ' or all')
    parser.add_argument('--timeout', type=float, default=5400, help='give up watching after this many seconds')
    parser.add_argument('--settle', type=float, default=90, help='how long to wait for expected runs to appear')
    parser.add_argument('--poll', type=float, default=30)
    parser.add_argument('--no-wait', action='store_true', help='print the runs found so far and exit')
    args = parser.parse_args(argv)
    branch = args.branch.removeprefix('refs/heads/').removeprefix('origin/')
    if branch == 'main':
        parser.error('main has its own CI; this lane is for wip/** branches')
    sha = remote_head(branch)
    if not sha:
        print(f'origin has no branch {branch}; push it first (this tool pushes nothing).')
        return 2
    print(f'{branch} at {sha[:9]} on origin')
    wanted = [w for w in (WORKFLOWS if args.dispatch == 'all' else args.dispatch.split(',')) if w]
    unknown = [w for w in wanted if w not in WORKFLOWS]
    if unknown:
        parser.error(f'unknown workflow {unknown[0]}; choose from {", ".join(WORKFLOWS)} or all')
    before = set()
    if wanted:
        # Runs of an earlier dispatch on the same head are not this one's.
        before = {r['databaseId'] for r in runs_for(branch, sha)}
        for key in wanted:
            gh('workflow', 'run', WORKFLOWS[key], '--ref', branch)
            print(f'started {WORKFLOWS[key]} on {branch}')
        hint = wanted
    else:
        files = branch_files(sha)
        hint = expected(branch, files) if files is not None else []
        if not branch_matches(path_filter(WORKFLOWS['host'])[0], branch):
            print(f'{branch} is not a wip/** branch: pushes to it start no CI here; use --dispatch.')
        print('expected by the path filters: ' + (', '.join(WORKFLOWS[k] for k in hint) or 'none (docs only)'))
    names = {'host': 'Rubylight Windows', 'android': 'Nightjar Android', 'macos': 'Nightjar macOS',
             'tools': 'Developer workflow checks'}
    want_names = {names[k] for k in hint}
    started = time.monotonic()
    last = None
    while True:
        found = runs_for(branch, sha)
        if wanted:
            found = [r for r in found if r['event'] == 'workflow_dispatch' and r['databaseId'] not in before]
        # The newest run per workflow (a re-run or a later dispatch replaces it).
        latest = {}
        for r in sorted(found, key=lambda r: r['createdAt']):
            latest[r['workflowName']] = r
        state = sorted((n, r['status'], r['conclusion']) for n, r in latest.items())
        if state != last:
            for n, status, conclusion in state:
                print(f'  {n}: {conclusion or status}', flush=True)
            last = state
        missing = want_names - set(latest)
        settled = time.monotonic() - started >= args.settle
        done = all(r['status'] == 'completed' for r in latest.values())
        if args.no_wait or (latest and done and (not missing or settled)) or (not latest and settled):
            break
        if time.monotonic() - started >= args.timeout:
            print(f'timed out after {args.timeout:g} s')
            return 3
        time.sleep(args.poll)
    if not latest:
        print(f'No workflow ran for {sha[:9]}. GitHub filters each push by what that push changed: '
              f'use --dispatch {",".join(hint) or "all"} to run them anyway.')
        return 2
    if missing and not args.no_wait:
        print('Not started: ' + ', '.join(sorted(missing)) + ' (the last push did not touch their paths; --dispatch runs them).')
    print()
    print(f'{"workflow":<28} {"result":<10} {"time":>6}  url')
    bad = False
    for name, r in sorted(latest.items()):
        result = r['conclusion'] or r['status']
        bad |= r['status'] == 'completed' and result not in ('success', 'skipped', 'neutral')
        print(f'{name:<28} {result:<10} {seconds(r):>5}s  {r["url"]}')
        if r['status'] == 'completed' and result not in ('success', 'skipped', 'neutral'):
            for job in failed_jobs(r['databaseId']):
                print(f'    failed job: {job}')
    if args.no_wait and not all(r['status'] == 'completed' for r in latest.values()):
        return 0
    return 1 if bad else 2 if missing and not args.no_wait else 0


if __name__ == '__main__':
    sys.exit(main())

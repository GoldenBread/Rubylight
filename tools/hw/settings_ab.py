r"""One Nightjar Debug setting, Off against On, on the Retroid: 1080p60 HEVC from the isolated host of fec_wifi_ab.py
(the stream and analysis of smoothest_ab.py, one APK for both arms, the setting flipped in the app's preferences).

  settings_ab.py run --setting submit|priority --arm off|on --rep N [--load] [--seconds 90] --apk PATH
  settings_ab.py table                      every run in results.jsonl as a Markdown table

submit = "Speed up frame hand-off" (checkbox_submit_hint), priority = "Decoder priority for receiving"
(checkbox_receive_decode_priority). Both runs force Direct submit and Prioritize stream traffic on, which the priority
setting needs. --load keeps busy loops running on the device's CPUs during the stream (a CPU stress, not a game).
Exit 3: an encoder hang in the host log; stop and investigate, never retry.
"""
import argparse, json, pathlib, subprocess, sys, tempfile, time, os

import fec_wifi_ab as f
import smoothest_ab as s

OUT = pathlib.Path('D:/CodexArtifacts/nj-settings-ab/runs')
KEYS = {'submit': 'checkbox_submit_hint', 'priority': 'checkbox_receive_decode_priority'}
STRESS_JOBS = 8


def set_prefs(setting, arm):
    f.set_prefs('host', s.KBPS)
    path = f'shared_prefs/{f.PACKAGE}_preferences.xml'
    prefs = f.run_as(f'cat {path}').replace('\r\n', '\n')
    prefs = f.pset(prefs, 'frame_pacing', 'string', 'smoothness')
    for key, value in (('checkbox_codec_direct', 'true'), ('checkbox_network_priority', 'true'),
                       (KEYS['submit'], 'false'), (KEYS['priority'], 'false'),
                       (KEYS[setting], 'true' if arm == 'on' else 'false')):
        prefs = f.pset(prefs, key, 'boolean', value)
    with tempfile.NamedTemporaryFile('w', newline='\n', suffix='.xml', delete=False) as t:
        t.write(prefs)
    f.adb('push', t.name, '/data/local/tmp/nj-ab-prefs.xml')
    os.unlink(t.name)
    f.run_as(f'cp /data/local/tmp/nj-ab-prefs.xml {path}')
    f.sh('rm -f /data/local/tmp/nj-ab-prefs.xml')
    got = f.run_as(f'cat {path}')
    want = f'name="{KEYS[setting]}" value="{"true" if arm == "on" else "false"}"'
    if want not in got or 'name="checkbox_codec_direct" value="true"' not in got:
        raise SystemExit('prefs did not take')


def cpu_busy_percent():
    """Whole-device CPU use over one second, from /proc/stat."""
    def read():
        v = [int(x) for x in f.sh('head -1 /proc/stat', check=False).split()[1:9]]
        return sum(v), v[3] + v[4]
    t0, i0 = read(); time.sleep(1.0); t1, i1 = read()
    return round(100 * (1 - (i1 - i0) / max(1, t1 - t0)), 1)


def check_load(jobs):
    """Stops the run unless `jobs` busy shells run and the CPUs are busy; returns the CPU use in percent."""
    time.sleep(1.0)
    ps = f.sh('ps -A -o PID,STAT,NAME,ARGS', check=False)
    # The subshells share the parent's command line, which holds the loop text.
    busy = sum(1 for line in ps.splitlines() if 'while :' in line and 'ps -A' not in line)
    cpu = cpu_busy_percent()
    f.log(f'load check: {busy} sh processes, CPU {cpu}%')
    if busy < jobs or cpu < 70:
        f.sh('pkill -f "while :" ; true', check=False)
        raise SystemExit(f'load did not start: {busy} sh processes, CPU {cpu}%')
    return cpu


def run(args):
    tag = f'{args.setting}-{args.arm}-{"load" if args.load else "idle"}-{args.rep}'
    OUT.mkdir(parents=True, exist_ok=True)
    if (OUT / f'{tag}-host.log').exists():
        raise SystemExit(f'{tag} exists')
    f.OUT = OUT
    f.OWNER = 'nj-settings-ab'
    f.installed_host_idle()
    f.take_locks(args.lock_wait)
    host = motion = None
    load_cpu = None
    try:
        f.installed_host_idle()
        f.sh('input keyevent KEYCODE_WAKEUP'); f.sh('svc power stayon true'); f.sh('wm dismiss-keyguard', check=False)
        f.sh(f'am force-stop {f.PACKAGE}')
        f.adb('install', '-r', '-t', str(args.apk))
        set_prefs(args.setting, args.arm)
        f.sh('logcat -c', check=False)
        f.run_as('rm -f files/butterpollo-latency.csv files/butterpollo-latency.csv.tmp')
        host = f.start_host(tag)
        motion = subprocess.Popen([str(f.MOTION_PROBE), f.primary_monitor()['display_name'], str(args.seconds + 40),
                                   str(OUT / f'{tag}-motion.json'), '60', '128'],
                                  stdout=open(OUT / f'{tag}-motion.log', 'w'), stderr=subprocess.STDOUT,
                                  creationflags=0x08000000,
                                  env=dict(os.environ, PATH=r'C:\msys64\ucrt64\bin;' + os.environ['PATH']))
        time.sleep(1.5)
        if motion.poll() is not None:
            raise SystemExit(f'motion probe exited {motion.returncode}')
        f.log(f'{tag}: stream {args.seconds} s')
        f.sh(f'am start -n {f.PACKAGE}/com.limelight.ShortcutTrampoline --es Name njrf --es AppUuid {f.APP_UUID} '
             '--es AppName Desktop --ez HDR false')
        time.sleep(10)
        if args.load:
            loops = ' '.join('(while :; do :; done) &' for _ in range(STRESS_JOBS))
            # Detached busy loops; the stream's own threads compete with them for the CPUs.
            f.sh(f"nohup sh -c '{loops} sleep {args.seconds}; kill 0' >/dev/null 2>&1 &", check=False)
            load_cpu = check_load(STRESS_JOBS)
            f.log(f'{tag}: load verified, device CPU {load_cpu}% busy')
        time.sleep(args.seconds / 2 - 10)
        mode = s.active_hz(f.sh('dumpsys display', check=False))
        time.sleep(args.seconds - args.seconds / 2)
        try:
            f.close_stream()
        except Exception as e:
            f.log(f'close failed: {e}')
        time.sleep(4)
        f.sh(f'am force-stop {f.PACKAGE}')
        f.sh('pkill -f "while :" ; true', check=False)
        time.sleep(2)
        f.stop_host(host); host = None
        motion.terminate(); motion = None
        text = f.run_as('cat files/butterpollo-latency.csv', check=False)
        if 'frame_number' not in text:
            text = f.run_as('cat files/butterpollo-latency.csv.tmp', check=False)
        (OUT / f'{tag}-latency.csv').write_text(text.replace('\r\n', '\n'))
        (OUT / f'{tag}-logcat.txt').write_text(f.sh('logcat -d', check=False), errors='replace')
        import shutil
        shutil.copy(f.PROFILE / 'logs' / 'butterpollo.log', OUT / f'{tag}-host.log')
    finally:
        if motion is not None:
            motion.terminate()
        if host is not None:
            f.stop_host(host)
        f.release_locks()
        f.sh('svc power stayon false', check=False)
    s.OUT = OUT
    result = s.analyse(tag, f'{args.setting}-{args.arm}', args.rep, args.seconds, mode)
    result.update(setting=args.setting, on=args.arm == 'on', load=args.load, load_cpu_percent=load_cpu)
    with open(OUT / 'results.jsonl', 'a') as out:
        out.write(json.dumps(result) + '\n')
    f.log(json.dumps(result))
    if result['encoder_hang']:
        print('encoder hang in the host log: stop and investigate', file=sys.stderr)
        sys.exit(3)


def table(_):
    s.OUT = OUT
    print('| Run | Frames | Drops by reason | Per min | Receive to release p50 / p95 ms | Wait p50 / p95 ms | Gaps over 25 / 100 ms |')
    print('|---|---|---|---|---|---|---|')
    for line in (OUT / 'results.jsonl').read_text().splitlines():
        r = json.loads(line)
        print(f"| {r['tag']} | {r['frames']} | {r['reasons'] or '-'} | {r['drops_per_min']} | "
              f"{r['receive_to_release_p50']} / {r['receive_to_release_p95']} | "
              f"{r['input_to_output_p50']} / {r['input_to_output_p95']} | {r['gaps_over_25ms']} / {r['gaps_over_100ms']} |")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='cmd', required=True)
    r = sub.add_parser('run')
    r.add_argument('--setting', required=True, choices=KEYS); r.add_argument('--arm', required=True, choices=('off', 'on'))
    r.add_argument('--apk', required=True, type=pathlib.Path); r.add_argument('--rep', type=int, default=1)
    r.add_argument('--seconds', type=int, default=90); r.add_argument('--load', action='store_true')
    r.add_argument('--lock-wait', type=int, default=1800)
    r.set_defaults(fn=run)
    sub.add_parser('table').set_defaults(fn=table)
    args = p.parse_args()
    args.fn(args)


if __name__ == '__main__':
    main()

#!/bin/sh
# Streams from an isolated Linux test host to stock Moonlight (moonlight-qt), which runs
# inside a headless gamescope of its own so nothing opens on the desktop:
#   tools/hw/linux-host/moonlight.sh [CODEC [WIDTH HEIGHT FPS [SECONDS [KBPS]]]]   CODEC: H.264 or HEVC
# Needs moonlight-qt (the moonlight command, or the Flathub com.moonlight_stream.Moonlight) and a
# release host build (HOST_EXE names another). It pairs with a PIN through the host's console
# API, streams SECONDS, quits the app and prints Moonlight's and the host's lines that matter.
set -eu
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
WORK=${WORK:-${TMPDIR:-/tmp}/rubylight-linux-moonlight}
HOST_EXE=${HOST_EXE:-$REPO/target/release/butterpollo}
if command -v moonlight >/dev/null; then MOONLIGHT=${MOONLIGHT:-moonlight}
else MOONLIGHT=${MOONLIGHT:-flatpak run com.moonlight_stream.Moonlight}; fi
HOST_EXE=$(realpath "$HOST_EXE")
PORT=38989
CODEC=${1:-HEVC}; W=${2:-1920}; H=${3:-1080}; FPS=${4:-60}; SECS=${5:-30}; KBPS=${6:-30000}
rm -rf "$WORK"; mkdir -p "$WORK/profile/logs"
P=$WORK/profile
cat > "$P/sunshine.conf" <<EOF
port = $PORT
upnp = false
enable_discovery = false
system_tray = false
updates = off
pyrowave = false
sunshine_name = linux-test
EOF
APP=${APP:-"ffplay -hide_banner -loglevel warning -fs -f lavfi -i 'testsrc2=size=${W}x${H}:rate=${FPS}[out0];sine=frequency=440:sample_rate=48000[out1]'"}
python3 - "$P" "$APP" <<'EOF'
import hashlib, json, os, sys
p, app = sys.argv[1], sys.argv[2]
json.dump({'apps': [{'name': 'Test', 'cmd': app, 'prep-cmd': [], 'working-dir': ''}]}, open(p + '/apps.json', 'w'))
salt = os.urandom(8).hex()
json.dump({'username': 'test', 'salt': salt,
           'password': hashlib.sha256(('rust-smoke-only' + salt).encode()).digest()[::-1].hex().upper()},
          open(p + '/sunshine_state.json', 'w'))
EOF
RUST_LOG=${RUST_LOG:-info} "$HOST_EXE" --config-dir "$P" --assets "$REPO/rust/assets" --port $PORT --bind 127.0.0.1 --no-tray \
    > "$WORK/host.stdout.log" 2> "$WORK/host.stderr.log" &
HOST_PID=$!
trap 'kill $HOST_PID 2>/dev/null; wait $HOST_PID 2>/dev/null || true' EXIT
for i in $(seq 60); do curl -s -o /dev/null "http://127.0.0.1:$PORT/serverinfo" && break; sleep 1; done
for i in $(seq 30); do
  m=$(curl -s "http://127.0.0.1:$PORT/serverinfo" | sed -n 's:.*<ServerCodecModeSupport>\([0-9]*\)<.*:\1:p')
  [ "${m:-0}" != 0 ] && break; sleep 1
done
echo "ServerCodecModeSupport=$m"
# Moonlight pairs with a PIN of our choosing; the console accepts it for the one pending request.
PIN=4321
$MOONLIGHT pair 127.0.0.1:$PORT --pin $PIN > "$WORK/pair.log" 2>&1 &
PAIR=$!
python3 - "$PORT" "$PIN" <<'EOF'
import sys, time, requests, urllib3
urllib3.disable_warnings()
port, pin = int(sys.argv[1]), sys.argv[2]
web = f'https://127.0.0.1:{port + 1}'
s = requests.Session(); s.verify = False
s.headers['X-CSRF-Token'] = s.post(web + '/api/auth/login', json={'username': 'test', 'password': 'rust-smoke-only'}, timeout=10).json()['csrf_token']
deadline = time.monotonic() + 60
while time.monotonic() < deadline:
    pending = s.get(web + '/api/clients/pending', timeout=3).json()['requests']
    if pending:
        r = s.post(web + '/api/pin', json={'pin': pin, 'name': pending[0]['name'], 'uniqueid': pending[0]['uniqueid']}, timeout=10)
        r.raise_for_status(); print('PIN accepted for', pending[0]['name']); break
    time.sleep(0.2)
else:
    sys.exit('Moonlight sent no pairing request')
time.sleep(2)
for c in s.get(web + '/api/clients/list', timeout=10).json()['clients']:
    # A first client may launch; grant it explicitly in case it is not the first.
    s.post(web + '/api/clients/update', json={'uuid': c['uuid'], 'perm': 0x071f1f00}, timeout=10).raise_for_status()
print('clients', [c['name'] for c in s.get(web + '/api/clients/list', timeout=10).json()['clients']])
EOF
wait $PAIR || { cat "$WORK/pair.log"; exit 1; }
echo "PAIRED: $(tail -1 "$WORK/pair.log")"
# The stream, inside a headless gamescope of its own, for SECS seconds.
timeout $((SECS + 15)) gamescope --backend headless -W "$W" -H "$H" -r "$FPS" -- \
  sh -c "$MOONLIGHT stream 127.0.0.1:$PORT Test --resolution ${W}x${H} --fps $FPS --bitrate $KBPS --video-codec $CODEC --display-mode fullscreen --audio-config stereo --quit-after & M=\$!; sleep $SECS; $MOONLIGHT quit 127.0.0.1:$PORT >/dev/null 2>&1; sleep 3; kill \$M 2>/dev/null; wait \$M" \
  > "$WORK/moonlight.log" 2>&1 || true
L=$P/logs/butterpollo.log
echo "--- Moonlight"
grep -iE 'error|fail|video stream is|decoder|codec|frames|connection|terminated|quit' "$WORK/moonlight.log" | grep -v '^\s*$' | tail -25
echo "--- host"
sed 's/\x1b\[[0-9;]*m//g' "$L" | grep -E 'stream peer|stream timings|stream ended|ERROR|WARN' | cut -c1-260 | tail -25

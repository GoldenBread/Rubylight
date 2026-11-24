#!/bin/sh
# Streams from an isolated Linux test host (its own profile and ports, not the installed host)
# to Nightjar's headless client through rust/tests/interop.py, and prints the client's verdict:
#   tools/hw/linux-host/stream.sh [CODEC [WIDTH HEIGHT FPS [SECONDS [KBPS]]]]   CODEC: h264 or hevc
# Build first: cargo build -p butterpollo, and in client/protocol
#   CARGO_TARGET_DIR=../../target/nightjar cargo build -p nightjar-cli --features ffmpeg
# HOST_EXE and CLIENT name other builds (release ones for timing), APP the app the host starts
# inside its gamescope session (a test picture and a 440 Hz tone by default), WORK the run's
# folder. The client's own variables pass through, e.g. BUTTERPOLLO_TEST_TIMING_CSV=FILE for
# the timing record and NIGHTJAR_CLI_INPUT_SCRIPT=3 with
#   APP="target/probes/release/input_recorder OUT" (rust/linux/probes) for input in a stream.
#   HOST_CONF="vaapi_latency_over_power = false" adds lines to the sunshine.conf of the host.
set -eu
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
WORK=${WORK:-${TMPDIR:-/tmp}/rubylight-linux-stream}
HOST_EXE=${HOST_EXE:-$REPO/target/debug/butterpollo}
CLIENT=${CLIENT:-$REPO/target/nightjar/debug/nightjar-cli}
HOST_EXE=$(realpath "$HOST_EXE")
CLIENT=$(realpath "$CLIENT")
PORT=38989
CODEC=${1:-h264}; W=${2:-1280}; H=${3:-720}; FPS=${4:-60}; SECS=${5:-10}; KBPS=${6:-20000}
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
${HOST_CONF:-}
EOF
# APP=desktop streams the desktop through the ScreenCast portal (an app without a command).
# APP=steam:APPID starts that Steam game as an imported library game, on the desktop it streams.
[ "${APP:-}" = desktop ] && APP=" "
APP=${APP:-"ffplay -hide_banner -loglevel warning -fs -f lavfi -i 'testsrc2=size=${W}x${H}:rate=${FPS}[out0];sine=frequency=440:sample_rate=48000[out1]'"}
python3 - "$P" "$APP" <<'EOF'
import hashlib, json, os, sys
p, app = sys.argv[1], sys.argv[2].strip()
entry = {'name': 'Test', 'cmd': app, 'prep-cmd': [], 'working-dir': ''}
if app.startswith('steam:'):
    # A Steam game as the library import writes it (rust/core/session/src/library.rs).
    appid = app.split(':', 1)[1]
    command = f'cmd /c start "" steam://rungameid/{appid}'
    import re
    install = ''
    steam = os.path.expanduser('~/.local/share/Steam')
    vdf = os.path.join(steam, 'steamapps', 'libraryfolders.vdf')
    libraries = re.findall(r'"path"\s+"([^"]+)"', open(vdf).read()) if os.path.exists(vdf) else [steam]
    for library in libraries:
        manifest = os.path.join(library, 'steamapps', f'appmanifest_{appid}.acf')
        if os.path.exists(manifest):
            folder = re.search(r'"installdir"\s+"([^"]+)"', open(manifest).read()).group(1)
            install = os.path.join(library, 'steamapps', 'common', folder)
    entry.update({'cmd': command, 'auto-detach': True, 'launcher-source': 'steam', 'launcher-id': appid,
                  'launcher-command': command, 'steam-id': appid, 'launcher-install-dir': install,
                  'steam-install-dir': install,
                  'launcher-launch': {'kind': 'uri', 'uri': f'steam://rungameid/{appid}'}})
json.dump({'apps': [entry]}, open(p + '/apps.json', 'w'))
salt = os.urandom(8).hex()
json.dump({'username': 'test', 'salt': salt,
           'password': hashlib.sha256(('rust-smoke-only' + salt).encode()).digest()[::-1].hex().upper()},
          open(p + '/sunshine_state.json', 'w'))
EOF
RUST_LOG=${RUST_LOG:-info} "$HOST_EXE" --config-dir "$P" --assets "$REPO/rust/assets" --port $PORT --bind 127.0.0.1 --no-tray \
    > "$WORK/host.stdout.log" 2> "$WORK/host.stderr.log" &
HOST_PID=$!
# On exit, the host and the gamescope sessions it started (rust/linux/io/src/gamescope.rs).
stop() {
  kill $HOST_PID 2>/dev/null; wait $HOST_PID 2>/dev/null
  for pid in "${XDG_RUNTIME_DIR:-/tmp}/rubylight-gamescope/$HOST_PID"-*/pid; do
    [ -f "$pid" ] && kill "$(cat "$pid")" 2>/dev/null
  done
  true
}
trap stop EXIT
for i in $(seq 60); do curl -s -o /dev/null "http://127.0.0.1:$PORT/serverinfo" && break; sleep 1; done
# Wait for the codec probe to publish codecs.
for i in $(seq 30); do
  m=$(curl -s "http://127.0.0.1:$PORT/serverinfo" | sed -n 's:.*<ServerCodecModeSupport>\([0-9]*\)<.*:\1:p')
  [ "${m:-0}" != 0 ] && break; sleep 1
done
echo "ServerCodecModeSupport=$m"
cd "$WORK"
BUTTERPOLLO_TEST_MATCH_DISPLAY=1 BUTTERPOLLO_TEST_PORT=$PORT BUTTERPOLLO_TEST_CLIENT_EXE="$CLIENT" \
  python3 "$REPO/rust/tests/interop.py" "$WORK" "$CODEC" "$W" "$H" "$FPS" "$SECS" "$KBPS" 2>&1 | tee "$WORK/client.log"

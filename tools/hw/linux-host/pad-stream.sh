#!/bin/bash
# A controller through a real stream: the client (NIGHTJAR_CLI_PAD_TYPE) announces a pad, the
# host builds it, rust/linux/probes' input_recorder in the stream's gamescope session reads it
# and asks it to rumble, and the client prints the rumble it gets back.
#   tools/hw/linux-host/pad-stream.sh LABEL TYPE [HOST_CONF]
# TYPE is xbox, playstation, nintendo or steam; HOST_CONF a sunshine.conf line such as
# "gamepad = vhf_ds4". Builds needed: the release host, target/nightjar/release/nightjar-cli
# and target/probes/release/input_recorder. Output goes to $WORK_ROOT (default $TMPDIR).
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
S=${WORK_ROOT:-${TMPDIR:-/tmp}/rubylight-pad-stream}; mkdir -p "$S"
cd "$REPO"
REC=$(realpath ${PROBE:-target/probes/release/input_recorder})
label=$1 type=$2 conf=$3; out=$S/pads-$label.txt; rm -f $out
echo "=== $label (client type $type, ${conf:-defaults})"
(sleep 21; for d in /sys/class/input/event*; do e=${d##*/}; ph=$(grep -s HID_PHYS= $d/device/device/uevent | cut -d= -f2); n=$(cat $d/device/name); case "$ph$n" in rubylight-*|*Rubylight*) acl=$(getfacl -p /dev/input/$e 2>/dev/null | grep -c "^user:$USER:"); echo "  device $e \"$n\" phys=${ph:-none} user_acl=$acl";; esac; done) > $S/mid-$label.txt &
NIGHTJAR_CLI_INPUT_SCRIPT=2 NIGHTJAR_CLI_PAD_TYPE=$type HOST_CONF="$conf" APP="$REC $out" WORK=$S/pads-run-$label HOST_EXE=${HOST_EXE:-target/release/butterpollo} CLIENT=target/nightjar/release/nightjar-cli timeout 200 tools/hw/linux-host/stream.sh h264 1280 720 60 30 10000 2>&1 | grep -E '^(RESULT|FEEDBACK Rumble)' | cut -c1-120 | sort | uniq -c
wait; cat $S/mid-$label.txt
grep -E 'opened|refused|key 304|abs 0 |rumble' $out | sed 's/^/  probe: /' | sort | uniq | head -14

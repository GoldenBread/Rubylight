#!/bin/sh
# Installs Rubylight's KMS broker from a build of this checkout, as the package would, for a
# test before the package is rebuilt: the binary in /usr/lib/rubylight, its socket and service
# units, the rubylight group (and USER in it), and the socket enabled. Run as root:
#   cargo build --release -p rubylight-kms-broker
#   sudo tools/hw/linux-host/install-kms-broker.sh [USER] [BROKER]
# USER defaults to $SUDO_USER; BROKER to target/release/rubylight-kms-broker. The group takes
# effect at the user's next sign-in (or `sg rubylight -c COMMAND` meanwhile).
# Undo: systemctl disable --now rubylight-kms.socket; rm /usr/lib/rubylight/rubylight-kms-broker
#   /usr/lib/systemd/system/rubylight-kms.socket /usr/lib/systemd/system/rubylight-kms@.service
set -eu
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
USER_NAME=${1:-${SUDO_USER:-}}
BROKER=${2:-$REPO/target/release/rubylight-kms-broker}
[ "$(id -u)" = 0 ] || { echo "run as root (sudo)" >&2; exit 1; }
[ -x "$BROKER" ] || { echo "no broker at $BROKER: cargo build --release -p rubylight-kms-broker" >&2; exit 1; }
P=$REPO/rust/linux/packaging
install -Dm755 "$BROKER" /usr/lib/rubylight/rubylight-kms-broker
install -Dm644 -t /usr/lib/systemd/system "$P/systemd/rubylight-kms.socket" "$P/systemd/rubylight-kms@.service"
install -Dm644 "$P/sysusers/rubylight.conf" /usr/lib/sysusers.d/rubylight.conf
systemd-sysusers rubylight.conf
systemctl daemon-reload
systemctl enable --now rubylight-kms.socket
if [ -n "$USER_NAME" ]; then
    usermod -aG rubylight "$USER_NAME"
    echo "$USER_NAME is in the rubylight group (from the next sign-in; sg rubylight -c ... now)"
fi
ls -l /run/rubylight/kms.sock

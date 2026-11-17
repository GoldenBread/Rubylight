#!/bin/bash
# Starts PyroWave's GPU decoder in a packaged Nightjar.app (nightjar --check-pyrowave) on an Apple
# silicon runner. Exit 0 from the app: the decoder started on the bundled KosmicKrisp. Exit 2: it
# did not, and the report says why the processor decodes; accepted only when the runner has no GPU
# the driver supports (a virtual GPU without Metal 4). Any other outcome, such as a loader, driver
# or SDK that does not load or start, fails.
# Usage: pyrowave_mac_smoke.sh <Nightjar.app>
set -u
app=${1:?usage: pyrowave_mac_smoke.sh <Nightjar.app>}
report=$(mktemp)
VK_LOADER_DEBUG=error "$app/Contents/MacOS/nightjar" --check-pyrowave "$report"
code=$?
cat "$report"
case $code in
	0) echo "::notice::PyroWave GPU decoding started on KosmicKrisp on this runner" ;;
	2)
		grep -Eq "the processor decodes: (Vulkan lists no devices|the bundled Vulkan driver finds no GPU it supports)" "$report" || exit 1
		echo "::notice::PyroWave falls back to the processor on this runner: it has no Metal 4 GPU"
		;;
	*) exit 1 ;;
esac

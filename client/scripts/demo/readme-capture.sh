#!/bin/bash
# Captures the README's Android pictures and clips from Nightjar's offline demo (a debug build's
# DemoLauncherActivity: a sample library and a bundled test scene, no PC and no network).
#
#   readme-capture.sh <captures>/android [stills] [clips]
#
# Writes raw/<state>.png and clips/<scene>.mp4 for docs/media/build_readme_media.py at the repository root (or the
# Rubylight checkout this folder is mirrored from). adb picks the device from ANDROID_SERIAL; NIGHTJAR_DEMO_PACKAGE
# names a debug build installed under another application ID. A 1920x1080 landscape handheld is assumed: the
# taps and swipes of the on-screen controls clip are in its pixels.
#
# Before capturing, turn off anything the device draws over apps (a handheld's floating FPS counter, for one):
# screencap and screenrecord take those along. Handheld-layout stills keep Android's status bar, which the build
# script crops; the TV layout (--es ui tv) and streams are full screen.
export MSYS_NO_PATHCONV=1
OUT="${1:?captures folder}"; shift
PKG="${NIGHTJAR_DEMO_PACKAGE:-com.butterpollo.client.debug}"
DEMO="$PKG/com.limelight.demo.DemoLauncherActivity"
mkdir -p "$OUT/raw" "$OUT/clips"

want() { [ $# -eq 1 ] && return 0; local name="$1"; shift; for arg in "$@"; do [ "$arg" = "$name" ] && return 0; done; return 1; }
start() { adb shell am force-stop "$PKG"; adb shell am start -n "$DEMO" "$@" >/dev/null; }
shot() { sleep "${2:-5}"; adb exec-out screencap -p > "$OUT/raw/$1.png"; }
key() { for name in "$@"; do adb shell input keyevent "KEYCODE_$name"; sleep 0.6; done; }
record() { adb shell screenrecord --bit-rate 20000000 --time-limit "$2" "/sdcard/readme-$1.mp4" & RECORDER=$!; sleep 1.2; }
finish() { wait "$RECORDER"; sleep 0.5; adb pull "/sdcard/readme-$1.mp4" "$OUT/clips/$1.mp4" >/dev/null; adb shell rm "/sdcard/readme-$1.mp4"; }

adb shell svc power stayon true
adb shell input keyevent KEYCODE_WAKEUP; sleep 1

if want stills "$@"; then
  start --es state home --es host native --es view default; shot home 6
  start --es state home --es host native --es look day --es view default; shot home-day 6
  start --es state home --es host native --es ui tv --es view default; shot tv-home 6
  start --es state detail --es host native --es ui tv; shot tv-detail
  start --es state stream; shot stream 6
  adb shell input keyevent KEYCODE_BACK; shot stream-menu 2
  start --es state statistics; shot stream-stats 6
  start --es state stream --ez modern true; shot stream-modern 6
  start --es state stream --ez pckeyboard true; shot stream-keyboard 6
fi

if want clips "$@"; then
  # Home in the TV layout: the focus crosses two rows of covers, then opens a game's page.
  start --es state home --es host native --es ui tv --es view default; sleep 6
  record tv-home 19
  key DPAD_RIGHT DPAD_RIGHT DPAD_RIGHT
  key DPAD_DOWN DPAD_RIGHT DPAD_RIGHT DPAD_RIGHT
  key DPAD_UP DPAD_LEFT DPAD_LEFT
  key DPAD_CENTER; sleep 4
  finish tv-home
  # A stream: the stream menu opens, the focus moves, the menu closes.
  start --es state stream; sleep 9
  record stream-menu 10
  sleep 1.6; key BACK; sleep 1.2
  key DPAD_RIGHT DPAD_RIGHT DPAD_DOWN; sleep 1.2
  key BACK; sleep 1.5
  finish stream-menu
  # The Modern on-screen controls: both sticks, then two face buttons.
  start --es state stream --ez modern true; sleep 9
  record controls 9
  sleep 1
  adb shell input swipe 280 630 400 560 700; sleep 0.3
  adb shell input swipe 280 630 180 700 700; sleep 0.3
  adb shell input swipe 1230 886 1330 800 700; sleep 0.3
  adb shell input tap 1757 834; sleep 0.4
  adb shell input tap 1640 714; sleep 1.5
  finish controls
fi

adb shell am force-stop "$PKG"
adb shell svc power stayon false
ls "$OUT/raw" "$OUT/clips"

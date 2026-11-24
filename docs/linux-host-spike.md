# Linux host: plan and spike

[Rubylight docs](README.md) › [For developers](README.md#for-developers) · [Architecture](architecture.md) · [Code map](code-map.md) · [Rubylight 3.0 plan](rubylight-3.0-plan.md)

Rubylight hosts on Windows only. The [3.0 plan](rubylight-3.0-plan.md) leaves a Linux host to a later track that starts with a spike: capture and hardware encoding on Mesa, measured before any host code depends on them. This page is that spike's record, and the plan it supports. It is work on a fork (GoldenBread/Rubylight, branch `linux/spike`, started 2026-11-24); nothing here is in a Rubylight release.

## What the spike answers

| Question | Answer |
| --- | --- |
| Can a captured frame reach the encoder without entering system memory? | Yes. A gamescope session's frames arrive on PipeWire as DMA-BUFs and VA-API encodes them where they are. |
| Is hardware HEVC encoding fast enough on Mesa? | Yes at 120 fps: 3.1 ms a frame at 2560x1440 and 5.7 ms at 3840x2160, none over its frame interval. |
| Is it fast enough at 60 fps? | Yes, with the GPU's clocks held up during a stream: 1080p60 HEVC host time p95 1.7 ms instead of 7.0, for about 40 W more GPU power. See [the clock](#the-encoders-clock-follows-its-load). |
| VA-API or Vulkan Video first? | VA-API. It encodes HEVC, 10-bit HEVC and AV1 here. Vulkan Video encodes H.264 and AV1, and its HEVC encoder fails to start through FFmpeg. |

## The machine

AMD Radeon RX 7800 XT (Navi 32), Mesa 26.3.1 (radeonsi, RADV), Linux 7.3.5, KDE Plasma on Wayland, PipeWire 1.6.11, gamescope 3.17.0, libva 1.24.0, FFmpeg 9.1. Every figure below is from a release build of the probes on this PC, 2026-11-24. No other GPU, driver or desktop was run.

## The probes

The probes live in [`rust/linux/probes/`](../rust/linux/probes/Cargo.toml), a workspace of its own, so the Windows host's lock file is untouched. They need the libva, libva-drm and libpipewire development files and clang (for the generated libva bindings).

```sh
cd rust/linux/probes
cargo build --release
./target/release/va_encode --caps                       # what the driver offers
./target/release/va_encode --size 2560x1440 --fps 120 --frames 2400 --paced
gamescope --backend headless -W 2560 -H 1440 -r 120 -- vkcube &
./target/release/pw_capture --target gamescope --format nv12 --fps 120 --frames 2400
```

`va_encode` encodes a loop of 16 test pictures that are on the GPU before timing starts: noise over a gradient, 8-pixel blocks with hard edges, smooth waves and two-pixel stripes, moving 8 pixels a frame. `--paced` submits one picture per frame interval, as a stream does; without it pictures go in back to back. `pw_capture` takes a PipeWire node's frames as DMA-BUFs and encodes each one. Both encode HEVC at constant bitrate with one reference picture and no reordering, write the parameter sets and slice headers themselves, and can keep the bitstream (`--out`) and the per-frame rows (`--csv`).

The times are per frame, after the first second of a run: **submit** is the time inside the calls that hand a picture to the encoder, **encode** runs from the start of the submit to the encoded bytes copied out.

## Encoding

Paced runs, 2400 frames at 120 fps and 1200 at 60 fps:

| Picture | Bitrate | Encode mean | p95 | p99 | max | Over one interval |
| --- | --- | --- | --- | --- | --- | --- |
| 2560x1440, 120 fps, 8-bit | 40 Mbit/s | 3.12 ms | 3.19 | 4.49 | 4.73 | 0 of 2280 |
| 2560x1440, 120 fps, 10-bit | 40 Mbit/s | 3.17 ms | 3.37 | 3.40 | 5.45 | 0 of 2280 |
| 3840x2160, 60 fps, 8-bit | 80 Mbit/s | 5.67 ms | 5.74 | 5.79 | 8.28 | 0 of 1140 |
| 3840x2160, 60 fps, 10-bit | 80 Mbit/s | 5.77 ms | 5.96 | 5.99 | 10.56 | 0 of 1140 |
| 3840x2160, 120 fps, 8-bit | 100 Mbit/s | 5.68 ms | 5.77 | 7.22 | 8.11 | 0 of 2280 |

Submit took 0.01 to 0.02 ms in every run: the calls return at once and the time is the hardware's. Back to back, the encoder reached 362 fps at 2560x1440, 187 fps at 3840x2160 and 599 fps at 1920x1088.

For scale, the Windows host's AMF figures on an RX 7900 XT at 3840x2160 are about 6.3 and 5.2 ms a frame ([Performance](../rust/PERFORMANCE.md), "October 7: AMF split-frame encoding"). That is another GPU and another content, so it shows the same order of size and nothing finer.

Each output was decoded with FFmpeg: every frame present, HEVC Main or Main 10 at level 5.1, no decode error. Against the source pictures the 2560x1440 stream scores 34.6 dB PSNR at 40 Mbit/s; the pictures are hard on purpose.

What Mesa's driver offers for HEVC (`va_encode --caps`): constant, variable, quality-defined variable and constant-QP rate control, one reference picture, intra refresh, regions of interest, a frame size cap, up to 128 slices, 8192x4352 at most. It has no 4:4:4 HEVC encoder.

## Capture

A headless gamescope session runs the game off screen at the size and rate asked for and offers its picture on PipeWire, either as the desktop's BGRx or already converted to NV12. No portal prompt is involved.

| Capture | Arrival rate | Arrival interval p99 | Arrival to bytes mean | p99 |
| --- | --- | --- | --- | --- |
| 2560x1440 at 120 fps, NV12 | 120.0 fps | 8.58 ms | 3.14 ms | 3.21 |
| 2560x1440 at 120 fps, BGRx converted by VA-API | 120.0 fps | 8.44 ms | 4.30 ms | 4.40 |
| 3840x2160 at 120 fps, NV12 | 120.0 fps | 8.64 ms | 5.83 ms | 5.92 |

Every buffer was a DMA-BUF; the probe stops if one is not. Importing a buffer into VA-API took 0.04 ms, once per buffer of the stream's pool of four. Taking NV12 from gamescope costs no conversion on the host's side; converting BGRx with VA-API's video processing adds 1.2 ms at 2560x1440. A decoded frame of the NV12 capture shows the game's picture with the right colours.

Two things the capture probe had to know:

- gamescope describes an NV12 picture by its first plane only. The probe places the chroma plane right after the luma rows, which the decoded frame confirms for these sizes; a size whose layout pads the luma plane would need the real offset.
- gamescope converts with BT.601 unless the consumer asks for BT.709 in the format it offers. The probe asks, since the stream's headers say BT.709.

gamescope's frame timestamps are not on the monotonic clock, so the probe reports no frame age before arrival: the time from the game's present to the buffer's arrival is not measured.

## The encoder's clock follows its load

Paced runs at 2560x1440 on an otherwise idle GPU:

| Frame rate | 120 | 110 | 100 | 90 | 80 | 75 | 60 | 30 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Encode mean, ms | 3.09 | 4.26 | 5.59 | 6.47 | 8.11 | 10.58 | 10.59 | 10.81 |

The kernel reports three clock levels for the encoder (`pp_dpm_vclk`: 24, 513 and 2934 MHz) and leaves the choice to the GPU's firmware. The firmware runs the encoder only as fast as the work needs: at 120 fps it must use the high clock, at 75 fps and below the frame fits at the low one and takes 3.4 times as long. A 60 fps stream at 2560x1440 therefore pays about 7.5 ms more encode time than a 120 fps one. At 3840x2160 and 60 fps the frame sits on the edge: 5.7 ms in the paced run, and between 10 and 22 ms when captured from gamescope, enough to drop the stream to 55.8 fps.

It is not the capture, the imported buffers or the frame rate the encoder is told: the same 10.6 ms shows with test pictures, with gamescope running beside the probe, and with the encoder told 120 fps while frames come at 60.

More load brings the fast clock back. With a second encode running flat out beside it, the 60 fps stream's frames took 2.62 ms. A small 640x368 encode at 240 fps beside it brought them to 8.20 ms only, and a second 60 fps stream beside it left them at 10.2 ms.

### The answer: the GPU's level held at `high` while a stream runs

`rust/linux/graphics/src/encoder_clock.rs` sets `power_dpm_force_performance_level` to `high` when a stream's encoder opens and puts `auto` back when the last one ends, journaled, so a host that crashed restores it at its next start. A level someone else set is left alone. The file is root's; `rust/linux/packaging/udev/70-rubylight-encoder-clock.rules` gives the `video` group that one file. Without the rule the stream runs as before and the console says why.

Release builds, HEVC through gamescope to the headless client, 30 Mbit/s at 1080p and 50 at 2160p, runs interleaved (raised, firmware, firmware, raised; 60 s each at 1080p, 40 s at 2160p), GPU power the mean of `power1_average` over 30 s of each 1080p run:

| | Host time mean / p95 / max, ms | Host output | GPU power |
| --- | --- | --- | --- |
| 1920x1080 60 fps, firmware's clocks | 6.94 / 7.0 / 7.1 | 60 fps | 18.9 W |
| 1920x1080 60 fps, `high` | 1.70 / 1.7 / 1.9 | 60 fps | 58.5 W |
| 3840x2160 60 fps, firmware's clocks | 12.87 / 22.2 / 23.0 | 55.9 fps | |
| 3840x2160 60 fps, `high` | 5.37 / 5.4 / 5.5 | 60 fps | |

The idle GPU drew 15.6 W. At 2160p the headless client's software decoder took 52 fps in both runs; the host's own output rate is the figure above. Forcing only the video clocks (`manual`, then the top index in `pp_dpm_vclk` and `pp_dpm_vclk1`) did not hold them up: the clock files read 24 and 513 MHz during the stream, host time 6.6 ms. `profile_peak` measured the same as `high`, 1.7 ms. Holding `high` costs about 40 W while a stream of a light picture runs; a game that loads the GPU raises its clocks by itself. The setting `vaapi_latency_over_power` (**Full GPU clocks while streaming**, on by default, like NVIDIA's `nvenc_latency_over_power`) turns it off. The measurements above set it off for the firmware's runs.

The Windows host meets the same kind of problem on NVIDIA, where a driver profile keeps the GPU's clocks up while it encodes (`rust/windows/graphics/src/nvidia_profile.rs`).

## VA-API or Vulkan Video

The spike was to time both. It timed VA-API with its own encoder and checked Vulkan Video through FFmpeg, for these reasons:

- FFmpeg's `hevc_vulkan` fails to start on this driver: the sequence header the driver hands back carries a picture order count field out of range (`log2_max_pic_order_cnt_lsb_minus4 out of range: 13`). `h264_vulkan` and `av1_vulkan` encode 2560x1440 and their output decodes. GStreamer ships a Vulkan H.264 encoder and no HEVC one.
- VA-API's submit takes 0.01 to 0.02 ms. What remains is the hardware's time, which a second API over the same hardware cannot shorten by much.

So the first encoder backend is VA-API. Whether RADV's HEVC encoding works when driven directly, without FFmpeg, was not tried. Vulkan Video stays worth a second look later for one reason: the application manages the reference pictures itself, and VA-API on this driver offers HEVC a single reference picture, so recovering from loss by referring to an older picture is not possible there. Intra refresh is.

## Not measured

- The desktop portal's screen capture. It asks the user for permission in a dialog, and nobody was at the PC. KWin's capture offers BGRx with the GPU's tiled layouts; the probe's BGRx path ran against gamescope's linear BGRx only.
- A game. vkcube draws a cube; the encoder's work came from the test pictures.
- HDR, AV1 timing, NVIDIA, Intel, GNOME, wlroots and X11.
- Anything over a network: no packet left the PC.
- Power draw.

## Plan

The portable half of the host already builds on Linux: the five `rust/core/` crates type-check here unchanged. The three `rust/windows/` crates are `#![cfg(windows)]`, the host runtime calls them directly in about 750 places, and `cargo check -p butterpollo-stream` fails on Linux with 565 errors. The work is therefore a platform seam first and Linux backends behind it second.

| Area | Windows today | Linux first | Later |
| --- | --- | --- | --- |
| Capture | WGC, Desktop Duplication, the display driver's frame push | A gamescope session's PipeWire node, as measured here | The desktop portal for the user's own desktop; KMS capture for a PC with nobody signed in |
| Convert | D3D11 and D3D12 shaders from `color.hlsl` | None for gamescope's NV12; VA-API video processing for BGRx | The same shaders on Vulkan, for exact colour parity and HDR |
| Encode | AMF, NVENC, PyroWave | VA-API on Mesa | Vulkan Video, NVENC, PyroWave's Vulkan half |
| Virtual display | Rubylight's display driver | The gamescope session is the display: it takes the client's size and rate | A portal virtual output on KWin and Mutter |
| Input | SendInput, the VHF gamepad driver, ViGEm, USB/IP | XTEST keyboard and mouse into the gamescope session's Xwayland, `uinput` Xbox pads | `uhid` DualSense, Switch and Steam Deck pads from the report descriptors the host already has |
| Audio | WASAPI loopback and routing | PipeWire: a sink per stream, a source for the microphone | Game-only audio by linking nodes |
| Lifecycle | Windows service, installer, tray | A systemd user unit, a udev rule for `uinput` and `uhid` | Packages; the updater off with its reason shown |

Phases, each with the check that ends it:

1. **Spike.** This page.
2. **Platform seam, no Linux feature.** *Done on the fork, 2026-11-24.* `butterpollo-win-graphics`, `-display` and `-io` re-export `rust/linux/graphics`, `display` and `io` on Linux, so the host names one platform and its code is unchanged apart from a small `os_ext` module for its few direct uses of `std::os::windows`. The network layer, processes, files and folders, and Opus are real on Linux; capture, encoding, displays, input injection and audio routing answer that they are not available yet. Every host and test crate builds on Linux with no clippy finding except the Windows installer (`rust/setup`) and the Windows hardware probes (`rust/probes`), and 700 tests pass there: the host's integration suites (`tests/host`), the interop suites against Nightjar's engine (`tests/interop`), and the Linux crates' and configuration's own. Tests of Windows-only behaviour (Nucleus Co-op, Neural Rendering, Playnite, files held open against deletion, tokens, `cmd.exe`) build on Windows only. Two bugs a Linux host would have had were fixed on the way: its TCP ports could not bind for a minute after a restart (no `SO_REUSEADDR` outside Windows), and a shared folder had to be a Windows drive path. The Windows build has not been run since; its changes are the per-module `cfg(windows)`, two dev-dependencies made Windows-only, and one build script line. Original phase description: The host reaches its platform through one facade that is the Windows crates on Windows and new `rust/linux/` crates on Linux, chosen at compile time so the Windows path gains no cost. Windows-only features (Radeon controls, RTSS, Lossless Scaling, Playnite, Nucleus Co-op, RTX HDR) report "unsupported" there. Done when the host and its GPU-free suites (`tests/host`, `tests/interop`) build and pass on Linux and `rust\build.ps1` passes unchanged on Windows.
3. **First stream.** *Done on the fork, 2026-11-24.* On Linux an app's command starts inside a headless gamescope session at the client's size and rate (`os_ext::game_command`, `rust/linux/io/src/gamescope.rs`), and the stream's display is that session; the capture and the keyboard and mouse follow only sessions the host itself started. The capture takes the session's PipeWire node, converts each DMA-BUF into a pooled NV12 surface with VA-API video processing and returns the buffer at once (`rust/linux/graphics/src/capture/`); the encoder is VA-API H.264 High and HEVC Main, 8-bit, CBR, one reference, with the headers written by the host (`rust/linux/graphics/src/vaapi/`). Keyboard and mouse go into the session's Xwayland through XTEST, because a headless gamescope reads no input devices; each controller is a `uinput` Xbox 360 pad. Audio is the default output's monitor through PipeWire. AV1, HDR, 10-bit, 4:4:4, touch, pen, rumble and per-app audio fail visibly or are not offered. Checked with Nightjar's headless client against an isolated host (`tools/hw/linux-host/stream.sh`, `nightjar-cli` through `rust/tests/interop.py`):

   - **30 minutes**, release builds, HEVC 1920x1080 at 60 fps, 30 Mbit/s asked (27.7 Mbit/s on the wire), a test picture and a 440 Hz tone: 107,983 of 107,983 frames decoded, steady 60.000 fps over 1,797 s with no interval over 1.5 periods, arrival interval p99 16.84 ms and max 21.1 ms, one keyframe, the tone received throughout. Host time (capture to sent) p50 6.9 ms, p95 7.0 ms, max 11.2 ms. The Windows host's records on an RX 7900 XT with AMF at the same mode (`rust/PERFORMANCE.md`, the October 26 hold and November 1 records) read host total p95 3.3 to 3.4 ms in clean runs and 4.8 to 5.0 ms in others, so this Linux host, on an RX 7800 XT (the same RDNA 3 encoder generation, not the same card), takes about twice as long. The gap is the encoder's low clock at 60 fps described above, which phase 4 has to answer; it is not hidden by a lower frame rate or bitrate here.
   - **Shorter runs**, debug builds: H.264 and HEVC at 1280x720 and 1920x1080, 60 fps, every frame decoded.
   - **Stock Moonlight** 6.2.0 (moonlight-qt, Arch package), paired with a PIN and streaming inside a headless gamescope of its own (`tools/hw/linux-host/moonlight.sh`), release host: HEVC 1920x1080 at 60 fps for 30 s and H.264 for 20 s, both decoded by VA-API, 0.00 % of frames dropped by the network or jitter, the host at 60 fps with host time p95 6.7 ms (HEVC) and 7.2 ms (H.264), audio connected, and the app quit from Moonlight. moonlight-qt's `pair` command stored the pairing but did not exit afterwards; the script waits for the host's "paired" line instead.
   - **Input in a stream**: with `NIGHTJAR_CLI_INPUT_SCRIPT` the client sent a key, a click, a move, a scroll click and an Xbox pad's A button and left stick; `input_recorder` (`rust/linux/probes`), the stream's app, received each of them: X key and button events, the move as `motion 10 5`, and the pad's BTN_SOUTH and ABS_X.

   The stream crate's unit tests now pass on Linux too: tests of Windows-only behaviour are gated to Windows and the others' fixtures use the OS's shell. Original phase description: A gamescope session, SDR 8-bit 4:2:0, HEVC and H.264, stereo audio, keyboard, mouse and one Xbox pad, to stock Moonlight and to Nightjar. Done with a 30-minute stream from this PC and its timing record beside a Windows one.
4. **Parity that matters.** *In progress on the fork, 2026-11-24.* Built, with what was checked on this PC (RX 7800 XT, Mesa 26.3 radeonsi, KDE Plasma 6 on Wayland, PipeWire 1.6):

   - **AV1** (VA-API, profile 0, 8-bit; headers written by the host, a temporal delimiter added to each unit): stock Moonlight 6.2.0 streamed it at 1920x1080 60 fps for 20 s with no dropped frame, host time p95 4.1 ms. VCN 4 codes 1080 rows as 1082 and says 1080 in the frame header; Moonlight reported the stream as 1920x1080 and showed no decode error.
   - **Loss recovery** for H.264, HEVC and AV1: four reconstructed pictures are kept and the picture after a loss refers to the newest one before it, else the host asks for a keyframe (reference frame invalidation, as NVENC answers it). A test drops reported pictures from the stream and the rest decodes bit-identical to the uninterrupted stream. Encode time unchanged (1080p, about 3.1 ms unpaced).
   - **10-bit**: HEVC Main10 and AV1 10-bit with HDR10 metadata encode and decode, but the host offers no HDR codec, as neither capture gives an HDR picture:
     - gamescope 3.17's PipeWire stream offers BGRx and NV12 only (`build_format_params` in `src/pipewire.cpp`) and `paint_pipewire` encodes it with a gamma 2.2 transfer and the SDR lookup tables, whatever `--hdr-*` options are given. Checked with `--hdr-enabled --hdr-debug-force-output` on the headless backend: the node still offered BGRx and NV12, BT.601/709. Only gamescope's screenshot path writes HDR (PQ AVIF, a 10-bit capture texture).
     - KWin 6.8.3's ScreenCast offers 8-bit formats only (`supportedFormats` in `src/plugins/screencast/screencaststream.cpp`; the portal node offered BGRA/BGRx). Not checked with an output in HDR mode: this PC's active monitor is not HDR-capable.
     - What would give one: a gamescope patched to send its HDR screenshot path's 10-bit PQ picture over PipeWire (a patch upstream, or a gamescope shipped with the host), or reading the KMS scanout plane, which needs `CAP_SYS_ADMIN`. Neither is built.
   - **PyroWave**: the SDK's shared library (`client/scripts/build-pyrowave-sdk.sh`) beside the host, frames in as DMA-BUF from the capture's VA surfaces; the host advertises 4:2:0 and 4:4:4 SDR. Encode and decode checked with the SDK's own decoder (Y 36.9 dB on a hard test picture at 400 Mbit/s); no stream yet, as the headless client does not decode PyroWave.
   - **Controllers**: rumble on the `uinput` Xbox pad (force feedback uploads become the client's rumble reports); over `uhid` the pads the Windows host presents that the kernel has a driver for, chosen as Windows chooses them (`pad_choice.rs`): a DualSense and a DualShock 4 (hid-playstation; motion, touchpad, rumble, lightbar, and the DualSense's trigger effects), a Switch Pro Controller (hid-nintendo; sticks, gyro, rumble, from the Windows driver's profile, which now also answers hid-nintendo's one-stick calibration reads), a Steam Deck (hid-steam, from the model the Windows host serves over USB/IP) and a Steam Controller 2026 (hid-steam; trackpads, grips, Steam's haptics forwarded or turned into rumble). hid-steam keeps these two in lizard mode until Menu is held, so the pad holds it for the driver (`lizard.rs`) unless Steam runs, which takes them over through hidraw as on a real controller; any program that opens a Valve controller's hidraw node does the same, and the kernel then removes its input devices until it lets go. A Switch 2 Pro Controller (057e:2069) has no kernel HID driver, so it is a Switch Pro Controller with a stream warning; the Xbox One and Series HID pads stay the `uinput` Xbox 360 pad, which games read better than a custom HID Xbox. A pad whose driver does not bind within 6 s is dropped with a warning naming the driver, and the next pad the policy gives takes its place. Games need `rust/linux/packaging/udev/71-rubylight-pads.rules` for the motion and touchpad devices.
   - **Microphone**: a PipeWire "Rubylight Microphone" source the client's microphone plays into, the default source while a stream runs and restored after, journaled.
   - **The user's desktop**: an app without a command streams the desktop through the ScreenCast portal (`desktop`, `desktop:N`; DMA-BUF, else memory buffers), with keyboard and mouse through `uinput`. Streamed once the portal was answered (`APP=desktop tools/hw/linux-host/stream.sh hevc 1920 1080 60 20`): 639 of 639 frames decoded, host time p95 1.7 ms, without the dialog the second time. KDE's portal sends a picture when the desktop changes: with a 60 fps test picture on the desktop the stream ran at a steady 60.000 fps for 23 s, 1,500 of 1,500 frames decoded, host p95 1.7 ms.
   - **Libraries**: Steam (3 games here), Heroic (Epic, GOG, Amazon, sideloaded; 1 here) and Lutris. A launcher hands its game to an instance already running, so a library game streams the desktop the launcher runs on (`APP=steam:921590 tools/hw/linux-host/stream.sh hevc 1920 1080 60 180`: DISSIDIA FINAL FANTASY NT Free Edition, started by Steam, which was not running, under Proton Experimental; 10,409 of 10,409 frames decoded over 3 minutes, host p95 1.7 ms, 58 fps). The client's quit ends the game: the host's game tracker follows the new processes whose program is in the game's folder, and a Windows program under Wine or Proton now reports that program (`Z:\\...\\dffnt.exe` through the prefix's drive links) rather than the Wine loader. Checked: the quit asked the one `dffnt.exe` process to end, and it was gone within the second. A Wine process starts as the loader (`wine64`, a Unix command line) and takes the game's name and Windows path about 20 ms later, so the tracker reads a process again when its name changes; before that, a game started while Steam was closed was missed. Checked: with Steam closed, the game started, was tracked, and when it ended mid-stream the host logged the game's exit 11 s later (its 10 s grace for a game that restarts itself) and ended the stream. Not yet checked: a Heroic or Lutris game.
   - **Clipboard** (gamescope's X selections; the desktop through `wl-clipboard`), a logind sleep inhibitor while streaming, suspend and power actions, and session end.
   - **The encoder's clock**: the GPU's level held at `high` while a stream runs: 1080p60 host time p95 7.0 → 1.7 ms, and 2160p60 from 55.9 fps to 60 fps, for about 40 W ([the clock](#the-encoders-clock-follows-its-load)).

      - **Library games** stream the desktop, where their launcher (Steam, Heroic, Lutris) starts them; a game with its own command streams its gamescope session.

   Left in this phase: HDR waits for an HDR capture source (above). Original phase description: HDR and 10-bit, the `uhid` controllers with rumble and motion, the microphone, intra refresh for loss recovery, AV1, PyroWave, the user's own desktop through the portal, library import from Steam, Heroic and Lutris, clipboard, sleep and wake.
5. **Ship.** Packages, a Linux CI workflow, the hardware matrix with only what ran, and capability flags so a client sees only what a Linux host offers.

Out of scope: Radeon controls and AMD Direct Capture (ADLX has no Linux counterpart), the RTSS limiter, Lossless Scaling, Playnite, Nucleus Co-op, RTX HDR and RTX Video, the 1000 Hz VRR virtual display, and Intel.

Open before phase 2 can land upstream: the maintainer's agreement. The seam touches about 750 call sites in crates that are being restructured, and the 3.0 plan gives each 3.x track a go-ahead of its own.

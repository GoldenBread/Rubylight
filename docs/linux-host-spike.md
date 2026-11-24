# Linux host: plan and spike

[Rubylight docs](README.md) › [For developers](README.md#for-developers) · [Architecture](architecture.md) · [Code map](code-map.md) · [Rubylight 3.0 plan](rubylight-3.0-plan.md)

Rubylight hosts on Windows only. The [3.0 plan](rubylight-3.0-plan.md) leaves a Linux host to a later track that starts with a spike: capture and hardware encoding on Mesa, measured before any host code depends on them. This page is that spike's record, and the plan it supports. It is work on a fork (GoldenBread/Rubylight, branch `linux/spike`, started 2026-11-24); nothing here is in a Rubylight release.

## What the spike answers

| Question | Answer |
| --- | --- |
| Can a captured frame reach the encoder without entering system memory? | Yes. A gamescope session's frames arrive on PipeWire as DMA-BUFs and VA-API encodes them where they are. |
| Is hardware HEVC encoding fast enough on Mesa? | Yes at 120 fps: 3.1 ms a frame at 2560x1440 and 5.7 ms at 3840x2160, none over its frame interval. |
| Is it fast enough at 60 fps? | Not without work. The encoder's clock follows its load, and a 2560x1440 frame takes 10.6 ms at 75 fps and below. See [the clock](#the-encoders-clock-follows-its-load). |
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

This needs an answer before a Linux host can claim low latency at 60 fps. The candidates, none of them built or measured yet:

- Raise the clock from a privileged helper while a stream runs (`power_dpm_force_performance_level`, or a power profile), and put it back at the stream's end, journaled like every other change the host makes to the PC.
- Keep the encoder loaded with work of the host's own. It costs power, and the figures above say a little load is not enough.
- Leave it, and state the encode time honestly per frame rate.

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
| Input | SendInput, the VHF gamepad driver, ViGEm, USB/IP | `uinput` keyboard, mouse, touch and an Xbox pad | `uhid` DualSense, Switch and Steam Deck pads from the report descriptors the host already has |
| Audio | WASAPI loopback and routing | PipeWire: a sink per stream, a source for the microphone | Game-only audio by linking nodes |
| Lifecycle | Windows service, installer, tray | A systemd user unit, a udev rule for `uinput` and `uhid` | Packages; the updater off with its reason shown |

Phases, each with the check that ends it:

1. **Spike.** This page.
2. **Platform seam, no Linux feature.** The host reaches its platform through one facade that is the Windows crates on Windows and new `rust/linux/` crates on Linux, chosen at compile time so the Windows path gains no cost. Windows-only features (Radeon controls, RTSS, Lossless Scaling, Playnite, Nucleus Co-op, RTX HDR) report "unsupported" there. Done when the host and its GPU-free suites (`tests/host`, `tests/interop`) build and pass on Linux and `rust\build.ps1` passes unchanged on Windows.
3. **First stream.** A gamescope session, SDR 8-bit 4:2:0, HEVC and H.264, stereo audio, keyboard, mouse and one Xbox pad, to stock Moonlight and to Nightjar. Done with a 30-minute stream from this PC and its timing record beside a Windows one.
4. **Parity that matters.** HDR and 10-bit, the `uhid` controllers with rumble and motion, the microphone, intra refresh for loss recovery, AV1, PyroWave, the user's own desktop through the portal, library import from Steam, Heroic and Lutris, clipboard, sleep and wake.
5. **Ship.** Packages, a Linux CI workflow, the hardware matrix with only what ran, and capability flags so a client sees only what a Linux host offers.

Out of scope: Radeon controls and AMD Direct Capture (ADLX has no Linux counterpart), the RTSS limiter, Lossless Scaling, Playnite, Nucleus Co-op, RTX HDR and RTX Video, the 1000 Hz VRR virtual display, and Intel.

Open before phase 2 can land upstream: the maintainer's agreement. The seam touches about 750 call sites in crates that are being restructured, and the 3.0 plan gives each 3.x track a go-ahead of its own.

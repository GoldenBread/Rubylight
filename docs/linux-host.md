# Linux host: what differs from Windows

The Linux host is a preview on a fork, not a release. It builds from the same code as the Windows host, and a client sees only what a Linux host offers: serverinfo names the platform (`HostPlatform`), and the extras a Linux host cannot serve are left out of its capabilities (`rust/host/stream/src/extensions.rs`, `for_platform`). What ran on which hardware is in [the hardware matrix](hardware-matrix.md#6-linux-host-preview); how each part was built and measured is in [the plan and spike](linux-host-spike.md).

Test PC: an RX 7800 XT on CachyOS, KDE Plasma 6.8 on Wayland, Mesa's VA-API, PipeWire and gamescope 3.17. Clients: Nightjar for Android (an AYN Odin 3 over Wi-Fi), Nightjar's headless client and moonlight-qt.

## The same on both

| Area | On Linux |
| --- | --- |
| Codecs | H.264, HEVC and AV1 through VA-API; packed 4:4:4 (8-bit HEVC and AV1); PyroWave 4:2:0 and 4:4:4 on Vulkan; loss recovery by reference invalidation. |
| What is streamed | A game in a gamescope session of its own, the user's desktop through the ScreenCast portal, and a virtual display for a desktop stream on KDE (Rubylight's VKMS pool). |
| Libraries | Steam, Heroic and Lutris games, with their covers (JPEG, PNG and BMP). The client's quit ends the game, and the stream ends when the game does. |
| Input | Keyboard and mouse; Xbox 360 pads through `uinput`; DualSense, DualShock 4, Switch Pro, Steam Deck and Steam Controller through `uhid`, with rumble. |
| Native touch and pen | On the desktop, a virtual multitouch touchscreen (16 fingers) and a pen tablet with pressure, tilt, two barrel buttons and the eraser, both on `uinput` and tied to the streamed monitor through KWin (`outputName`). In a gamescope session, whose X server has no touch, the first finger and the pen move the pointer and hold its left button. Offered when `native_pen_touch` is on and the user can write `/dev/uinput`. |
| Audio | Stereo from a PipeWire sink per stream (surround not yet streamed to a client); the client's microphone as a PipeWire source. |
| Around the stream | Clipboard, sleep inhibit and power actions (logind), Wake-on-LAN targets in `/serverinfo` and `/wake`, hardware metrics from `amdgpu`'s sysfs, the tray icon in Plasma's system tray with the Windows menu and notifications, the web console. |
| Lifecycle | A systemd user unit, udev rules for input and the encoder clock, an Arch package. |

## Missing on Linux

Each of these could come to Linux; the reason says what stands in the way.

| Feature | Why |
| --- | --- |
| HDR | No capture gives an HDR picture: gamescope 3.17's PipeWire stream and KWin 6.8's screen sharing are 8-bit SDR. The 10-bit encoders are there. |
| Native 4:4:4 | VA-API on this GPU encodes 4:2:0 only. Packed 4:4:4 and PyroWave 4:4:4 give full chroma instead. |
| Text input that follows the focused field, the client's keyboard layout | Not built. Typed text arrives as the keys of the PC's own layout. |
| The client-drawn cursor, display scale, pen barrel roll | Not built; not offered to clients. |
| Frame Warp, the frame limiter | Not built; not offered to clients. |
| Dual screen mode | Not offered: a launch has no second display to give a second screen. |
| The native API's session launcher | Not offered: it reads the PC's window list, which the host does not read on Linux. |
| USB forwarding | Not built; not offered to clients. |
| Window capture | Not built (`rust/linux/graphics/src/capture.rs`). |
| Per-application audio routing | Not built: no program's sound can be moved to another device (`rust/linux/io/src/audio_app_route.rs`). |
| AC3 and E-AC3 passthrough | The encoders need FFmpeg 8 placed beside the host; a system FFmpeg of another version is refused, and the stream uses Opus. |
| Switch 2 Pro Controller | The kernel has no driver for it; it becomes a Switch Pro Controller, with a warning. |
| WebP covers | Not decoded; no cover in this PC's Steam cache is WebP. |
| GNOME, wlroots and X11 desktops | Not built or not run. A gamescope session works without the desktop's help; the desktop, virtual display and KWin capture are KDE Plasma's. |
| NVIDIA and Intel GPUs | Not built or not run: the encoder is Mesa's VA-API, run on AMD only. |
| Updates | The updater is off on Linux and says why; the package manager updates the host. Nightjar does not offer a Linux host the Windows release. |

## Windows only by design

Out of the Linux host's scope, as they rest on Windows or a vendor's Windows driver:

- Radeon controls and AMD Direct Capture (ADLX has no Linux counterpart).
- AMF and NVENC, RTX HDR and RTX Video.
- The RTSS limiter, Lossless Scaling, frame generation, Playnite and Nucleus Co-op.
- The 1000 Hz VRR virtual display.

## Built, not yet run on Linux hardware

- 10-bit HEVC and AV1 streamed to a client (encode and decode tests only).
- Native touch and pen from a client (device tests and KWin's mapping only).
- Lutris games (not installed on the test PC).
- Waking the PC with Wake-on-LAN (the NIC's own wake setting is not read; it needs `CAP_NET_ADMIN`).
- Remote Monitor, and the controllers' buttons from Nightjar for Android (tested from the headless client).

# Rubylight documentation

[Rubylight](../README.md) · [Download](https://github.com/RamazanKara/Rubylight/releases/latest) · [Release notes](../rust/RELEASE_NOTES.md) · [Nightjar docs](../client/docs/index.md)

Rubylight streams games from a Windows 10 or 11 PC to Moonlight-compatible devices: phones, tablets, TVs, handhelds, other PCs and browsers. It is built for AMD Radeon GPUs with a working AMF encoder and NVIDIA GPUs with NVENC. PCs with only Intel graphics use [Rubylight 2.6.x](https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0).

Find your task below. Each guide starts with what to do and links to the [configuration reference](configuration.md) for every setting it names.

<a id="start-here"></a>

## Get started

**First stream:** [install the host](getting-started.md#install-or-run-portable), [open the console](getting-started.md#open-the-console), [pair a device](getting-started.md#pair-a-device-and-start-desktop), then start **Desktop** at 1080p60 in SDR. Already running another host? Read [Coming from another host](getting-started.md#coming-from-another-host) first.

| I want to | Read |
| --- | --- |
| Install Rubylight, or run it without installing | [Install or run portable](getting-started.md#install-or-run-portable) |
| Move from another Moonlight host, or keep it and try Rubylight beside it | [Coming from another host](getting-started.md#coming-from-another-host) · [Install alongside](getting-started.md#keep-another-host-and-install-alongside) · [Import a profile](getting-started.md#bring-an-existing-profile) |
| Pair my first device | [Pair a device and start Desktop](getting-started.md#pair-a-device-and-start-desktop) |
| Pick a client for my device | [Clients](features.md#clients) · [Nightjar](../client/docs/index.md) |
| Update Rubylight | [Upgrading](getting-started.md#upgrading) · [Updates](configuration.md#updates) |

## Everyday use

The console, the game library, your devices and the PC's power. Start with the [everyday use guide](everyday-use.md).

| I want to | Read |
| --- | --- |
| Find my way around the console | [The console](everyday-use.md#the-console) · [How configuration works](configuration.md#how-configuration-works) |
| Add games from Steam, Playnite or other launchers, or my own apps | [Games and apps](everyday-use.md#games-and-apps) · [Game library](features.md#game-library) |
| Leave a game running, end a stream or close the game | [Start, leave and end a game](everyday-use.md#start-leave-and-end-a-game) · [Session actions](configuration.md#session-actions) |
| Pause a game and pick it up later, from any device | [Quick Suspend](configuration.md#quick-suspend) |
| Save the last moments of a stream as a clip, or record it | [Stream DVR and clips](configuration.md#stream-dvr-and-clips) |
| See the game's cover instead of the desktop while a game starts | [Launch curtain](configuration.md#launch-curtain) |
| Open a menu inside the stream from any client | [Couch menu](configuration.md#couch-menu) |
| Decide what each device may do | [Devices and permissions](everyday-use.md#devices-and-permissions) · [Host control](configuration.md#host-control-for-a-device) |
| Keep each device's own in-game settings, or add frame generation to a game | [Managed game files](configuration.md#managed-game-files) · [Neural Rendering](configuration.md#neural-rendering) |
| Wake or sleep the PC from a device | [Sleep and wake](everyday-use.md#sleep-and-wake) |
| Share the clipboard, files, folders or USB devices | [Sharing with a device](everyday-use.md#sharing-with-a-device) · [USB forwarding](usb-forwarding.md) |
| Back up or move my settings | [Settings backup and restore](configuration.md#settings-backup-and-restore) |
| Let Nightjar or a script control the PC | [Host control, session actions and Radeon settings](configuration.md#host-control-session-actions-and-radeon-settings) · [Webhooks](webhooks.md) |

## Play anywhere

Streaming from outside your home. Start with the [play anywhere guide](play-anywhere.md).

| I want to | Read |
| --- | --- |
| Play away from home with the least setup | [Tailscale or another VPN](play-anywhere.md#tailscale-or-another-vpn) |
| Reach the PC without a VPN | [Automatic port forwarding](play-anywhere.md#automatic-port-forwarding) · [Play over the internet](play-anywhere.md#play-over-the-internet) |
| Understand how a device finds the PC over the internet | [Play over the internet: how it works](internet-play.md) |
| Wake the sleeping PC while I am away | [Wake the PC from away](play-anywhere.md#wake-the-pc-from-away) · [Away wake relay](internet-play.md#away-wake-relay) |
| Keep a phone's stream when it leaves Wi-Fi | [Roaming](configuration.md#roaming) |
| Fix a device that cannot connect from outside | [Clients outside your network cannot connect](troubleshooting.md#clients-outside-your-network-cannot-connect) |

## Play together

Friends on their own devices or in a browser, split-screen co-op, and playing while someone else uses the PC. Start with the [play together guide](play-together.md).

| I want to | Read |
| --- | --- |
| Let a friend's paired device join the game I am playing | [Guests from other devices](multiclient.md) |
| Invite a friend who has no app, through a link | [Guests in the browser](browser-guests.md) |
| Give every player their own picture in a split-screen game | [Co-op with Nucleus Co-op](nucleus-coop.md) |
| Play while someone else uses the PC | [Background streams](nucleus-coop.md#background-streams) · [Background streaming](configuration.md#background-streaming) |
| Stream to a browser or a Samsung TV, or play from a TV browser | [WebRTC server](webrtc.md) · [Play in a TV browser](webrtc.md#play-in-a-tv-browser) |
| Move my game to another of my devices, or stream a second screen | [Your own devices](play-together.md#your-own-devices) · [Displays](features.md#displays) |

## Picture and smoothness

Codecs, HDR, displays, frame pacing and an unsteady network. Start with the [picture and smoothness guide](picture-and-smoothness.md).

| I want to | Read |
| --- | --- |
| Choose a codec, turn on HDR or use PyroWave | [Choose your stream format](getting-started.md#choose-your-stream-format) · [Capture and video](configuration.md#capture-and-video) |
| Stream Dolby Vision, HDR10+ or HLG to a TV | [Codecs, full chroma and HDR](configuration.md#codecs-full-chroma-and-hdr) |
| Pick a latency or quality preset | [Stream presets](configuration.md#stream-presets) |
| Stream only a game's window, or its window while it is in front | [The app's window in place of a display](configuration.md#the-apps-window-in-place-of-a-display) |
| Give each device its own display, or keep my monitors on | [Displays](picture-and-smoothness.md#displays) · [Displays and RTSS](configuration.md#displays-and-rtss) |
| Keep a VRR screen on the game's rhythm | [Real VRR](../client/docs/real-vrr.md) |
| See camera turns sooner with Nightjar | [Frame Warp](frame-warp.md) |
| Cap a game's frame rate | [Displays and RTSS](configuration.md#displays-and-rtss) · [Settings for one app or device](configuration.md#settings-for-one-app-or-device) |
| Keep the stream steady on a slow or lossy link | [An unsteady network](picture-and-smoothness.md#an-unsteady-network) · [Link-rate adaptation](configuration.md#lower-bitrate-when-the-link-cannot-carry-it) |
| Send PCM or Dolby Digital to a receiver, use 7.1.4, or stream only the game's sound | [PCM and Dolby Digital audio](configuration.md#pcm-and-dolby-digital-audio) · [Surround, night mode and headphone surround](configuration.md#surround-night-mode-and-headphone-surround) · [Game-only audio](configuration.md#game-only-audio) |
| Stream from an NVIDIA GPU, or use RTX HDR | [NVENC](nvenc.md) · [RTX HDR and RTX Video enhancement](configuration.md#rtx-hdr-and-rtx-video-enhancement) |
| Change Radeon or NVIDIA driver settings for a game | [Radeon settings per game](configuration.md#radeon-settings-per-game) · [Radeon settings with restore](configuration.md#radeon-settings-with-restore) |

## Controllers

Controllers, haptics, keyboard, mouse, touch and the microphone. Start with the [controllers guide](controllers.md).

| I want to | Read |
| --- | --- |
| Choose the controller games see | [Controller types](controllers.md#controller-types) · [Controllers](configuration.md#controllers) |
| Get DualSense haptics, speaker and microphone, or Steam Deck trackpads and back grips | [DualSense USB audio and haptics](dualsense-haptics.md) |
| Use a mouse games read like a real one, or a touchpad's gestures | [Virtual mouse and touchpad](virtual-mouse.md) |
| Use a controller's motion in an emulator | [Motion server for emulators](configuration.md#motion-server-for-emulators-dsu) |
| Launch Steam games from a Steam Deck with MoonDeck | [MoonDeck](configuration.md#moondeck) |
| Talk through the device's microphone | [Microphone](microphone.md) |
| Use a USB device plugged into my device | [USB forwarding](usb-forwarding.md) |

## Troubleshooting

[Troubleshooting](troubleshooting.md) lists symptoms with their cause and fix. The console's **Help** page reads the host log for you (**Log analysis**) and builds a support bundle.

| Problem | Read |
| --- | --- |
| A device cannot find or pair with the PC | [Moonlight cannot find or pair with the PC](troubleshooting.md#moonlight-cannot-find-or-pair-with-the-pc) |
| Black picture or no display | [Black picture, no display, or WGC fails](troubleshooting.md#black-picture-no-display-or-wgc-fails) |
| The stream stutters, freezes or drops | [The stream breaks up, drops or times out on the network](troubleshooting.md#the-stream-breaks-up-drops-or-times-out-on-the-network) · [Low latency is reported, but motion still stutters](troubleshooting.md#low-latency-is-reported-but-motion-still-stutters) |
| HDR looks wrong | [HDR troubleshooting](troubleshooting.md#hdr-looks-washed-out-too-bright-or-different-between-clients) |
| A controller does nothing | [A controller or other input does nothing](troubleshooting.md#a-controller-or-other-input-does-nothing) |
| The service is stopped or a driver is missing | [A driver is missing or out of date](troubleshooting.md#a-driver-is-missing-or-out-of-date) |
| Monitors stay off after a stream | [Monitors stay on, or the display layout does not return](troubleshooting.md#monitors-stay-on-or-the-display-layout-does-not-return) |
| Report a problem | [Logs and a useful report](troubleshooting.md#logs-and-a-useful-report) |

## Reference

| Page | Contents |
| --- | --- |
| [Configuration](configuration.md) | Every feature's settings by topic, then every setting in the console with its configuration key and default. |
| [Features](features.md) | Everything Rubylight does, by area, with a link to where each is set up. |
| [NVENC](nvenc.md) | The NVIDIA encoder, feature by feature next to AMF. |
| [Privacy](privacy.md) | Every connection the host makes, what it sends, and how to turn it off. |
| [Performance](performance.md) | Measured results with their conditions, what each metric means, and how to measure your own setup. |
| [Hardware matrix](hardware-matrix.md) | What has been tested on each GPU generation, Windows version, feature and client, with the evidence. |
| [Measurement record](../rust/PERFORMANCE.md) | Dated fixtures, raw results, reverted experiments and reproduction steps. |
| [Release notes](../rust/RELEASE_NOTES.md) | Every release, with what was tested. |

<a id="guides"></a><a id="by-task"></a><a id="performance-and-hardware"></a>

## For developers

<a id="reference-for-integrators-and-client-authors"></a>

**Integrations and clients**

| Reference | Contents |
| --- | --- |
| [HTTP API](api.md) | The console API: authentication, CSRF, permissions and endpoints. |
| [Native API v1](native-api.md) | The paired-client API Nightjar uses: capabilities, library, session actions, driver settings, files and USB. |
| [Control messages](control-messages.md) | Rubylight's client-to-host control messages and how a client discovers them. |
| [Hardware metrics](hardware-metrics.md) | CPU and GPU readings for client overlays. |
| [Remote connection route](network-path.md) | The endpoint that tells a client whether its Tailscale route is direct or relayed. |
| [Webhooks](webhooks.md) | Signed event notifications to your own services. |
| [Timing record](timing-record.md) | The per-frame CSV record of capture, encode, send and client times. |
| WebRTC, microphone and Frame Warp protocols | [WebRTC for client authors](webrtc.md#for-client-authors) · [Microphone for client developers](microphone.md#for-client-developers) · [Frame Warp wire formats](frame-warp.md#wire-formats) |

<a id="building-and-internals"></a>

**Building and internals**

| Document | Contents |
| --- | --- |
| [Architecture](architecture.md) | The crates, the frame path from capture to packet, compute queues, HDR and PyroWave. |
| [Code map](code-map.md) | Where each part lives, the files one kind of change touches, and the narrowest test for it. |
| [Build guide](building.md) | Toolchain, pinned dependencies, packaging, the web console and the website. |
| [Developer guide](../rust/README.md) | Workspace layout, isolated profiles, running the host from source and validation tools. |
| [Release signing](code-signing.md) | How the maintainer signs each release and what it signs. |
| [Third-party components](../rust/THIRD_PARTY.md) | Codec SDKs, drivers and their licences. |

<a id="articles"></a>

**Articles**

- [Racing the beam with PyroWave](beam-racing-pyrowave.md): how Rubylight and Nightjar draw a frame before all of it has arrived.

## Engineering notes

Working documents from the project's development: plans, designs, investigations and audits. Each records the state on the date it was written and is not maintained as user documentation; where a note and a guide disagree, the guide is current.

- Status: [the status table](status.md), the living record of each feature. History: [Rubylight 3.0 plan](rubylight-3.0-plan.md).
- Designs: [couch menu](couch-menu-design.md), [encoder focus](encoder-focus-design.md), [Wake-on-LAN, sleep and couch audio](wake-and-audio-design.md), [WebRTC server](webrtc-design.md), [slice delivery](b2-slice-delivery.md).
- Audits: [streaming stability, October 18](stability-audit-2026-10-18.md), [PyroWave and D3D12, October 18](pyrowave-d3d12-audit-2026-10-18.md), [bug hunt, October 28](bug-hunt-2026-10-28.md).
- Work log: [Windows performance work, October 2](../rust/PERFORMANCE_WORK.md).

## Project

| Document | Contents |
| --- | --- |
| [Privacy](privacy.md) | Every connection the host makes, what it sends, and how to turn it off. |
| [Code signing policy](code-signing-policy.md) | Who signs releases and what is signed. |
| [Project history](butterpollo-cpp.md) | The C++ fork that preceded the Rust host, and the [archived C++ references](https://github.com/RamazanKara/Rubylight/blob/2.0.0-rc.23/docs/legacy/README.md). |
| [Contributors](../CONTRIBUTORS.md) | Everyone who helped, with their GitHub names. |
| [License](../LICENSE) and [credits](../NOTICE) | GPL-3.0-only, and the projects Rubylight builds on. |

# Controllers and input

[Rubylight docs](README.md) › [Controllers](README.md#controllers) · [Controllers reference](configuration.md#controllers) · [Troubleshooting](troubleshooting.md#a-controller-or-other-input-does-nothing)

Controllers, keyboards, mice, touch and pens on the device reach games on the PC as if they were plugged in there. The installer sets up the virtual gamepad driver, so a controller works without any setup.

## Controller types

**Controller type** (**All settings → Sound & controls → Controllers**) decides which controller games see. **Automatic**, the default, matches each device's own controller:

| The device has | Games see |
| --- | --- |
| A PlayStation controller | A DualSense, with motion, touchpad, lightbar and adaptive triggers |
| A Switch 2 Pro Controller, with the USB driver installed | A Switch 2 Pro Controller |
| Another Nintendo controller | A Switch Pro Controller |
| Any other controller | An Xbox Series controller, or a DualSense when the device has motion sensors or a touchpad (a Steam Deck, for example) |

Change it when a game or a tool needs something else:

- **A game ignores the controller, or the camera drifts.** Some games read the controller's name. Give that game **DualSense with waveform haptics (USB/IP)** or **Xbox Series** in its own settings, not every controller.
- **Steam Input should see a Steam Controller**, with both trackpads, back grips and motion: choose **Steam Controller**.
- **reWASD or another tool sees no controller.** These look only for USB controllers. Choose **Xbox 360 (ViGEmBus)** or **DualShock 4 (ViGEmBus)** after installing ViGEmBus yourself. ViGEmBus no longer gets updates, and the Xbox 360 pad has no motion or touchpad, so choose these only for such a tool ([reWASD or another tool doesn't see the virtual controller](troubleshooting.md#rewasd-or-another-tool-doesnt-see-the-virtual-controller)).

A device can have its own type under **Devices**, and a game under **Games** ([Controllers](configuration.md#controllers)).

## DualSense haptics, speaker and microphone, Steam Deck trackpads

With the installer's optional **Steam Deck and DualSense USB audio and haptics** driver, a DualSense on the device gets the game's waveform haptics, its speaker, headset jack and microphone, and a Steam Deck appears to Steam as a real Steam Deck controller with trackpads, gyro and back grips. It needs a client that sends them, such as Nightjar ([DualSense USB audio and haptics](dualsense-haptics.md)).

**Haptics from game audio** (off by default) lets a DualSense feel the game's bass and impacts when the game sends no vibration. It is a matter of taste: turn it on and adjust its strength while you play.

## Motion for emulators

**Motion server for emulators (DSU)** (off by default) lets Cemu, Dolphin, Citra and other emulators read each streamed controller's gyroscope and accelerometer. Turn it on only when you use such an emulator ([Motion server for emulators](configuration.md#motion-server-for-emulators-dsu)).

## The couch menu

Hold L3+R3, press Ctrl+Alt+Backspace or hold the top-left corner of the screen to open a menu drawn into the stream, in any Moonlight client: **Back to game**, **Disconnect**, **End session** and **Close game** ([Couch menu](configuration.md#couch-menu)).

## Keyboard, mouse, touch and pen

- **Keyboard.** Keys map with the layout the device reports. On a phone or tablet, the device's keyboard opens when a text field on the PC gets the focus.
- **Mouse.** With the USB driver installed, the mouse is a virtual USB mouse that games and anti-cheat read like a real one; without it, Rubylight uses Windows' input injection, which some games ignore ([Virtual mouse and touchpad](virtual-mouse.md)).
- **Touchpad.** A tablet's or laptop's touchpad in Nightjar becomes a Windows precision touchpad, so two-finger scroll and three- and four-finger gestures work.
- **Touch and pen** reach Windows as touch and pen. **Touch** can turn touches into a mouse or a trackpad instead.
- **Cursor.** A device that can draw the cursor itself moves it without waiting for the video.

See [Keyboard, touch, pen and cursor](configuration.md#keyboard-touch-pen-and-cursor).

## Microphone and USB devices

- **Microphone.** Turn on the microphone in the client; the PC hears it through the Steam Streaming Microphone. Nightjar can also be the PC's microphone with no stream running ([Microphone](microphone.md)).
- **USB devices.** A USB device plugged into the device, such as a flash drive, a USB gamepad or a dongle, can appear on the PC during a stream, with the USB driver installed, **USB forwarding** on and the device's permission. Nightjar for Windows, Linux and Android forwards them ([USB forwarding](usb-forwarding.md)).
- **Steam Deck with MoonDeck.** MoonDeck starts Steam games on the PC from a Steam Deck's Game Mode ([MoonDeck](configuration.md#moondeck)).

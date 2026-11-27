# Everyday use

[Rubylight docs](README.md) › [Everyday use](README.md#everyday-use) · [Configuration](configuration.md) · [Troubleshooting](troubleshooting.md)

What you do with Rubylight once the first stream works: find your way around the console, add games, look after your devices and the PC's power, and keep the host up to date. Every setting named here is in the [configuration reference](configuration.md) with its key and default.

## The console

The console is Rubylight's web page on the PC, at **https://localhost:47990**. **Open Rubylight** in the tray icon's menu opens it too.

| Page | What it is for |
| --- | --- |
| **Home** | While a device streams, the live stage at the top: what plays on which device, the stream's health in one sentence, frame rate, bitrate and host frame time over the last two minutes, and where a new picture's time goes on this PC. Below it running co-op games, guests asking to join, a paused game, the host's own warnings, and your games. |
| **Games** | The game library: launchers, folders, your own apps, covers, and each game's own settings. |
| **Devices** | Pairing, and each paired device's permissions and settings. |
| **Settings** | The few settings most people change: **This PC**, **Picture**, **Sound & controls** and **Games**. |
| **Advanced → All settings** | Every setting, in six tabs with search. **Show config keys** shows the key each guide names. |
| **Advanced → Sessions** | What streams now and the streams that ended, with why each ended. |
| **Advanced → Clips** | Instant replays and recordings saved on this PC, with the live replay and recording controls while a device streams ([Stream DVR and clips](configuration.md#stream-dvr-and-clips)). |
| **Advanced → Troubleshoot** | Restart or quit Rubylight, bring back the saved display layout, a map of the connected displays, and crash reports. |
| **Help** | Connection help for devices, **Log analysis** of the host log, and the support bundle. |

The console follows Windows' light or dark theme, or the one you choose. **Ctrl+K** (⌘K on a Mac) finds any page, game, device, setting or action, such as **Pair a device**, **Sync games now** or **Restart Rubylight**, from anywhere in the console. A setting takes effect when you select **Save changes**; a stream that is already running keeps its settings until it starts again, and a few settings say **Applies after a restart** ([Save, reconnect and restart](configuration.md#save-reconnect-and-restart)).

## Games and apps

Rubylight finds the games your launchers have installed: Steam, Playnite, Epic Games, Battle.net, EA app, Ubisoft Connect, GOG, Xbox and Microsoft Store, and game folders such as `D:\Games`. They appear on **Games** with their covers, and every Moonlight client sees them as apps. **Sync now** reads the launchers again; each launcher can sync by itself and leave out games you name (**All settings → Games**). A game you remove stays out until you restore it.

- **Your own apps.** **Games → Add manually** takes a program, a command with preparation and undo steps, a URL or a document.
- **Covers.** Launchers' own covers are used where they exist. With a SteamGridDB key, Rubylight fetches the rest ([Covers from SteamGridDB](configuration.md#covers-from-steamgriddb)).
- **A game's own settings.** Open a game to give it its own stream, display, input or encoder settings, a frame limit, a graphics driver profile, or its settings files kept per device ([Settings for one app or device](configuration.md#settings-for-one-app-or-device), [Managed game files](configuration.md#managed-game-files)).
- **Frame generation.** A game that ships DLSS frame generation can get Neural Rendering's frame generation from its page, and lose it again ([Neural Rendering](configuration.md#neural-rendering)).
- **Playnite.** Games launched through Playnite get the stream's environment and are brought to the front ([Playnite launch focus](configuration.md#playnite-launch-focus)).
- **A clean start.** With **Launch curtain** on (**Settings → Games**, off by default), Nightjar keeps the game's cover over the stream while the game starts, instead of the desktop, store windows and splash screens, and the PC brings the game to the front when its store's window covers it. A launcher that wants a sign in or an update, or a Windows administrator prompt, shows on the device with **Show PC screen**; the PC never clicks them. Moonlight and other clients see the start as before ([Launch curtain](configuration.md#launch-curtain)).
- **Who decides a setting.** On a game's page under **Per device**, and on a device's page under **Stream settings**, each setting is a card that shows which layer decides it (this game on this device, the game, the device, then the host), every layer's value, and **Reset to inherited**.

## Start, leave and end a game

Start a game from the device: pick it in Nightjar or Moonlight. Starting another game while one runs closes the first only after the player confirms.

There are three ways to stop:

| Action | What you get | Use it when |
| --- | --- | --- |
| **Disconnect** | The stream stops; the game keeps running and waits for you to come back. | You will return to the game, from this device or another. |
| **End session** | The stream ends and the PC's displays and driver settings go back; the game keeps running. | You are done streaming but the game should stay open. |
| **Close game** | As End session, and the game quits. | You are done playing. It needs [host control](#devices-and-permissions). |

Nightjar offers all three in its Control Center. Any Moonlight client reaches them through the [couch menu](configuration.md#couch-menu): hold L3+R3, press Ctrl+Alt+Backspace or hold the top-left corner of the screen. Moonlight's own **Quit** closes the game.

**Pause a game for later.** With **Quick Suspend** set to **When a device asks** (**Settings → Games**, off by default), **Suspend game** on Nightjar's Control Center (**Power**) or on the console's Home pauses the game's own processes, and **Resume game** picks it up later, from the same device or another. A paused game keeps its session for up to two hours after its device leaves. Rubylight refuses, with the reason, a game while anti-cheat runs on the PC, and co-op and guest sessions; if Rubylight stops or crashes while a game is paused, the game is resumed. Leave it off for online games ([Quick Suspend](configuration.md#quick-suspend)).

**Keep a moment.** With **Instant replay** on (**All settings → Picture → Stream DVR and clips**, off by default), the PC keeps the last 30 to 120 seconds of the stream in memory: **Clips** in Nightjar's Control Center or the console's **Clips** page saves them as an MP4 clip, or starts and stops a recording, without encoding again. While it is on, the encoder adds a keyframe every 4 seconds; PyroWave streams and passthrough sound can't be clipped ([Stream DVR and clips](configuration.md#stream-dvr-and-clips)).

**Close the app when the last device disconnects** (**Settings → Games**, off by default) closes a game once no device is left in it. Leave it off if you ever come back to a game you left; with it on, leaving the stream closes the game. Each game can follow this default or choose for itself ([Close apps on disconnect](configuration.md#close-apps-on-disconnect)).

## Devices and permissions

Pair a device from **Devices**, or **Pair a device** on Home or in the tray menu, with the PIN the device shows or a QR code Nightjar scans ([Pair a device](getting-started.md#pair-a-device-and-start-desktop)). Pairing asks what the device may do:

- **Play games** (the default): games, co-op and every kind of input.
- **Watch only**: see the games and watch a running stream, without launching anything.
- **Everything**: also the clipboard and host commands. The first device you pair gets it.

Some permissions are never part of pairing and are granted under **Devices → device → Custom permissions**: **Host control** (close games, change graphics driver settings, install a Rubylight update and manage guests), **Sleep PC**, **Restart and shut down PC**, file transfer, shared folders and USB devices. Grant them to devices you hold yourself ([Host control for a device](configuration.md#host-control-for-a-device)).

Each device can also have its own stream preset, display layout, controller type and encoder overrides, and commands that run when it connects and leaves ([Settings for one app or device](configuration.md#settings-for-one-app-or-device), [Device connect and disconnect commands](configuration.md#device-connect-and-disconnect-commands)). Once your devices are paired, you can turn **Allow pairing** off so no new device can pair.

## Sleep and wake

- **Wake the PC from a device.** **Settings → This PC → Wake this PC from a device** shows whether devices can wake the PC and each network adapter's wake settings. Nightjar and Moonlight wake it with Wake-on-LAN, which works on your home network; from outside it needs a device at home that sends the wake, such as an always-on computer running Nightjar as an away wake relay ([Wake the PC from away](play-anywhere.md#wake-the-pc-from-away)).
- **Sleep from a device.** A device with **Sleep PC** puts the PC to sleep while nothing streams. It needs a PC with S1, S2 or S3 sleep; on a PC with only Modern Standby the console says why it is missing ([Sleep PC and sleep after idle](configuration.md#sleep-pc-and-sleep-after-idle)).
- **Sleep after idle** (off by default) puts the PC to sleep after the minutes you choose without a stream or local use.
- **Lock Windows when the last device leaves** (off by default) locks the PC ten seconds after the last device leaves, so nobody finds it signed in; the next stream shows the lock screen and you sign in from the device ([Lock Windows when the last device leaves](configuration.md#lock-windows-when-the-last-device-leaves)).

## Sharing with a device

Each needs a permission you give the device on **Devices**:

- **Clipboard**: text and images copied on either side reach the other.
- **File transfer** (off by default): send files to the PC and fetch them back ([File transfer](configuration.md#file-transfer)).
- **Shared folders**: browse and download from folders you share, and upload with a write grant ([Shared folders](configuration.md#shared-folders)).
- **USB devices** (off by default): a USB device plugged into the device appears on the PC during a stream ([USB forwarding](usb-forwarding.md)).

[Privacy](privacy.md#your-own-devices) says what each permission lets a device read or change.

## Updates and backups

Rubylight checks for a new release once a day and tells you; it installs only when you choose. **Settings → This PC → Updates** has **Check now**, **Install when idle** and **Install now**. **Install now** ends the running stream while Rubylight restarts; **Install when idle** waits for a quiet minute. Updates is **Tell me** by default; choose **Install automatically** if you would rather never think about updates and accept an update at a time you are not watching ([Updates](configuration.md#updates)).

**Settings backup** puts the settings, apps, covers, device settings, game settings files and webhooks in one file, from the console or the tray; the pairing identity and sign-in go in only when you choose. A restore shows what changes, saves the current settings first and restarts the host ([Settings backup and restore](configuration.md#settings-backup-and-restore)).

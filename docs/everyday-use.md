# Everyday use

[Rubylight docs](README.md) › [Everyday use](README.md#everyday-use) · [Configuration](configuration.md) · [Troubleshooting](troubleshooting.md)

What you do with Rubylight once the first stream works: find your way around the console, add games, look after your devices and the PC's power, and keep the host up to date. Every setting named here is in the [configuration reference](configuration.md) with its key and default.

## The console

The console is Rubylight's web page on the PC, at **https://localhost:47990**. The tray icon's menu opens it too.

| Page | What it is for |
| --- | --- |
| **Home** | The current stream with its stream card, running co-op games, guests asking to join, the host's own warnings, and your games. |
| **Games** | The game library: launchers, folders, your own apps, covers, and each game's own settings. |
| **Devices** | Pairing, and each paired device's permissions and settings. |
| **Settings** | The few settings most people change: **This PC**, **Picture**, **Sound & controls** and **Games**. |
| **Advanced → All settings** | Every setting, in six tabs with search. **Show config keys** shows the key each guide names. |
| **Advanced → Sessions** | What streams now and the streams that ended, with why each ended. |
| **Advanced → Troubleshoot** | Restart or quit Rubylight, bring back the saved display layout, and crash reports. |
| **Help** | Connection help for devices, **Log analysis** of the host log, and the support bundle. |

**Ctrl+K** finds any page, game, device, setting or action, such as **Pair a device**, from anywhere in the console. A setting takes effect when you select **Save changes**; a stream that is already running keeps its settings until it starts again, and a few settings say **Applies after a restart** ([Save, reconnect and restart](configuration.md#save-reconnect-and-restart)).

## Games and apps

Rubylight finds the games your launchers have installed: Steam, Playnite, Epic Games, Battle.net, EA app, Ubisoft Connect, GOG, Xbox and Microsoft Store, and game folders such as `D:\Games`. They appear on **Games** with their covers, and every Moonlight client sees them as apps. **Sync now** reads the launchers again; each launcher can sync by itself and leave out games you name (**All settings → Games**). A game you remove stays out until you restore it.

- **Your own apps.** **Games → Add manually** takes a program, a command with preparation and undo steps, a URL or a document.
- **Covers.** Launchers' own covers are used where they exist. With a SteamGridDB key, Rubylight fetches the rest ([Covers from SteamGridDB](configuration.md#covers-from-steamgriddb)).
- **A game's own settings.** Open a game to give it its own stream, display, input or encoder settings, a frame limit, a graphics driver profile, or its settings files kept per device ([Settings for one app or device](configuration.md#settings-for-one-app-or-device), [Managed game files](configuration.md#managed-game-files)).
- **Frame generation.** A game that ships DLSS frame generation can get Neural Rendering's frame generation from its page, and lose it again ([Neural Rendering](configuration.md#neural-rendering)).
- **Playnite.** Games launched through Playnite get the stream's environment and are brought to the front ([Playnite launch focus](configuration.md#playnite-launch-focus)).

## Start, leave and end a game

Start a game from the device: pick it in Nightjar or Moonlight. Starting another game while one runs closes the first only after the player confirms.

There are three ways to stop:

| Action | What you get | Use it when |
| --- | --- | --- |
| **Disconnect** | The stream stops; the game keeps running and waits for you to come back. | You will return to the game, from this device or another. |
| **End session** | The stream ends and the PC's displays and driver settings go back; the game keeps running. | You are done streaming but the game should stay open. |
| **Close game** | As End session, and the game quits. | You are done playing. It needs [host control](#devices-and-permissions). |

Nightjar offers all three in its session menu. Any Moonlight client reaches them through the [couch menu](configuration.md#couch-menu): hold L3+R3, press Ctrl+Alt+Backspace or hold the top-left corner of the screen. Moonlight's own **Quit** closes the game.

**Close the app when the last device disconnects** (**Settings → Games**, off by default) closes a game once no device is left in it. Leave it off unless you never return to a game you left; with it on, a dropped connection costs you the game ([Close apps on disconnect](configuration.md#close-apps-on-disconnect)).

## Devices and permissions

Pair a device from **Devices**, or **Pair a device** on Home or in the tray menu,, with the PIN the device shows or a QR code Nightjar scans ([Pair a device](getting-started.md#pair-a-device-and-start-desktop)). Pairing asks what the device may do:

- **Play games** (the default): games, co-op and every kind of input.
- **Watch only**: see the games and watch a running stream, without launching anything.
- **Everything**: also the clipboard and host commands. The first device you pair gets it.

Some permissions are never part of pairing and are granted under **Devices → device → Custom permissions**: **Host control** (close games, change graphics driver settings, install a Rubylight update and manage guests), **Sleep PC**, **Restart and shut down PC**, file transfer, shared folders and USB devices. Grant them to devices you hold yourself ([Host control for a device](configuration.md#host-control-for-a-device)).

Each device can also have its own stream preset, display layout, controller type and encoder overrides, and commands that run when it connects and leaves ([Settings for one app or device](configuration.md#settings-for-one-app-or-device), [Device connect and disconnect commands](configuration.md#device-connect-and-disconnect-commands)). Once your devices are paired, you can turn **Allow pairing** off so no new device can pair.

## Sleep and wake

- **Wake the PC from a device.** **Settings → This PC → Wake this PC from a device** shows whether devices can wake the PC and each network adapter's wake settings. Nightjar and Moonlight wake it with Wake-on-LAN, which works on your home network; from outside it needs a device at home that forwards the wake ([Play anywhere](play-anywhere.md#wake-the-pc-from-away)).
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

Rubylight checks for a new release once a day and tells you; it installs only when you choose. **Settings → This PC → Updates** has **Check now**, **Install when idle** and **Install now**. **Install now** ends the running stream while Rubylight restarts; **Install when idle** waits for a quiet minute. **Install updates automatically** is off by default; turn it on if you would rather never think about updates and accept an update at a time you are not watching ([Updates](configuration.md#updates)).

**Settings backup** puts the settings, apps, covers, device settings, game settings files and webhooks in one file, from the console or the tray; the pairing identity and sign-in go in only when you choose. A restore shows what changes, saves the current settings first and restarts the host ([Settings backup and restore](configuration.md#settings-backup-and-restore)).

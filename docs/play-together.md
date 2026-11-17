# Play together

[Rubylight docs](README.md) › [Play together](README.md#play-together) · [Guests from other devices](multiclient.md) · [Guests in the browser](browser-guests.md) · [Co-op and background streaming](nucleus-coop.md)

Rubylight has several ways for more than one person to use the PC. They differ in who sees what and what each person needs.

## Pick the way that fits

| You want | Use | Everyone sees | They need |
| --- | --- | --- | --- |
| A friend joins the game you are playing, on their own device | [Guests from other devices](#guests-from-other-devices) | The same picture | A paired device with Nightjar or Moonlight |
| A friend watches or plays along without installing anything | [Guests in the browser](#guests-in-the-browser) | The same picture | A browser and your link |
| Each player has their own picture in a split-screen game | [Co-op with Nucleus Co-op](#split-screen-co-op) | Their own instance of the game | A paired device each, and a Nucleus Co-op handler for the game |
| You play on a device while someone else uses the PC | [Background streaming](#background-streaming) | You your game, they the desktop | A paired device |
| You move your game to another of your devices, or add a second screen | [Your own devices](#your-own-devices) | Your game | Your paired devices |

## Guests from other devices

A second paired device that joins the running game becomes a **guest** of the device playing: it sees the same picture, hears the same sound and gets a controller of its own as the next player. The keyboard and mouse stay with the player unless they hand them over.

1. Under **All settings → Games → Play together**, set **When another device joins the running game** to **Ask the player** (or **Let in without asking**). It is off by default, so your own devices move between each other without anyone answering.
2. The friend selects the running game in Nightjar and presses **Join**, or starts the running app in Moonlight.
3. You let them in from Nightjar or the console's **Home**, and can change what they may do, kick or block them at any time.

Each guest gets an encoder of its own, so every guest adds load on the GPU. Up to four guests can join. [Guests from other devices](multiclient.md) has the details.

## Guests in the browser

While you stream a game in the console's **Stream** page, an invite link lets up to four friends watch in their browsers, view only or with a controller, or with keyboard and mouse if you allow it. They need no app, account or pairing, and connect straight to your PC with no relay in between. The link expires when you choose, and **Kick** and **Revoke** end a guest's or a link's access.

It needs the WebRTC server on, and guests outside your network need the guest ports to reach the PC. [Guests in the browser](browser-guests.md) has the setup and what must be reachable.

## Split-screen co-op

For games that [Nucleus Co-op](https://github.com/SplitScreen-Me/splitscreenme-nucleus) supports, Rubylight runs one instance of the game for each player and streams each instance's picture, sound and controller to its own device, up to four players. You download Nucleus Co-op yourself; Rubylight shows the game with a **(Co-op)** entry once it has a handler, and **Home** shows who plays in which slot. A player who steps away keeps their slot for two minutes.

Every instance runs the whole game, so co-op needs a PC that can run the game several times at once. [Co-op and background streaming](nucleus-coop.md) has the setup and the limits.

## Background streaming

A device plays its own instance of a game while someone else keeps using the PC: the PC's displays, focus, keyboard and mouse stay with the person at the PC, the game's sound leaves the PC's speakers, and only the device's controllers reach the game. A Windows notification tells the person at the PC which device started which game. Games with anti-cheat can't run this way. See [Background streams](nucleus-coop.md#background-streams) and [Background streaming](configuration.md#background-streaming).

## Your own devices

- **Continue here** (Nightjar) moves the running game's stream to the device in your hand; the game keeps running.
- **Stream here too** (Nightjar) shows the running game on a second device as well.
- **Dual screen** (Nightjar) streams a second display of the PC to the same device.
- **Disconnect** on one device and starting the game on another resumes it there, with any Moonlight client.

See [Displays](features.md#displays) and [Control from a client](features.md#control-from-a-client).

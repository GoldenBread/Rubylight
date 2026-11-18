## desktop
- A settings, identity or PC list file Nightjar cannot read is kept as it is, with a copy beside it, instead of being replaced by defaults or a new pairing key.
- An automatic reconnect keeps the packet size the running stream asked for instead of testing jumbo packets against a PC that already runs it; a manual resume of a running game still tests the path as before.
- Quitting or reconnecting while a stream ends waits for its decoder and audio threads before the window and its GPU device go, so the exit no longer races them.
- Streams pace at the display's refresh rate only when the display reports one, so a faster panel is no longer held to 60 frames a second.
- Changing only the frame rate in the stream menu now takes effect, and stays off with a message when the PC does not apply it.
- A slow trackpad scroll is no longer lost.

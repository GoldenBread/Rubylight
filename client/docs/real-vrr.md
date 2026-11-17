# Real VRR

[Nightjar docs](index.md) › [Picture and smoothness](index.md#picture-and-smoothness)

With **Variable refresh rate** on, the screen refreshes whenever Nightjar shows a frame. Shown the
moment each one is decoded, frames reach the screen in the network's rhythm, not the game's: a game
running a steady 90 fps can arrive 8, 14, 9 and 13 ms apart, because packets never arrive perfectly
evenly. **Real VRR** shows each frame at the moment the game presented it on the PC, plus a short,
steady delay, so the screen refreshes in step with the game.

- **Nearly every frame on the game's rhythm** on a clean link, for a delay of a millisecond or two
  (PyroWave, HEVC and AV1 alike), where frames shown on arrival miss it far more often.
- **Built for Wi-Fi too.** The delay follows the connection: it covers the frames that arrive
  together, a frame much later than the rest is shown when it is ready, and the frames behind a
  late one keep the game's spacing. Over Wi-Fi most frame gaps keep their timestamps for a few
  milliseconds of delay, where frames shown on arrival lose most of them.
- **On by default** whenever Variable refresh rate is on.

## Turning it on or off

Real VRR is the **On** choice of **Variable refresh rate**, in **Settings → Display & image** on
Android and the desktop:

- **Off:** the screen keeps a fixed refresh, and Frame pacing applies.
- **On:** Real VRR. Each frame is shown at the game's timing, and whole.
- **Fastest:** each frame is shown the moment it is decoded, for the lowest latency. On Windows
  and Linux such a frame can tear ([whole frames](#tear-free-vrr-on-windows)).

Up to Nightjar 5.3.0 these were separate switches: Variable refresh rate, Real VRR and, on
Windows, Tear-free VRR. What you chose there is kept: Real VRR on reads as On and Real VRR off as
Fastest. On the desktop, where Tear-free VRR counts too, both on read as On and both off as
Fastest; another mix shows as **Custom** and works as before until you choose one of the three.
settings.json holds the same three values as before (`vrr`, `present_at_host_times`,
`vrr_tear_free`). Settings saved before Real VRR became the default had it turned on once.

What it needs:

- **Windows and Linux:** a display with G-Sync, FreeSync or HDMI VRR turned on, so that Variable
  refresh rate applies. If the display turns out not to refresh when a frame is presented, Nightjar
  waits for the refresh instead, as without VRR, and the overlay says so.
- **Android:** Variable refresh rate. On a screen with adaptive refresh (ARR, Android 16) the
  screen follows the frames; on any other the frames keep the game's times on its refresh grid.
- **macOS:** not on by default. Nightjar's log names the refresh range the screen reports (ProMotion and
  Adaptive-Sync screens have one). On such a screen the environment variable `NIGHTJAR_MAC_VRR=1`
  turns Real VRR on, which has not been measured: if you try it, tell us what you see.
- **A PC:** any Moonlight-compatible host sends a timestamp with every frame, and Real VRR follows
  it. Rubylight stamps each frame with the moment the game presented it; other hosts' stamps follow
  the game as closely as their capture does.

With Variable refresh rate off, the matching choice is Frame pacing's **PC's frame times** (not the
default), on Android and the desktop: it shows each frame on the refresh nearest its time on the PC. See
[refresh rate and VRR](video-and-latency.md#refresh-rate-and-vrr).

<a id="tear-free-vrr-on-windows"></a>
## Whole frames

A VRR screen starts drawing a frame the moment it is presented and needs one refresh to draw it
(4.2 ms at 240 Hz). A frame presented while the screen is still drawing the last one is torn: the
part of the screen not yet drawn shows the new frame, the rest shows it only on the next refresh.
That happens when two frames arrive close together, and often below the screen's range, where the
graphics driver repeats the last frame by itself.

With Variable refresh rate **On**, a frame that arrives while the screen is drawing waits for it
to finish, at most one refresh, and is then shown whole. With **Fastest** it is shown at once,
with tearing. How the wait is done depends on the system:

- **Windows:** before each frame Nightjar reads where the screen is in its drawing, and holds a
  frame until the drawing has finished. A frame that follows another closely is also held until
  that one has reached the screen. A frame whose wait for a scan under way would cost the next
  frame its turn is handed to the swap chain instead: Nightjar presents it with sync interval 1,
  and the swap chain shows it whole at the next refresh, so the wait ends there rather than a
  landing later. Some screens do not always start drawing when a frame reaches them, so where the
  screen is says too little there. When Nightjar sees that at the start of a stream, it hands the
  wait to Windows instead (V-Sync under VRR): a frame that meets the screen
  idle goes out at once, one that arrives while it draws waits for that refresh to end. That
  holds for the rest of the stream and for later streams to the same screen in this run, and
  after Nightjar starts again: the screen is kept by its monitor's device interface path, not the
  output it is plugged into, in the desktop config (`scan_untrusted.json`), so another monitor on
  that output does not inherit V-Sync. The overlay's Pacing line says "tear-free with V-Sync". It
  costs delay now and then: measured on a 240 Hz VRR monitor at 60 fps against showing every
  frame at once, the 95th percentile from present to screen went from 1.2 to 8.3 ms.
  The setting **Whole frames under VRR** in Display & image keeps Nightjar's wait or uses Windows
  V-Sync from the start, for a screen either way; choosing Nightjar's wait forgets a screen kept
  this way, so Automatic learns afresh.
- **Linux, drawing straight to the display** (KMS, without a desktop): Nightjar hands each picture
  to the display in step with it. A picture handed over during a refresh is taken when that
  refresh ends, and starts the next one. Up to Nightjar 5.3.0 every VRR stream there was shown at
  once, with tearing. The log's `kms:` lines say which of the two a stream does and, at its end,
  how long the hand-overs took. It has not been measured on a screen: if you try it, tell us what
  you see.
- **Linux on a Wayland desktop:** the compositor shows whole frames. Fastest presents with tearing
  where the compositor allows it. Up to Nightjar 5.3.0, V-Sync off decided that; its switch is
  hidden while Variable refresh rate is on.
- **Linux under gamescope or X11:** frames are presented at once with both choices. gamescope
  shows them whole unless Steam's **Allow tearing** is on.
- **Android and macOS:** the system shows whole frames with both choices.

## How it works

**On the PC.** For a VRR stream to a virtual display, Rubylight runs that display at 1000 Hz
(unless its **Virtual display refresh** is Off), so each frame is captured the moment the game
draws it rather than on the next refresh of a 120 Hz display. With automatic capture it uses
Windows Graphics Capture and
stamps each frame with the moment the game called present, which it reads from DXGI's event
trace; where that trace is not available, it uses the time Windows composed the frame. That stamp
is the frame's RTP timestamp, so nothing in the stream changes for other clients.

**In Nightjar.** For every frame, Nightjar compares when the frame was ready to show with its time
on the PC. It needs no shared clock: the offset between the two clocks is the same for every frame,
so it cancels out, and slow drift is followed as it goes. How late the last 128 frames were sets the
delay, and each frame is shown at its PC time plus that delay. It works like a jitter buffer that is
measured in milliseconds instead of frames.

Three rules size that delay:

- **It covers the frames that arrive together, not the stragglers.** Sorted by lateness, most frames
  lie within a millisecond or two of each other and a few trail far behind. The delay stops at that
  knee: where waiting 2 ms longer would bring in fewer than 7% of the frames (and never past the
  97th percentile), plus 0.5 ms for the present call itself. Jitter without a tail is covered
  whole; behind a long tail the delay stays near the typical frame, and a frame later than the
  delay is shown when it is ready.
- **A late frame takes the ones behind it along.** Late frames come in bursts: in the Wi-Fi
  recordings the frame after a late one was late 33 to 43% of the time, against 11 to 17% of all
  frames. A frame later than the delay raises the delay to its own lateness at once, at most 6 ms
  above the typical frame, and the delay falls back with a time constant of 0.1 s. One gap is
  longer; the frames behind it keep the game's spacing instead of each jumping on its own.
- **The screen's limit is respected.** Two frames are never shown closer together than the
  display's fastest refresh, and a frame is pushed back by at most one refresh for it.

**Below the screen's range.** A VRR screen waits only so long for a frame: the longest interval
of its range. Then the graphics driver shows the old picture again (low frame rate compensation),
which takes as long as a refresh at the screen's top rate, and a frame that arrives meanwhile tears
or waits. Nightjar leaves those repeats to the driver and the compositor. Timing them itself on
Windows was built and measured, and is not on: see [Limits](#limits).

On the desktop, the overlay's **Pacing** line reads **VRR, PC frame times**, with the average hold,
and how many frames came later than the delay. On Android the decoder line of a PyroWave stream ends in **PC frame times**.

## Measured

A test animation at an uneven, game-like frame rate, streamed at 1080p and 120 fps, keeps far more
of its frames on the game's rhythm with Real VRR than shown on arrival, on the same PC and over
Wi-Fi, for a delay of a few milliseconds; on Windows, whole frames all but end tearing. The
method, the tables and the replays are in [Measurements: Real VRR](measurements.md#real-vrr).

## Limits

- **It adds delay:** a millisecond or two on a clean link and a few milliseconds over Wi-Fi
  ([measured](measurements.md#real-vrr)). That is the price of the steady rhythm; choose **Fastest** if you would rather have each
  frame the moment it is decoded.
- **Over Wi-Fi some frames stay off the rhythm.** A frame later than the delay is shown when it is
  ready, and one more than 6 ms later than the typical frame raises the delay no further: some
  frame gaps stay more than 1 ms off their timestamps.
- **Below the screen's range the graphics driver repeats pictures,** and a frame that arrives
  during such a refresh waits for it to end with **On**, or tears with **Fastest**. Nightjar repeating the picture itself on Windows, timed from the frame rate
  and the screen's scan line, helped only some frame rates and made waiting frames later, so it is
  not on ([measured](measurements.md#real-vrr), [details](../protocol/desktop/README.md)).
- **Frames are shown on arrival** on Windows when **Sharper colored text** streams packed full
  chroma, which it does when the codec's own 4:4:4 is not available on the device or the PC.
- **PyroWave on Android** follows Real VRR on a screen with adaptive refresh. It has not been
  measured on such a screen.
- The rhythm can only be as even as the PC's timestamps. With Rubylight and automatic capture they
  are the game's own present times; a capture method other than Windows Graphics Capture stamps the
  time each frame was captured.

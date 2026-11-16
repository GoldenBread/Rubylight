# Display wake

- **A cold launch wakes the console display.** When a stream starts while the PC's monitor is off and nobody has used the PC for a minute, the host sends a net-zero mouse move on the input desktop and, if the driver still reports no swap chain after a second, does it once more. The host log now records the first-frame wait's swap-chain and ring flags, display path, secure desktop and idle time. Nothing changes when the display is on or the user is active.

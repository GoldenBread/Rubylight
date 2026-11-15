# Fixes from a code review
- **Pairing again after backing out works at once.** A device that left the PIN dialog and asked again was refused as "another device is pairing" for up to five minutes. Only a different device is refused now.
- **An automatic update whose download broke off is tried again** at the next check, where a download cut short used to block that release until the next one.
- **Ending a remote monitor no longer pauses other devices' input.** With Remove remote monitor on device disconnect on, the display change ran in the way of every other client's keyboard, mouse and controller until Windows finished it.
- **Sound returns faster after a capture error**: the capture reopens on the next pass, not up to a second later.
- With two streams running, one ending no longer makes the other set up its frame buffers again. An idle PyroWave stream wakes its sender four times a second, and the bitrate controller for H.264, HEVC and AV1 counts only the packets that reached the network.

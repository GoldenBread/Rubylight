# Play over the internet
- **A device on the direct internet path no longer gets jumbo video packets.** The host measured the tunnel's local route and offered 8,192-byte packets, which then crossed the internet; it now keeps such a stream at Moonlight's remote packet size, as it does for any device on the internet. Seen on the Linux host streaming to Nightjar on a phone's hotspot.

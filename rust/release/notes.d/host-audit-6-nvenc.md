# NVENC bitrate and driver APIs
- **NVENC streams above 800 Mbps keep changing their bitrate.** The change limit is now the stream's own 2 Gbit/s negotiation limit, not a lower ceiling that failed a raise; a request above it uses the maximum and says so.
- **An older NVENC driver API shows a session warning** naming the HDR10 metadata or split-frame encoding it cannot provide, instead of leaving that to the log.

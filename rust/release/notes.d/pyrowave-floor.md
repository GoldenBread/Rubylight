# PyroWave below its floor

- **A PyroWave stream that would start below its minimum bitrate is refused instead of streamed broken.** Below the floor for its resolution and frame rate every frame loses detail severely even on a perfect link, so the launch fails with a message naming the minimum, the rate the stream would get, and the choice between a lower resolution or frame rate and HEVC or AV1. Nightjar shows that reason; a stock Moonlight client shows that the stream could not start, and the log carries the same text. At 4K120 the minimum is 1493 Mbps, at 1080p60 it is 187 Mbps.
- **Codecs: Stream PyroWave below its minimum** (`pyrowave_allow_below_minimum`, off) starts such a stream anyway, for tests or a picture you accept; the existing below-minimum warning still appears.

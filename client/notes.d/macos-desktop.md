## desktop
- macOS: **MetalFX** is a new Upscaling choice on Macs whose GPU supports it (macOS 13 or later). It enlarges hardware-decoded SDR video with Apple's spatial scaler and adds a few milliseconds of delay; the overlay says what keeps it off for a stream.
- macOS: scrolling follows the system's natural scrolling setting, in the launcher and in a stream; Linux Wayland now follows it too.
- macOS: the Debug setting **VideoToolbox recovery** (off by default) repairs a lost frame without waiting for a keyframe.
- PyroWave: when the decoder can't keep up and keeps skipping frames, the stream says so and what to lower.
- macOS: PyroWave can decode on the GPU on Apple silicon Macs with macOS 26 or later; turn it on with **PyroWave decoder** in Settings. Other Macs decode on the processor, and the stream says why.

## desktop
- macOS: **MetalFX** is a new Upscaling choice on Macs whose GPU supports it (macOS 13 or later). It enlarges hardware-decoded SDR video with Apple's spatial scaler and adds a few milliseconds of delay; the overlay says what keeps it off for a stream.
- macOS: scrolling follows the system's natural scrolling setting, in the launcher and in a stream.
- macOS: the Debug setting **VideoToolbox recovery** (off by default) repairs a lost frame without waiting for a keyframe.
- PyroWave: when the decoder can't keep up and keeps skipping frames, the stream says so and what to lower.
- macOS: on Apple silicon with macOS 26 or later, PyroWave decodes on the GPU through the bundled KosmicKrisp Vulkan driver. Other Macs, or a Mac where it can't start, decode on the processor, and the stream says why.

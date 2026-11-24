## en
- Lighter stream receive path with cached encryption keys, faster FEC recovery, and fewer allocations across video, audio, and control queues.
- Smoother Android playback with lower-overhead MediaCodec, Vulkan pacing, GLES and PyroWave shaders, direct JNI callbacks, and AAudio output.

## desktop
- Windows desktop playback reduces per-frame copies and allocations across D3D11 decode and presentation, Direct rendering, AI video enhancement, PyroWave, and WASAPI audio.
- Linux desktop playback streamlines KMS direct scanout, Vulkan Video synchronization and DMA-BUF imports, and NV12/P010 plane uploads.
- macOS desktop playback pools MetalFX ring textures and caches VideoToolbox IOSurface textures to avoid per-frame allocations.

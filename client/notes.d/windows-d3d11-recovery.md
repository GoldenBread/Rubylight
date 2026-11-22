## desktop
- Windows: fixed crashes after a graphics driver reset or Direct3D 11 device loss during a stream or in the launcher (#154).
- Windows: hardware video decoding and decoder checks now run on their own Direct3D 11 video device instead of sharing the renderer's context, preventing driver contention and stalled frames.
- Windows: a stream ended by a graphics driver reset now reconnects automatically once the renderer recovers.

## en
- Stream logs now record why native 4:4:4 was skipped when Sharper colored text falls back to packed full chroma (including the decoder reason and advertised host/decoder codec flags).

## desktop
- Hardware 4:4:4 decoder checks now probe with standard lossy Level 2.1 test frames instead of lossless Level 8.5 bitstreams, so NVIDIA and Intel decoders that support native HEVC 4:4:4 are no longer rejected onto packed full chroma (#169).
- Fixed an internal frame-kind collision between VideoToolbox and D3D11 4:4:4 frames in the desktop decoder probe.

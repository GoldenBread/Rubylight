# Text colour in HDR

- **No magenta tint on small text in HDR.** With HDR on, Windows' ClearType text (File Explorer's file lists, for example) could look magenta on the device: the stream shares one colour between neighbouring pixels, and the host mixed ClearType's red and blue glyph edges by their HDR signal values instead of by their light, which HDR's steep brightness curve turns into a visible cast. The host now mixes them by their light, so text keeps its colour with ClearType on. SDR streams and full-chroma (4:4:4) streams are unchanged.

## desktop
- On Windows, a PyroWave stream whose resolution differs from the display's no longer comes out too dark or too bright: a scaled picture is drawn through the video texture, which scales it in linear light, while a stream at the display's own size keeps the one-pass draw.

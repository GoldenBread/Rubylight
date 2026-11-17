# MetalFX spatial scaling on macOS

MetalFX is an opt-in desktop upscaler on macOS 13 and later. The settings entry is
shown only when `MTLFXSpatialScalerDescriptor.supportsDevice` accepts the default
Metal device; creation checks the renderer's actual device again. Bilinear remains
the default. No SDL or protocol changes are required.

## Supported path

Hardware-decoded NV12 and P010 SDR pictures are imported into a Core Video Metal
texture cache. A compute pass converts the visible picture to nonlinear RGB using
the AVFrame's range and BT.601 or BT.709 matrix. MetalFX's perceptual
spatial scaler writes a private texture (MetalFX requires private storage for its
output), then a blit copies it to a BGRA IOSurface. SDL's native Metal renderer
imports that surface. There is no CPU pixel readback in the streaming path.

A GPU completion wait separates native processing from SDL's independent command
queue. The IOSurfaces come from a ring of four. One is written again only when only
the ring holds it and its IOSurface use count is back to what the ring's own Metal
texture gives it: SDL's texture of the surface, also kept alive by a command buffer
still in flight, raises that count. With all four busy a frame gets a surface of
its own. MetalFX does not flush SDL's renderer first: it reads only the decoded
picture and writes only its own textures. The conversion pipeline is built once per device; the
textures, ring and scaler are reused until dimensions change, and a present without a new picture
(an overlay redraw, a repeated picture) draws the last output again rather than running the pass a
second time. Stream teardown releases the pass before the renderer.

HDR, software-decoded pictures, BT.2020 SDR pictures, streams with Frame Warp on, and
formats that require SDL's GPU renderer retain existing presentation. Native-size and
downscaled pictures do not run MetalFX. A failed scaler creation or submission falls back
for that stream. `metalfx::blocker` decides this for each present from the stream's
settings and the picture, never from Frame Warp's shift of the moment, and the overlay's
upscaling line names the reason (`Bilinear (Frame Warp is on)`, `Bilinear (BT.2020 colour)`, …).
SDL tags a BT.2020 picture as YCbCr BT.2020 and has no RGB space with those primaries, so
MetalFX's RGB output could not be tagged to match it.
MetalFX has no separate sharpness setting; the FSR/SGSR setting is unchanged.

## Local verification, 2026-11-16

Machine: Apple M5, arm64, macOS 27.2.1. Metal API Validation was enabled for the GPU
comparison. Intel bridge compilation for macOS 13 passed, but **Intel runtime
support has not been tested**; the device probe is authoritative on each Mac.

The hardware check decoded the existing HEVC fixtures through VideoToolbox and
ran NV12 and P010 through MetalFX twice. At 256×256 → 512×512, mean sampled RGB
absolute differences from SDL's native conversion were 0.52 and 0.92 levels on an
8-bit scale. Saved output was visually inspected. Existing native imports, FSR 1,
SGSR 1, plane conversion, PyroWave and packed-chroma hardware checks also passed.

A synthetic `testsrc2` frame was encoded as HEVC at QP 38, without frame reordering,
then decoded by VideoToolbox. Five warm-up draws preceded 30 measured repetitions
of the same picture at 1280×720 → 1920×1080:

| Mode | Completed draw + readback median | p95 |
| --- | ---: | ---: |
| MetalFX | 3.766 ms | 4.734 ms |
| FSR 1, sharpness 50 | 1.376 ms | 1.772 ms |
| SGSR 1, sharpness 50 | 0.958 ms | 1.913 ms |

MetalFX's conversion + scaler + blit GPU median was 1.604 ms. These are debug-build
diagnostic measurements **including synchronous readback**, not live-stream
latency. MetalFX was slower in this check. The static, highly compressed test card
showed no obvious color corruption, but does not establish a quality winner or
represent a low-bitrate game's motion. Live game quality, Intel support, and HDR
color processing remain unverified. HDR MetalFX is deliberately not enabled.

## Live stream, 2026-11-16

Same Mac (Apple M5, built-in display, window 3024x1701 for the picture), release build,
Enshrouded streamed from a Windows PC over Wi-Fi at 1920x1080, 120 fps, 8-bit SDR HEVC,
28 Mbps. MetalFX started (`MetalFX upscaling 1920x1080 to 3024x1701`) and ran the whole
stream without falling back. Each mode is one stream of about 70 to 160 one-second samples,
first five skipped:

| Mode | Render median | Render p95 | Shown fps median |
| --- | ---: | ---: | ---: |
| Bilinear | 0.19 ms | 0.22 ms | 114.6 |
| FSR 1, sharpness 50 | 1.62 ms | 2.06 ms | 119.4 |
| MetalFX, first version | 5.59 ms | 7.99 ms | 95.9 |
| MetalFX, ring and no flush | 5.28 ms | 5.59 ms | 119.2 |

Render is the client's own draw of each picture (`render_ms` in the statistics file), up to
the present call. FSR's GPU work is not in it: SDL submits it with the frame and nothing
waits. MetalFX's is, because the client waits for MetalFX's own queue. The first version's
shown rate matched the received rate; the PC sent fewer frames in that scene, so its fps is
not a client loss. The second MetalFX run was 60 s at 1920x1080 to 2880x1620 (a smaller
window).

Stage times of that run, from temporary tracing (5617 frames, first 100 skipped), median and
p95: GPU work (conversion, scaler, copy) 4.64 and 5.12 ms, the wait for it 4.97 and 5.54 ms,
taking a ring surface 0.001 ms (5713 reuses, no surface of its own needed), SDL texture
0.03 ms. The first version spent 2.6 ms median flushing SDL's renderer, which waited for a
drawable before MetalFX could start, and allocated a surface every frame. The same GPU work
measured 0.83 to 0.94 ms in the release-build check below, which keeps the GPU busy; in the
stream the GPU otherwise runs only SDL's frame and the window server, and takes about five
times as long. Running MetalFX on SDL's own command queue would remove the wait, but SDL does
not expose it. End-to-end latency varied more between these Wi-Fi runs than
between modes, so it is not compared. At 120 Hz (8.3 ms per refresh) MetalFX's p95 is close to
a whole refresh. Picture quality was not compared side by side.

Release-build check at 1920x1080 to 2880x1620 (`testsrc2`, libx265 QP 30, no B-frames;
draw + readback for 30 frames after five warm-ups, ring version): MetalFX 4.25 ms median,
6.31 ms p95, GPU 0.94 ms; FSR 1 3.19 and 4.04 ms; SGSR 1 2.44 and 4.20 ms. Metal API
Validation was on and colour differences were unchanged (0.52 and 0.92).

## Reproduce

From `protocol/`, build with `cargo build --locked -p nightjar-desktop --features
demo-library`, then run `target/debug/nightjar --check-metal` on a Mac with a display
and VideoToolbox HEVC decoding. Unsupported devices skip the MetalFX part and keep
the other checks. The integration wrapper is:

```sh
cargo test --locked -p nightjar-desktop --features demo-library --test macos_video -- --ignored --nocapture
```

For the optional larger-frame comparison, generate a one-frame, no-reordering HEVC
file and set `NIGHTJAR_METALFX_FIXTURE` to its path when running `--check-metal`.
The check writes diagnostic BMPs to the system temporary directory. Use
`MTL_DEBUG_LAYER=1` to enable Metal validation. Captures/readback are test-only.

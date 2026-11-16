# Temporal upscaler evaluation

An offline harness that measures Nightjar's upscalers on streamed-video-like input before any
of them reaches the apps. It was written to decide whether a temporal upscaler adapted from
Qualcomm's Snapdragon Game Super Resolution 2 beats SGSR 1 on decoded video. It did not, so
nothing from it ships; the numbers are in `rust/PERFORMANCE.md` ("November 16: a temporal
upscaler for Nightjar, measured offline") and the decision in `docs/status.md`.

Python 3 with NumPy and SciPy, and ffmpeg with libx264 on the PATH.

```
python harness.py selftest                     # alignment and invariants
python harness.py prepare --out DIR            # native clips, encodes, decodes (~10 min)
python harness.py run --out DIR [--clips ...] [--conditions ...] [--variants ...] [--workers N]
python harness.py report --out DIR             # DIR/report.md from DIR/results.json
python harness.py bound --out DIR --clips pan_slow --source raw-area h264-0.50x-area
python harness.py fusion --out DIR --clips pan_slow --source h264-0.50x-area
```

## Footage

All self-generated, 1280x720 at 60 fps. A 7680x4320 world is assembled from ffmpeg's
`mandelbrot` (three regions), `testsrc2`, `life` and `sierpinski` sources rendered at twice
native density and enlarged, two tiles of text drawn with `drawtext`, and a zone plate. Native
frames are 4x4 box averages of the world, so pans move in exact quarter native pixels.

| Clip | Content |
| --- | --- |
| static | no motion |
| pan_slow | 1.25, 0.5 native px per frame (sub-pixel at every scale) |
| pan_int | 2, 2 native px per frame (whole stream pixels at 0.5x) |
| pan_fast | 24, 6 native px per frame |
| pan_vfast | 50 native px per frame, beyond the motion search range |
| object | a 256 px object moving against a slow pan (disocclusion) |
| hud | a static HUD (text panel, health bar, crosshair, 1 px frame) over a pan |
| testsrc2 | ffmpeg `testsrc2` |
| mandel_zoom | a Mandelbrot zoom (non-rigid motion) |
| cuts | five 30-frame shots joined by hard cuts at frames 30, 60, 90 and 120 |

## Conditions

Each clip is downscaled with ffmpeg, encoded with libx264 (`veryfast`, `zerolatency`, no
B-frames, one keyframe, one-frame VBV at the stated rate, 4:2:0), decoded, and upscaled back to
1280x720. Every method reads the same decoded frames.

- `h264-0.50x-area`, `h264-0.67x-area` (headline): area downscale, as the host's own
  downscaler (`rust/windows/src/shaders/color.hlsl`, `resize()`), 0.20 bits per pixel.
- `h264-0.50x-bicubic`: bicubic downscale, 0.20 bits per pixel.
- `h264-0.50x-area-lowrate`: 0.08 bits per pixel.

ffmpeg's `neighbor` filter is not used: at 2:1 it takes the odd source pixel, a quarter stream
pixel off centre.

## Methods

- `bilinear`: GL_LINEAR.
- `sgsr1`, `sgsr1_s0`: a line-for-line NumPy port of `app/src/main/assets/shaders/sgsr.frag`,
  run in GL orientation, at the default sharpness (50, edge sharpness 1.5) and at 0.
- `temporal`: the SGSR 2 adaptation. Motion per 8x8 block from hierarchical block matching on
  the decoded luma (quarter-resolution full search of +-4 texels plus last frame's vector, +-1
  refinements at half and full resolution, a parabolic sub-pixel fit); a match confidence
  (residual against block contrast) stands in for SGSR 2's depth-edge term; zero jitter; mean
  block confidence under 0.25 resets history (scene cut). The upscale is SGSR 2's 2-pass upscale
  pass: 5-tap Lanczos sample weight, neighbourhood box from the same taps, history clamped to it,
  blend `w / (history trust + w)`. The current frame's estimate is SGSR 1's output; history is
  sampled with Catmull-Rom and stored as half floats.
- `temporal_zero_mv`, `temporal_oracle`: the same with zero or exact motion (pans only).
- Variants (`--variants`): `t_detail` accumulates bilinear and adds SGSR 1's edge detail on
  output; `t_accum_sharpen` accumulates bilinear and sharpens on output; others in `VARIANTS`.
- `bound`: the best any jitter-free temporal method could do on a pan: exact global motion,
  every past sample splatted onto the world grid, Gaussian reconstruction and an unsharp mask.
- `fusion`: fusion of the last K decoded frames in screen space, 16 x K fetches per output
  pixel, with exact motion, or with `--estimated` the block matcher's vectors (`--global-motion`:
  each frame pair's median vector).
- `f4_median`, `f4_oracle` (`--variants`): that fusion as a per-frame method over every clip
  (`FusionUpscaler`: 4 frames, one fixed setting).

`results/` holds the reports, bounds and fusion numbers of the 2026-11-16 run.

## Metrics

PSNR over RGB against native, SSIM on luma (Gaussian window, sigma 1.5), flicker (mean absolute
difference between the output's and native's frame-to-frame luma change, 0-255), HF ratio
(Laplacian energy against native; under 1 is softer), 5th-percentile and minimum frame PSNR, and
PSNR of the eight frames after each cut.

`upscalers.fetch_cost` counts texture fetches per pass as a cost proxy.

## Licence

`upscalers.py` adapts the upscale pass of SGSR 2 (commit d926f074,
`sgsr/v2/include/glsl_2_pass_fs/sgsr2_upscale.fs`); Qualcomm's BSD-3-Clause notice is in
`LICENSE-SGSR2.txt` and the repository's `NOTICE`.

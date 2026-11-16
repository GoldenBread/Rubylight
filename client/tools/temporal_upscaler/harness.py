#!/usr/bin/env python3
"""Offline evaluation of Nightjar's upscalers on streamed-video-like input.

  python harness.py prepare --out DIR   build native clips and encode every condition
  python harness.py run --out DIR       upscale with every method, measure, write results.json
  python harness.py report --out DIR    write report.md from results.json
  python harness.py selftest            check alignment and invariants

Native footage is self-generated (see README.md): pans over a 7680x4320 still assembled from
ffmpeg's mandelbrot, testsrc2, life and sierpinski sources, text and a zone plate; a moving
object; a static HUD over a pan; testsrc2 and a mandelbrot zoom; and a clip of hard cuts.
Each clip is downscaled, round-tripped through H.264 at a stream-like bitrate with one-frame
VBV, no B-frames and no keyframes after the first, decoded, and upscaled back to native.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
import sys
import time
from concurrent.futures import ProcessPoolExecutor
from dataclasses import asdict, replace
from pathlib import Path

import numpy as np
from scipy import ndimage

sys.path.insert(0, str(Path(__file__).resolve().parent))
import upscalers as U  # noqa: E402

NW, NH = 1280, 720
FPS = 60
WORLD_TILE = (2560, 1440)
SUPERSAMPLE = 4  # world pixels per native pixel; pans move whole world pixels
FONT = os.environ.get("HARNESS_FONT", "C:/Windows/Fonts/arial.ttf")

CONDITIONS = {
    # name: (scale, downscale kernel, bits per pixel per frame)
    # Area matches the host's own downscaler (rust/windows/src/shaders/color.hlsl resize()).
    # ffmpeg's neighbor filter is not used: at 2:1 it takes the odd source pixel, a quarter
    # stream pixel off centre, which would skew every method.
    "h264-0.50x-area": (0.5, "area", 0.20),
    "h264-0.67x-area": (2.0 / 3.0, "area", 0.20),
    "h264-0.50x-bicubic": (0.5, "bicubic", 0.20),
    "h264-0.50x-area-lowrate": (0.5, "area", 0.08),
}
HEADLINE = ["h264-0.50x-area", "h264-0.67x-area"]
TUNING_CLIPS = ["pan_slow", "object", "testsrc2"]


def stream_size(scale):
    w = int(round(NW * scale / 2)) * 2
    h = int(round(NH * scale / 2)) * 2
    return w, h


# --------------------------------------------------------------------------------------------
# Native footage


def ffmpeg_frames(src: str, w: int, h: int, n: int) -> np.ndarray:
    cmd = ["ffmpeg", "-v", "error", "-f", "lavfi", "-i", src, "-frames:v", str(n),
           "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]
    raw = subprocess.run(cmd, check=True, capture_output=True).stdout
    return np.frombuffer(raw, np.uint8).reshape(n, h, w, 3)


def text_tile(size: int, line_height: int, seed: int) -> np.ndarray:
    tw, th = WORLD_TILE
    rng = np.random.default_rng(seed)
    words = ("stream frame motion vector history clamp luma detail edge texture pixel "
             "health ammo score level map quest inventory 0123456789 ABCDEFGH ijklmnop").split()
    lines = []
    for _ in range(th // line_height + 1):
        lines.append(" ".join(rng.choice(words, 14)))
    path = Path(os.environ.get("TEMP", ".")) / f"harness-text-{seed}.txt"
    path.write_text("\n".join(lines), encoding="ascii")
    esc = lambda p: str(p).replace("\\", "/").replace(":", "\\:")
    src = (f"color=c=0x1c2430:s={tw}x{th},drawtext=fontfile='{esc(FONT)}':textfile='{esc(path)}':"
           f"fontsize={size}:fontcolor=0xf0e8d0:line_spacing={line_height - size}:x=24:y=16")
    return ffmpeg_frames(src, tw, th, 1)[0]


def zone_plate() -> np.ndarray:
    tw, th = WORLD_TILE
    y, x = np.mgrid[0:th, 0:tw].astype(np.float64)
    r2 = (x - tw / 2) ** 2 + (y - th / 2) ** 2
    # Local frequency r / 19200 cycles per world pixel: native Nyquist (1/8) at the tile corner,
    # stream Nyquist at 0.5x halfway out.
    z = 0.5 + 0.5 * np.cos(math.pi * r2 / 19200.0)
    return np.repeat((z * 255).astype(np.uint8)[..., None], 3, -1)


def build_world(cache: Path) -> np.ndarray:
    path = cache / "world.npy"
    if path.exists():
        return np.load(path, mmap_mode="r")
    tw, th = WORLD_TILE
    # Synthetic sources are rendered at twice native density and enlarged to world size, so
    # native frames carry detail up to about their Nyquist limit, as a rendered game frame does;
    # rendered straight at world size they would be dense noise no upscaler could approach.
    big = f",scale={tw}:{th}:flags=lanczos"
    m = ("mandelbrot=s=1280x720:start_x={}:start_y={}:start_scale={}:maxiter=1024" + big).format
    life = ffmpeg_frames("life=s=160x90:mold=10:ratio=0.3:seed=7:death_color=#202020:"
                         "life_color=#e0e0e0", 160, 90, 30)[-1]
    tiles = [
        [ffmpeg_frames(m(-0.743643887037151, 0.131825904205330, 0.006), tw, th, 1)[0],
         text_tile(60, 76, 1), zone_plate()],
        [np.repeat(np.repeat(life, 16, 0), 16, 1),
         ffmpeg_frames(m(-0.10109636384562, 0.95628651080914, 0.02), tw, th, 1)[0],
         ffmpeg_frames("testsrc2=s=1280x720" + big, tw, th, 1)[0]],
        [text_tile(140, 170, 2),
         ffmpeg_frames("sierpinski=s=1280x720:type=carpet:seed=3" + big, tw, th, 1)[0],
         ffmpeg_frames(m(-1.25066, 0.02012, 0.0016), tw, th, 1)[0]],
    ]
    world = np.concatenate([np.concatenate(row, 1) for row in tiles], 0)
    np.save(path, world)
    return world


def world_crop(world, x0, y0):
    ww, wh = NW * SUPERSAMPLE, NH * SUPERSAMPLE
    ys = (np.arange(wh) + y0) % world.shape[0]
    xs = (np.arange(ww) + x0) % world.shape[1]
    return world[ys][:, xs]


def box_down(img):
    s = SUPERSAMPLE
    f = img.reshape(NH, s, NW, s, 3).mean((1, 3), dtype=np.float32)
    return np.rint(f).astype(np.uint8)


def pan(world, start, v, n, sprite=None, hud=None):
    """World-space pan; v in world px/frame, so native speed is v / 4.

    Oracle motion d (native px) satisfies prev(p + d) = cur(p): +v/4 for the background,
    -vs/4 inside the sprite, 0 under the HUD."""
    frames, oracles = [], []
    for t in range(n):
        crop = world_crop(world, start[0] + v[0] * t, start[1] + v[1] * t).copy()
        orc = np.empty((NH, NW, 2), np.float32)
        orc[...] = (v[0] / SUPERSAMPLE, v[1] / SUPERSAMPLE)
        if sprite is not None:
            img, (sx, sy), (vx, vy) = sprite
            x, y = sx + vx * t, sy + vy * t
            crop[y:y + img.shape[0], x:x + img.shape[1]] = img
            s = SUPERSAMPLE
            orc[y // s:(y + img.shape[0]) // s, x // s:(x + img.shape[1]) // s] = (-vx / s, -vy / s)
        frame = box_down(crop)
        if hud is not None:
            alpha, color = hud
            frame = np.rint(frame * (1 - alpha) + color * alpha).astype(np.uint8)
            orc[alpha[..., 0] > 0] = 0.0
        frames.append(frame)
        oracles.append(orc)
    return np.stack(frames), np.stack(oracles)


def make_hud(world):
    alpha = np.zeros((NH, NW, 1), np.float32)
    color = np.zeros((NH, NW, 3), np.float32)
    # Dark panel with crisp text cut from the text tile at native scale.
    text = box_down(world_crop(world, WORLD_TILE[0], 0))[:90, :520].astype(np.float32)
    glyph = (text.mean(-1) > 110).astype(np.float32)
    alpha[30:120, 30:550, 0] = 0.75
    color[30:120, 30:550] = 20.0
    alpha[30:120, 30:550, 0] = np.maximum(alpha[30:120, 30:550, 0], glyph)
    color[30:120, 30:550] = np.where(glyph[..., None] > 0, 245.0, color[30:120, 30:550])
    # Health bar, crosshair and a 1 px frame.
    alpha[660:680, 40:400] = 1.0
    color[660:680, 40:300] = (40, 220, 60)
    color[660:680, 300:400] = (70, 30, 30)
    for (y0, y1, x0, x1) in ((359, 361, 620, 660), (340, 380, 639, 641), (600, 601, 900, 1240),
                             (700, 701, 900, 1240), (600, 701, 900, 901), (600, 701, 1239, 1240)):
        alpha[y0:y1, x0:x1] = 1.0
        color[y0:y1, x0:x1] = 255.0
    return alpha, color


def make_clips(world, n=90):
    clips = {}
    clips["static"] = pan(world, (1200, 400), (0, 0), n)
    clips["pan_slow"] = pan(world, (300, 200), (5, 2), n)
    clips["pan_int"] = pan(world, (100, 1100), (8, 8), n)
    clips["pan_fast"] = pan(world, (0, 600), (96, 24), n)
    clips["pan_vfast"] = pan(world, (2000, 300), (200, 0), n)
    sprite = (np.pad(world[3000:3992, 6000:6992], ((4, 4), (4, 4), (0, 0))), (2400, 800), (-14, 9))
    clips["object"] = pan(world, (500, 1300), (5, 2), n, sprite=sprite)
    clips["hud"] = pan(world, (1700, 900), (7, 3), n, hud=make_hud(world))
    clips["testsrc2"] = (ffmpeg_frames(f"testsrc2=s={NW}x{NH}:r={FPS}", NW, NH, n), None)
    zoom = ("mandelbrot=s={}x{}:r={}:start_x=-0.743643887037151:start_y=0.131825904205330:"
            "start_scale=0.02:end_scale=0.0002:end_pts=30:maxiter=1024,scale={}:{}:flags=lanczos"
            ).format(NW * 3 // 4, NH * 3 // 4, FPS, NW, NH)
    clips["mandel_zoom"] = (ffmpeg_frames(zoom, NW, NH, n), None)
    k = 30
    parts = [pan(world, (2200, 100), (5, 2), k)[0], clips["mandel_zoom"][0][:k],
             pan(world, (0, 1400), (96, 24), k)[0], pan(world, (900, 1300), (0, 0), k)[0],
             clips["testsrc2"][0][:k]]
    clips["cuts"] = (np.concatenate(parts), None)
    return clips


# --------------------------------------------------------------------------------------------
# Encode


def prepare(out: Path):
    out.mkdir(parents=True, exist_ok=True)
    world = build_world(out)
    clips = make_clips(world)
    meta = {"native": [NW, NH], "fps": FPS, "clips": {}, "conditions": {}}
    for name, (frames, oracle) in clips.items():
        d = out / "clips" / name
        d.mkdir(parents=True, exist_ok=True)
        frames.tofile(d / "native.rgb")
        if oracle is not None:
            np.save(d / "oracle.npy", oracle)
        meta["clips"][name] = {"frames": int(frames.shape[0]), "oracle": oracle is not None}
        for cond, (scale, kernel, bpp) in CONDITIONS.items():
            w, h = stream_size(scale)
            kbps = int(w * h * FPS * bpp / 1000)
            enc = d / f"{cond}.mkv"
            cmd = ["ffmpeg", "-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgb24",
                   "-s", f"{NW}x{NH}", "-r", str(FPS), "-i", str(d / "native.rgb"),
                   "-vf", f"scale={w}:{h}:flags={kernel},format=yuv420p",
                   "-c:v", "libx264", "-preset", "veryfast", "-tune", "zerolatency", "-bf", "0",
                   "-g", "100000", "-keyint_min", "100000", "-sc_threshold", "0",
                   "-b:v", f"{kbps}k", "-maxrate", f"{kbps}k", "-bufsize", f"{max(1, kbps // FPS)}k",
                   str(enc)]
            subprocess.run(cmd, check=True)
            subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", str(enc), "-f", "rawvideo",
                            "-pix_fmt", "rgb24", str(d / f"{cond}.rgb")], check=True)
            meta["conditions"][cond] = {"stream": [w, h], "kernel": kernel, "bpp": bpp, "kbps": kbps}
        print("prepared", name, flush=True)
    (out / "meta.json").write_text(json.dumps(meta, indent=1))


# --------------------------------------------------------------------------------------------
# Metrics


def to_y(rgb255):
    return rgb255 @ U.LUMA.astype(np.float64)


def ssim(a, b):
    c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    f = lambda x: ndimage.gaussian_filter(x, 1.5, truncate=3.5)
    ma, mb = f(a), f(b)
    va, vb, cov = f(a * a) - ma * ma, f(b * b) - mb * mb, f(a * b) - ma * mb
    s = ((2 * ma * mb + c1) * (2 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2))
    return float(s.mean())


def hf_energy(y):
    return float(np.mean(ndimage.laplace(y) ** 2))


# --------------------------------------------------------------------------------------------
# Methods


def methods_for(has_oracle: bool):
    base = U.TemporalConfig()
    m = {
        "bilinear": None,
        "sgsr1": ("sgsr1", 1.5),
        "sgsr1_s0": ("sgsr1", 1.0),
        "temporal": base,
        "temporal_zero_mv": replace(base, motion="zero"),
    }
    if has_oracle:
        m["temporal_oracle"] = replace(base, motion="oracle")
    return m


VARIANTS = {
    "t_faithful": dict(current="lanczos5", history_filter="bilinear"),
    "t_bilinear_hist": dict(history_filter="bilinear"),
    "t_lk": dict(subpel="lk"),
    "t_nosubpel": dict(subpel="none"),
    "t_u8": dict(history_format="u8"),
    "t_sharp0": dict(edge_sharpness=1.0),
    "t_minlerp": dict(min_lerp=0.3),
    "t_alpha2": dict(alpha_scale=2.0),
    "t_alpha4": dict(alpha_scale=4.0),
    "t_detail": dict(current="detail", alpha_scale=10.0),
    "t_accum_sharpen": dict(current="bilinear", alpha_scale=15.0, output_usm=2.0),
    "t_accum_sharpen_oracle": dict(current="bilinear", alpha_scale=15.0, output_usm=2.0,
                                   motion="oracle"),
}


def run_job(args):
    out, clip, cond, method_names, variants = args
    out = Path(out)
    meta = json.loads((out / "meta.json").read_text())
    n = meta["clips"][clip]["frames"]
    w, h = meta["conditions"][cond]["stream"]
    d = out / "clips" / clip
    native = np.fromfile(d / "native.rgb", np.uint8).reshape(n, NH, NW, 3)
    dec = np.fromfile(d / f"{cond}.rgb", np.uint8).reshape(n, h, w, 3)
    oracle = np.load(d / "oracle.npy", mmap_mode="r") if meta["clips"][clip]["oracle"] else None
    methods = methods_for(oracle is not None)
    for v in variants:
        cfg = replace(U.TemporalConfig(), **VARIANTS[v])
        if cfg.motion != "oracle" or oracle is not None:
            methods[v] = cfg
    if method_names:
        methods = {k: v for k, v in methods.items() if k in method_names}
    scale = w / NW
    results = {}
    for name, spec in methods.items():
        t0 = time.time()
        temporal = None
        if isinstance(spec, U.TemporalConfig):
            temporal = U.TemporalUpscaler(w, h, NW, NH, spec)
        per = {"psnr": [], "ssim": [], "flicker": [], "hf_ratio": []}
        prev_o = prev_r = None
        for t in range(n):
            src = dec[t].astype(np.float32) / 255.0
            if spec is None:
                o = U.upscale_bilinear(src, NW, NH)
            elif temporal is None:
                o = U.upscale_sgsr1(src, NW, NH, spec[1])
            else:
                orc = None
                if oracle is not None:
                    # Native-pixel field sampled at stream pixel centres, in stream pixels.
                    ys = ((np.arange(h) + 0.5) / scale).astype(int)
                    xs = ((np.arange(w) + 0.5) / scale).astype(int)
                    orc = np.asarray(oracle[t])[ys][:, xs] * scale
                o = temporal.process(src, orc)
            o255 = np.rint(np.clip(o, 0, 1) * 255.0)  # what an 8-bit display shows
            r = native[t].astype(np.float64)
            mse = np.mean((o255 - r) ** 2)
            per["psnr"].append(10 * math.log10(255.0 ** 2 / max(mse, 1e-10)))
            yo, yr = to_y(o255), to_y(r)
            per["ssim"].append(ssim(yo, yr))
            per["hf_ratio"].append(hf_energy(yo) / max(hf_energy(yr), 1e-9))
            if prev_o is not None:
                per["flicker"].append(float(np.mean(np.abs((yo - prev_o) - (yr - prev_r)))))
            prev_o, prev_r = yo, yr
        results[name] = {"per_frame": per, "seconds": round(time.time() - t0, 1)}
        if temporal is not None:
            results[name]["cuts_detected"] = temporal.stats["cuts"]
    return clip, cond, results


def summarize(per):
    p = np.array(per["psnr"])
    return {
        "psnr": float(p.mean()), "psnr_p5": float(np.percentile(p, 5)), "psnr_min": float(p.min()),
        "ssim": float(np.mean(per["ssim"])), "flicker": float(np.mean(per["flicker"])),
        "hf_ratio": float(np.mean(per["hf_ratio"])),
    }


def run(out: Path, clips, conds, method_names, variants, workers):
    meta = json.loads((out / "meta.json").read_text())
    clips = clips or list(meta["clips"])
    conds = conds or list(meta["conditions"])
    jobs = [(str(out), c, k, method_names, variants) for c in clips for k in conds]
    res_path = out / "results.json"
    results = json.loads(res_path.read_text()) if res_path.exists() else {}
    with ProcessPoolExecutor(workers) as pool:
        for clip, cond, r in pool.map(run_job, jobs):
            for name, v in r.items():
                v["summary"] = summarize(v["per_frame"])
                results.setdefault(cond, {}).setdefault(clip, {})[name] = v
            res_path.write_text(json.dumps(results))
            print("done", clip, cond, flush=True)


# --------------------------------------------------------------------------------------------
# Report


def report(out: Path):
    results = json.loads((out / "results.json").read_text())
    meta = json.loads((out / "meta.json").read_text())
    lines = ["# Temporal upscaler evaluation", ""]
    lines.append("Native 1280x720 at 60 fps; PSNR in dB over RGB, SSIM on luma, flicker = mean "
                 "|change of output luma - change of native luma| between frames (0-255, lower is "
                 "steadier), HF = Laplacian energy relative to native (1.0 = as detailed as native).")
    lines.append(f"Tuning clips: {', '.join(TUNING_CLIPS)}; every other clip and condition is held out.")
    lines.append("")
    for cond, by_clip in results.items():
        c = meta["conditions"][cond]
        lines.append(f"## {cond}: stream {c['stream'][0]}x{c['stream'][1]}, {c['kernel']} downscale, "
                     f"{c['kbps']} kbit/s H.264")
        lines.append("")
        names = []
        for r in by_clip.values():
            for k in r:
                if k not in names:
                    names.append(k)
        lines.append("| clip | " + " | ".join(names) + " |")
        lines.append("|---" * (len(names) + 1) + "|")
        agg = {k: [] for k in names}
        for clip, r in by_clip.items():
            cells = []
            for k in names:
                if k in r:
                    s = r[k]["summary"]
                    agg[k].append(s)
                    cells.append(f"{s['psnr']:.2f} / {s['ssim']:.4f} / {s['flicker']:.2f}")
                else:
                    cells.append("")
            lines.append(f"| {clip} | " + " | ".join(cells) + " |")
        cells = []
        for k in names:
            if agg[k]:
                cells.append("**{:.2f} / {:.4f} / {:.2f}**".format(
                    np.mean([s["psnr"] for s in agg[k]]), np.mean([s["ssim"] for s in agg[k]]),
                    np.mean([s["flicker"] for s in agg[k]])))
            else:
                cells.append("")
        lines.append("| mean | " + " | ".join(cells) + " |")
        lines.append("")
        lines.append("Worst-frame PSNR (5th percentile / minimum) and HF ratio:")
        lines.append("")
        lines.append("| clip | " + " | ".join(names) + " |")
        lines.append("|---" * (len(names) + 1) + "|")
        for clip, r in by_clip.items():
            cells = [f"{r[k]['summary']['psnr_p5']:.2f} / {r[k]['summary']['psnr_min']:.2f} / "
                     f"{r[k]['summary']['hf_ratio']:.2f}" if k in r else "" for k in names]
            lines.append(f"| {clip} | " + " | ".join(cells) + " |")
        lines.append("")
        if "cuts" in by_clip:
            lines.append("PSNR of the 8 frames from each cut (frames 30, 60, 90, 120):")
            lines.append("")
            for k in names:
                if k not in by_clip["cuts"]:
                    continue
                p = by_clip["cuts"][k]["per_frame"]["psnr"]
                rows = [" ".join(f"{p[c + i]:.1f}" for i in range(8)) for c in (30, 60, 90, 120)]
                lines.append(f"- {k}: " + " | ".join(rows))
            lines.append("")
    (out / "report.md").write_text("\n".join(lines), encoding="utf-8")
    print(out / "report.md")


# --------------------------------------------------------------------------------------------
# Upper bound: oracle shift-and-add


PAN_VELOCITY = {"pan_slow": (5, 2), "pan_int": (8, 8), "static": (0, 0)}


def raw_downscale(native_rgb: Path, n: int, w: int, h: int, kernel: str) -> np.ndarray:
    """Un-encoded downscale with the same ffmpeg filter, kept in RGB (no codec, no 4:2:0)."""
    cmd = ["ffmpeg", "-v", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{NW}x{NH}",
           "-r", str(FPS), "-i", str(native_rgb), "-frames:v", str(n),
           "-vf", f"scale={w}:{h}:flags={kernel}", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]
    raw = subprocess.run(cmd, check=True, capture_output=True).stdout
    return np.frombuffer(raw, np.uint8).reshape(n, h, w, 3)


def bound(out: Path, clip: str, source: str, frames: int = 41):
    """Best case for any temporal method without jitter: exact global motion and every past
    sample splatted onto the world grid, then Gaussian reconstruction and an unsharp mask.

    `source` is a condition name (encoded) or raw-<kernel> (un-encoded ffmpeg downscale)."""
    meta = json.loads((out / "meta.json").read_text())
    n = meta["clips"][clip]["frames"]
    d = out / "clips" / clip
    native = np.fromfile(d / "native.rgb", np.uint8).reshape(n, NH, NW, 3)
    if source.startswith("raw-"):
        scale = 0.5
        w, h = stream_size(scale)
        dec = raw_downscale(d / "native.rgb", frames, w, h, source[4:])
    else:
        w, h = meta["conditions"][source]["stream"]
        scale = w / NW
        dec = np.fromfile(d / f"{source}.rgb", np.uint8).reshape(n, h, w, 3)
    stride = round(SUPERSAMPLE / scale)  # world pixels per stream pixel
    v = PAN_VELOCITY[clip]
    pad = 32
    S = np.zeros((NH * SUPERSAMPLE + abs(v[1]) * frames + 2 * pad,
                  NW * SUPERSAMPLE + abs(v[0]) * frames + 2 * pad, 3), np.float32)
    Wt = np.zeros(S.shape[:2], np.float32)
    centre = stride / 2 - 0.5  # world index of a stream pixel centre, relative to its cell
    k0 = int(math.floor(centre))
    fr = centre - k0
    psnr = lambda a, b: 10 * math.log10(255.0 ** 2 / np.mean((a - b) ** 2))
    sigmas = (1.0, 1.5, 2.0, 2.5, 3.0)
    usms = (0.0, 0.5, 1.0, 1.5)
    rows = []
    for t in range(frames):
        ox, oy = v[0] * t + pad, v[1] * t + pad
        f = dec[t].astype(np.float32)
        for dy, wy in ((0, 1 - fr), (1, fr)):
            for dx, wx in ((0, 1 - fr), (1, fr)):
                if wy * wx == 0:
                    continue
                ys = slice(oy + k0 + dy, oy + k0 + dy + stride * h, stride)
                xs = slice(ox + k0 + dx, ox + k0 + dx + stride * w, stride)
                S[ys, xs] += wy * wx * f
                Wt[ys, xs] += wy * wx
        if t < 10 or t % 5:
            continue
        r = native[t].astype(np.float64)
        row = {"sgsr1": psnr(np.rint(U.upscale_sgsr1(f / 255, NW, NH, 1.5) * 255), r),
               "bilinear": psnr(np.rint(U.upscale_bilinear(f / 255, NW, NH) * 255), r)}
        sy = slice(oy - pad, oy + NH * SUPERSAMPLE + pad)
        sx = slice(ox - pad, ox + NW * SUPERSAMPLE + pad)
        for sg in sigmas:
            est = ndimage.gaussian_filter(S[sy, sx], (sg, sg, 0), truncate=3.0)
            wt = ndimage.gaussian_filter(Wt[sy, sx], sg, truncate=3.0)
            e = (est / np.maximum(wt, 1e-6)[..., None])[pad:-pad, pad:-pad]
            o = e.reshape(NH, SUPERSAMPLE, NW, SUPERSAMPLE, 3).mean((1, 3))
            blur = ndimage.gaussian_filter(o, (1.0, 1.0, 0))
            for a in usms:
                row[f"s{sg}_u{a}"] = psnr(np.rint(np.clip(o + a * (o - blur), 0, 255)), r)
        rows.append(row)
    mean = {k: float(np.mean([r[k] for r in rows])) for k in rows[0]}
    best = max((k for k in mean if k.startswith("s")), key=mean.get)
    result = {"clip": clip, "source": source, "frames": [10, frames - 1], "sgsr1": mean["sgsr1"],
              "bilinear": mean["bilinear"], "best": best, "best_psnr": mean[best],
              "gain_over_sgsr1": mean[best] - mean["sgsr1"]}
    path = out / "bound.json"
    allres = json.loads(path.read_text()) if path.exists() else {}
    allres[f"{clip}/{source}"] = result
    path.write_text(json.dumps(allres, indent=1))
    print(json.dumps(result))


def fusion(out: Path, clip: str, source: str):
    """Oracle K-frame fusion in screen space: every output pixel gathers 4x4 samples from each of
    the current and K-1 previous decoded frames at their exact motion-compensated positions,
    Gaussian-weighted by distance, then an unsharp mask. What a non-recursive temporal method
    could reach with perfect motion, at 16 * K texture fetches per output pixel."""
    meta = json.loads((out / "meta.json").read_text())
    n = meta["clips"][clip]["frames"]
    d = out / "clips" / clip
    w, h = meta["conditions"][source]["stream"]
    scale = w / NW
    native = np.fromfile(d / "native.rgb", np.uint8).reshape(n, NH, NW, 3)
    dec = np.fromfile(d / f"{source}.rgb", np.uint8).reshape(n, h, w, 3)
    v = PAN_VELOCITY[clip]
    dv = np.array(v, np.float32) / SUPERSAMPLE * scale  # stream px per frame
    ox, oy = U.output_axes(NW, NH)
    px, py = ox * scale, oy * scale
    psnr = lambda a, b: 10 * math.log10(255.0 ** 2 / np.mean((a - b) ** 2))
    res = {}
    for t in range(12, 41, 4):
        r = native[t].astype(np.float64)
        cur = dec[t].astype(np.float32) / 255
        res.setdefault("sgsr1", []).append(psnr(np.rint(U.upscale_sgsr1(cur, NW, NH, 1.5) * 255), r))
        for K in (1, 4, 8):
            for sigma in (0.25, 0.35, 0.5):
                acc = 0.0
                wacc = 0.0
                for k in range(K):
                    f = dec[t - k].astype(np.float32) / 255
                    qx = px + k * dv[0] - 0.5
                    qy = py + k * dv[1] - 0.5
                    x0 = np.floor(qx).astype(np.int64)
                    y0 = np.floor(qy).astype(np.int64)
                    for sy in (-1, 0, 1, 2):
                        for sx in (-1, 0, 1, 2):
                            ddx = (x0 + sx - qx)[None, :]
                            ddy = (y0 + sy - qy)[:, None]
                            wgt = np.exp(-(ddx * ddx + ddy * ddy) / (2 * sigma * sigma))
                            acc = acc + U.fetch_sep(f, y0 + sy, x0 + sx) * wgt[..., None]
                            wacc = wacc + wgt
                o = acc / wacc[..., None]
                for a in (0.0, 1.0, 2.0):
                    oo = np.clip(o + a * (o - U.blur121(o)), 0, 1)
                    res.setdefault(f"K{K}_s{sigma}_u{a}", []).append(psnr(np.rint(oo * 255), r))
    mean = {k: float(np.mean(x)) for k, x in res.items()}
    best = {K: max((k for k in mean if k.startswith(f"K{K}_")), key=mean.get) for K in (1, 4, 8)}
    result = {"clip": clip, "source": source, "sgsr1": mean["sgsr1"],
              **{f"K{K}": {"best": best[K], "psnr": mean[best[K]],
                           "gain_over_sgsr1": mean[best[K]] - mean["sgsr1"]} for K in best}}
    path = out / "fusion.json"
    allres = json.loads(path.read_text()) if path.exists() else {}
    allres[f"{clip}/{source}"] = result
    path.write_text(json.dumps(allres, indent=1))
    print(json.dumps(result))


# --------------------------------------------------------------------------------------------
# Self-test


def selftest():
    rng = np.random.default_rng(1)
    flat = np.full((36, 64, 3), 0.4, np.float32)
    for name, f in (("bilinear", lambda s: U.upscale_bilinear(s, 128, 72)),
                    ("sgsr1", lambda s: U.upscale_sgsr1(s, 128, 72, 1.5))):
        o = f(flat)
        assert np.allclose(o, 0.4, atol=1e-6), name
    # A smooth image downscaled with a 2x2 box and upscaled bilinearly has no global shift.
    y, x = np.mgrid[0:144, 0:256].astype(np.float32)
    img = (0.5 + 0.25 * np.sin(x / 9.0) * np.cos(y / 7.0))[..., None].repeat(3, -1)
    small = img.reshape(72, 2, 128, 2, 3).mean((1, 3))
    up = U.upscale_bilinear(small, 256, 144)
    best = min(((float(np.mean((np.roll(up, (dy, dx), (0, 1)) - img)[8:-8, 8:-8] ** 2)), dx, dy)
                for dx in (-1, 0, 1) for dy in (-1, 0, 1)))
    assert best[1:] == (0, 0), best
    # SGSR 1 must sharpen symmetrically enough to keep the same alignment.
    up = U.upscale_sgsr1(small, 256, 144, 1.5)
    best = min(((float(np.mean((np.roll(up, (dy, dx), (0, 1)) - img)[8:-8, 8:-8] ** 2)), dx, dy)
                for dx in (-1, 0, 1) for dy in (-1, 0, 1)))
    assert best[1:] == (0, 0), best
    # The temporal upscaler's first frame and the first frame after a cut equal SGSR 1.
    frames = [rng.random((36, 64, 3)).astype(np.float32) for _ in range(3)]
    t = U.TemporalUpscaler(64, 36, 128, 72, U.TemporalConfig(history_format="f32"))
    assert np.array_equal(t.process(frames[0]), U.upscale_sgsr1(frames[0], 128, 72, 1.5))
    t.process(frames[0])
    o = t.process(frames[2])  # unrelated noise: every block rejects, history resets
    assert t.stats["cuts"] == 1, t.stats
    assert np.array_equal(o, U.upscale_sgsr1(frames[2], 128, 72, 1.5))
    # Motion estimation finds a known sub-pixel pan.
    big = (0.5 + 0.2 * np.sin(x / 5.0 + y / 11.0) + 0.2 * np.cos(x / 13.0 - y / 4.0)).astype(np.float32)
    a = U.bilinear(big[..., None], x + 0.5, y + 0.5)[..., 0]
    b = U.bilinear(big[..., None], x + 0.5 + 2.25, y + 0.5 - 1.5)[..., 0]
    pa = [a, U.down2(a), U.down2(U.down2(a))]
    pb = [b, U.down2(b), U.down2(U.down2(b))]
    for sub in ("parabola", "lk"):
        m = U.estimate_motion(pb, pa, np.zeros((18, 32, 2), np.float32), U.TemporalConfig(subpel=sub))
        v = np.median(m.vectors[3:-3, 3:-3].reshape(-1, 2), 0)
        assert np.allclose(v, (2.25, -1.5), atol=0.2), (sub, v)
    print("selftest ok")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("command", choices=["prepare", "run", "report", "bound", "fusion", "selftest"])
    ap.add_argument("--source", nargs="*", default=["h264-0.50x-bicubic"])
    ap.add_argument("--out", type=Path)
    ap.add_argument("--clips", nargs="*")
    ap.add_argument("--conditions", nargs="*")
    ap.add_argument("--methods", nargs="*")
    ap.add_argument("--variants", nargs="*", default=[])
    ap.add_argument("--workers", type=int, default=6)
    a = ap.parse_args()
    if a.command == "selftest":
        selftest()
    elif a.command == "prepare":
        prepare(a.out)
    elif a.command == "bound":
        for clip in a.clips or ["pan_slow"]:
            for src in a.source:
                bound(a.out, clip, src)
    elif a.command == "fusion":
        for clip in a.clips or ["pan_slow"]:
            for src in a.source:
                fusion(a.out, clip, src)
    elif a.command == "run":
        run(a.out, a.clips, a.conditions, a.methods, a.variants, a.workers)
    else:
        report(a.out)


if __name__ == "__main__":
    main()

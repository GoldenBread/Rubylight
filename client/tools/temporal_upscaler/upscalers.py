# TemporalUpscaler._upscale and fast_lanczos_sgsr2 adapt Snapdragon Game Super Resolution 2:
# Copyright (c) 2024, Qualcomm Innovation Center, Inc. All rights reserved.
# SPDX-License-Identifier: BSD-3-Clause (full notice: LICENSE-SGSR2.txt).
# The SGSR 1 port (_sgsr1_gl) follows sgsr.frag: Copyright (c) 2025, Qualcomm Innovation Center, Inc.,
# BSD-3-Clause (client/app/src/main/assets/NOTICE-SGSR1.txt).
"""Upscalers evaluated by the harness: bilinear, SGSR 1 and Nightjar's temporal upscaler.

Every function mirrors what the GPU does: float32, clamp-to-edge texel fetches, texel centres at
i + 0.5, and a strictly causal pipeline (the current decoded frame plus state kept from earlier
frames, never a later frame). Images are float32 arrays shaped (rows, columns, channels) in [0, 1].

The SGSR 1 port follows client/app/src/main/assets/shaders/sgsr.frag line by line. That shader
works in GL texture space, where y grows towards the top of the picture, and is not vertically
symmetric, so `sgsr1` flips the picture, runs the port in GL space and flips the result back.

The temporal upscaler adapts the upscale pass of Qualcomm's Snapdragon Game Super Resolution 2
(BSD-3-Clause, github.com/SnapdragonStudios/snapdragon-gsr, commit d926f074,
sgsr/v2/include/glsl_2_pass_fs/sgsr2_upscale.fs). Decoded video has no depth, engine motion
vectors or jitter, so motion comes from block matching on the decoded luma, the depth-edge
term is replaced by a match-confidence term, and the jitter is zero.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field, replace

import numpy as np

LUMA = np.array([0.299, 0.587, 0.114], np.float32)


# --------------------------------------------------------------------------------------------
# GPU-like sampling helpers


def fetch(img: np.ndarray, ys, xs):
    """texelFetch with coordinates clamped to the texture (CLAMP_TO_EDGE)."""
    h, w = img.shape[:2]
    return img[np.clip(ys, 0, h - 1), np.clip(xs, 0, w - 1)]


def bilinear(img: np.ndarray, x, y):
    """GL_LINEAR sample at texel coordinates (centres at i + 0.5), clamp-to-edge."""
    x = np.asarray(x, np.float32) - 0.5
    y = np.asarray(y, np.float32) - 0.5
    x0 = np.floor(x)
    y0 = np.floor(y)
    fx = x - x0
    fy = y - y0
    x0 = x0.astype(np.int64)
    y0 = y0.astype(np.int64)
    if img.ndim == 3:
        fx = fx[..., None]
        fy = fy[..., None]
    a = fetch(img, y0, x0)
    b = fetch(img, y0, x0 + 1)
    c = fetch(img, y0 + 1, x0)
    d = fetch(img, y0 + 1, x0 + 1)
    top = a + (b - a) * fx
    bot = c + (d - c) * fx
    return top + (bot - top) * fy


def fetch_sep(img: np.ndarray, ys, xs):
    """texelFetch on a separable grid: rows ys (H,) by columns xs (W,), clamp-to-edge."""
    h, w = img.shape[:2]
    return img[np.clip(ys, 0, h - 1)[:, None], np.clip(xs, 0, w - 1)[None, :]]


def bilinear_sep(img: np.ndarray, x, y):
    """bilinear() for sample points on a separable grid: x (W,) and y (H,) texel coordinates."""
    x = np.asarray(x, np.float32) - 0.5
    y = np.asarray(y, np.float32) - 0.5
    x0 = np.floor(x)
    y0 = np.floor(y)
    fx = (x - x0)[None, :]
    fy = (y - y0)[:, None]
    if img.ndim == 3:
        fx = fx[..., None]
        fy = fy[..., None]
    x0 = x0.astype(np.int64)
    y0 = y0.astype(np.int64)
    a = fetch_sep(img, y0, x0)
    b = fetch_sep(img, y0, x0 + 1)
    c = fetch_sep(img, y0 + 1, x0)
    d = fetch_sep(img, y0 + 1, x0 + 1)
    top = a + (b - a) * fx
    bot = c + (d - c) * fx
    return top + (bot - top) * fy


def catmull_rom(img: np.ndarray, x, y):
    """Catmull-Rom bicubic sample at texel coordinates, clamp-to-edge (16 taps).

    A shader gets the same result from 9 bilinear fetches by merging the two middle taps of
    each axis, which is exact because their weights share a sign."""
    x = np.asarray(x, np.float32) - 0.5
    y = np.asarray(y, np.float32) - 0.5
    x0 = np.floor(x)
    y0 = np.floor(y)
    tx = x - x0
    ty = y - y0
    x0 = x0.astype(np.int64)
    y0 = y0.astype(np.int64)

    def weights(t):
        t2 = t * t
        t3 = t2 * t
        return (
            -0.5 * t3 + t2 - 0.5 * t,
            1.5 * t3 - 2.5 * t2 + 1.0,
            -1.5 * t3 + 2.0 * t2 + 0.5 * t,
            0.5 * t3 - 0.5 * t2,
        )

    wx = weights(tx)
    wy = weights(ty)
    h, w = img.shape[:2]
    flat = img.reshape(h * w, -1)
    cols = [np.clip(x0 + i - 1, 0, w - 1).astype(np.int32) for i in range(4)]
    out = 0.0
    for j in range(4):
        rowbase = np.clip(y0 + j - 1, 0, h - 1).astype(np.int32) * np.int32(w)
        row = 0.0
        for i in range(4):
            wi = wx[i][..., None]
            row = row + np.take(flat, rowbase + cols[i], axis=0) * wi
        out = out + row * wy[j][..., None]
    return out.reshape(x.shape + img.shape[2:])


def output_grid(W: int, H: int):
    """Output pixel centres in output texel units."""
    xs = np.arange(W, dtype=np.float32) + 0.5
    ys = np.arange(H, dtype=np.float32) + 0.5
    return np.meshgrid(xs, ys)


def luma(rgb: np.ndarray) -> np.ndarray:
    return rgb @ LUMA


def down2(img: np.ndarray) -> np.ndarray:
    """2x2 box reduction, as glGenerateMipmap does for an even level."""
    h, w = img.shape[:2]
    h2, w2 = h // 2, w // 2
    return img[: h2 * 2, : w2 * 2].reshape(h2, 2, w2, 2).mean((1, 3)).astype(np.float32)


# --------------------------------------------------------------------------------------------
# Spatial upscalers


def output_axes(W: int, H: int):
    return np.arange(W, dtype=np.float32) + 0.5, np.arange(H, dtype=np.float32) + 0.5


def upscale_bilinear(src: np.ndarray, W: int, H: int) -> np.ndarray:
    h, w = src.shape[:2]
    ox, oy = output_axes(W, H)
    return bilinear_sep(src, ox * np.float32(w / W), oy * np.float32(h / H))


def _fast_lanczos2(x):
    wa = x - 4.0
    wb = x * wa - wa
    wa = wa * wa
    return wb * wa


def _weight_y(dx, dy, c, std):
    x = (dx * dx + dy * dy) * 0.55 + np.clip(np.abs(c) * std, 0.0, 1.0)
    w = _fast_lanczos2(x)
    return w, w * c


def _sgsr1_gl(src: np.ndarray, W: int, H: int, edge_sharpness: float) -> np.ndarray:
    """sgsr.frag in GL space: row index grows with the texture's t coordinate."""
    h, w = src.shape[:2]
    f32 = np.float32
    # Every output row shares its v and every column its u, so the grid is separable.
    ox, oy = output_axes(W, H)
    u = ox / f32(W)
    v = oy / f32(H)

    def load_color(px, py):
        px = np.clip(px, 0.5 / w, 1.0 - 0.5 / w).astype(f32)
        py = np.clip(py, 0.5 / h, 1.0 - 0.5 / h).astype(f32)
        return bilinear_sep(src, px * w, py * h)

    green = src[..., 1]
    rgb = load_color(u, v)
    img_x = u * f32(w) - f32(0.5)
    img_y = v * f32(h) + f32(0.5)
    pix_x = np.floor(img_x)
    pix_y = np.floor(img_y)
    plx = (img_x - pix_x)[None, :]
    ply = (img_y - pix_y)[:, None]
    pix_x = pix_x.astype(np.int64)
    pix_y = pix_y.astype(np.int64)

    def g(dx, dy):
        # loadGreen(pixel) = loadColor((pixel + 0.5) / size).g: an exact, clamped texel fetch.
        return fetch_sep(green, pix_y + dy, pix_x + dx)

    left = [g(-1, 0), g(0, 0), g(0, -1), g(-1, -1)]
    edge_vote = np.abs(left[2] - left[1]) + np.abs(rgb[..., 1] - left[1]) + np.abs(rgb[..., 1] - left[2])
    mask = edge_vote > f32(8.0 / 255.0)

    right = [g(1, 0), g(2, 0), g(2, -1), g(1, -1)]
    up_down = [g(0, -2), g(1, -2), g(1, 1), g(0, 1)]
    mean = (left[1] + left[2] + right[0] + right[3]) * f32(0.25)
    left = [c - mean for c in left]
    right = [c - mean for c in right]
    up_down = [c - mean for c in up_down]
    center = rgb[..., 1] - mean
    total = sum(np.abs(c) for c in left + right + up_down)
    std = f32(2.181818) / np.maximum(total, f32(1.0e-8))

    taps = [
        (plx, ply + 1.0, up_down[0]),
        (plx - 1.0, ply + 1.0, up_down[1]),
        (plx - 1.0, ply - 2.0, up_down[2]),
        (plx, ply - 2.0, up_down[3]),
        (plx + 1.0, ply - 1.0, left[0]),
        (plx, ply - 1.0, left[1]),
        (plx, ply, left[2]),
        (plx + 1.0, ply, left[3]),
        (plx - 1.0, ply - 1.0, right[0]),
        (plx - 2.0, ply - 1.0, right[1]),
        (plx - 2.0, ply, right[2]),
        (plx - 1.0, ply, right[3]),
    ]
    wsum = 0.0
    ysum = 0.0
    for dx, dy, c in taps:
        a, b = _weight_y(dx, dy, c, std)
        wsum = wsum + a
        ysum = ysum + b
    final_y = ysum / np.maximum(wsum, f32(1.0e-8))
    max_y = np.maximum(np.maximum(left[1], left[2]), np.maximum(right[0], right[3]))
    min_y = np.minimum(np.minimum(left[1], left[2]), np.minimum(right[0], right[3]))
    final_y = np.clip(f32(edge_sharpness) * final_y, min_y, max_y)
    delta_y = np.clip(final_y - center, -23.0 / 255.0, 23.0 / 255.0).astype(f32)
    sharpened = np.clip(rgb + delta_y[..., None], 0.0, 1.0)
    return np.where(mask[..., None], sharpened, rgb).astype(f32)


def upscale_sgsr1(src: np.ndarray, W: int, H: int, edge_sharpness: float) -> np.ndarray:
    return _sgsr1_gl(src[::-1], W, H, edge_sharpness)[::-1].copy()


def sgsr_edge_sharpness(strength_percent: int) -> float:
    """SgsrConstants.edgeSharpness: the app's 0-100 strength mapped onto Qualcomm's [1, 2]."""
    return 1.0 + strength_percent / 100.0


# --------------------------------------------------------------------------------------------
# Temporal upscaler


@dataclass(frozen=True)
class TemporalConfig:
    """Tunable choices of the temporal upscaler; the defaults are the shipped ones."""

    motion: str = "estimated"  # estimated | zero | oracle
    current: str = "sgsr1"  # sgsr1 | lanczos5: the current frame's own estimate
    history_filter: str = "catmull_rom"  # bilinear | catmull_rom
    history_format: str = "f16"  # f32 | f16 | u8
    edge_sharpness: float = 1.5
    block: int = 8  # motion vector per block x block stream pixels
    search: int = 4  # coarse search radius in quarter-resolution texels
    subpel: str = "parabola"  # parabola | lk | none
    # Match confidence: residual RMS over (block sigma * sqrt2 + noise floor) mapped from
    # [conf_lo, conf_hi] to confidence [1, 0].
    conf_lo: float = 0.35
    conf_hi: float = 0.8
    noise_floor: float = 3.0 / 255.0
    cut_threshold: float = 0.25  # mean block confidence below this resets history
    min_lerp: float = 0.0  # SGSR 2 minLerpContribution for still pixels outside the box
    box_static: float | None = None  # SGSR 2 scaleRatio.y; None derives it from the scale
    alpha_scale: float = 1.0  # multiplies the current frame's blend weight
    output_usm: float = 0.0  # unsharp mask on the way out (never fed back into history)


@dataclass
class MotionResult:
    vectors: np.ndarray  # (bh, bw, 2) stream pixels: prev(p + v) ~ cur(p)
    confidence: np.ndarray  # (bh, bw) in [0, 1]
    cut: bool


def fast_lanczos_sgsr2(base):
    y = base - 1.0
    y2 = y * y
    return (0.75 * y + y2) * y2


def _block_windows(bh, bw, step, size, offset):
    """Texel index windows (bh, bw, size, size) starting at block * step + offset."""
    by, bx = np.mgrid[0:bh, 0:bw]
    r = np.arange(size)
    wy = (by * step + offset)[..., None, None] + r[None, None, :, None]
    wx = (bx * step + offset)[..., None, None] + r[None, None, None, :]
    return wy, wx


def estimate_motion(cur, prev, pred, cfg: TemporalConfig) -> MotionResult:
    """Hierarchical block matching on luma pyramids (level 0, 1/2 and 1/4 of the stream).

    One fragment per block on the GPU: a full search of +-search texels at quarter resolution
    over a 4x4 window (16x16 stream pixels) plus the block's vector from the previous frame,
    +-1 refinements at half and full resolution over 8x8 windows, then a sub-pixel fit."""
    c0, c1, c2 = cur
    p0, p1, p2 = prev
    h, w = c0.shape
    B = cfg.block
    bh, bw = -(-h // B), -(-w // B)

    # Quarter resolution: window of 4x4 texels centred on the block centre.
    wy, wx = _block_windows(bh, bw, B // 4, 4, B // 8 - 2)
    cw = fetch(c2, wy, wx)
    R = cfg.search
    lam = 0.5 / 255.0
    best_cost = np.full((bh, bw), np.inf, np.float32)
    best = np.zeros((bh, bw, 2), np.int64)
    cands = [(dx, dy) for dy in range(-R, R + 1) for dx in range(-R, R + 1)]
    for dx, dy in cands:
        cost = np.abs(cw - fetch(p2, wy + dy, wx + dx)).mean((-1, -2)) + lam * (abs(dx) + abs(dy))
        better = cost < best_cost
        best_cost = np.where(better, cost, best_cost)
        best[better] = (dx, dy)
    # Temporal predictor: last frame's vector of this block.
    pq = np.rint(pred / 4.0).astype(np.int64)
    cost = np.abs(cw - fetch(p2, wy + pq[..., 1, None, None], wx + pq[..., 0, None, None])).mean((-1, -2))
    cost += lam * np.abs(pq).sum(-1)
    better = cost < best_cost
    best[better] = pq[better]
    at_edge = (np.abs(best) >= R).any(-1) & ~better

    # Half resolution refinement, 8x8 window.
    d = best * 2
    wy, wx = _block_windows(bh, bw, B // 2, 8, B // 4 - 4)
    cw = fetch(c1, wy, wx)
    d = _refine(cw, p1, wy, wx, d, np.abs)[0]

    # Full resolution refinement, 8x8 window (the block itself), squared differences.
    d = d * 2
    wy, wx = _block_windows(bh, bw, B, B, 0)
    cw = fetch(c0, wy, wx)
    d, ssd = _refine(cw, p0, wy, wx, d, np.square)

    vec = d.astype(np.float32)
    if cfg.subpel == "parabola":
        def cost_at(ox, oy):
            return np.square(cw - fetch(p0, wy + (d[..., 1] + oy)[..., None, None],
                                        wx + (d[..., 0] + ox)[..., None, None])).mean((-1, -2))
        cxm, cxp, cym, cyp = cost_at(-1, 0), cost_at(1, 0), cost_at(0, -1), cost_at(0, 1)
        for axis, (cm, cp) in enumerate(((cxm, cxp), (cym, cyp))):
            denom = cm - 2.0 * ssd + cp
            off = np.where(denom > 1e-9, 0.5 * (cm - cp) / np.maximum(denom, 1e-9), 0.0)
            vec[..., axis] += np.clip(off, -0.5, 0.5)
    elif cfg.subpel == "lk":
        sy = wy + d[..., 1, None, None]
        sx = wx + d[..., 0, None, None]
        pw = fetch(p0, sy, sx)
        ix = 0.5 * (fetch(p0, sy, sx + 1) - fetch(p0, sy, sx - 1))
        iy = 0.5 * (fetch(p0, sy + 1, sx) - fetch(p0, sy - 1, sx))
        e = pw - cw
        a11 = (ix * ix).sum((-1, -2)) + 1e-6
        a12 = (ix * iy).sum((-1, -2))
        a22 = (iy * iy).sum((-1, -2)) + 1e-6
        b1 = -(ix * e).sum((-1, -2))
        b2 = -(iy * e).sum((-1, -2))
        det = a11 * a22 - a12 * a12
        ux = np.where(det > 1e-9, (a22 * b1 - a12 * b2) / np.maximum(det, 1e-9), 0.0)
        uy = np.where(det > 1e-9, (a11 * b2 - a12 * b1) / np.maximum(det, 1e-9), 0.0)
        vec[..., 0] += np.clip(ux, -1.0, 1.0)
        vec[..., 1] += np.clip(uy, -1.0, 1.0)

    rms = np.sqrt(ssd)
    sigma = cw.std((-1, -2))
    ratio = rms / (sigma * math.sqrt(2.0) + cfg.noise_floor)
    conf = np.clip((cfg.conf_hi - ratio) / (cfg.conf_hi - cfg.conf_lo), 0.0, 1.0).astype(np.float32)
    conf = np.where(at_edge, conf * 0.5, conf)
    cut = float(conf.mean()) < cfg.cut_threshold
    return MotionResult(vec, conf, cut)


def _refine(cw, prev, wy, wx, base, metric):
    best_cost = np.full(base.shape[:2], np.inf, np.float32)
    best = base.copy()
    for dy in (-1, 0, 1):
        for dx in (-1, 0, 1):
            ox = base[..., 0] + dx
            oy = base[..., 1] + dy
            cost = metric(cw - fetch(prev, wy + oy[..., None, None], wx + ox[..., None, None])).mean((-1, -2))
            cost = cost + (0.25 / 255.0) * (abs(dx) + abs(dy)) * (metric is np.abs)
            better = cost < best_cost
            best_cost = np.where(better, cost, best_cost)
            best[better] = np.stack([ox, oy], -1)[better]
    return best, best_cost


def blur121(img: np.ndarray) -> np.ndarray:
    """Separable [1 2 1] / 4 blur with clamp-to-edge: four bilinear fetches on the GPU."""
    p = np.pad(img, ((1, 1), (1, 1), (0, 0)), mode="edge")
    r = (p[:, :-2] + 2.0 * p[:, 1:-1] + p[:, 2:]) * 0.25
    return ((r[:-2] + 2.0 * r[1:-1] + r[2:]) * 0.25).astype(np.float32)


def quantize(img: np.ndarray, fmt: str) -> np.ndarray:
    if fmt == "f32":
        return img
    if fmt == "f16":
        return img.astype(np.float16).astype(np.float32)
    if fmt == "u8":
        return (np.rint(np.clip(img, 0.0, 1.0) * 255.0) / 255.0).astype(np.float32)
    raise ValueError(fmt)


class TemporalUpscaler:
    """Causal temporal upscaler: one output per decoded frame, using only earlier frames."""

    def __init__(self, w: int, h: int, W: int, H: int, cfg: TemporalConfig):
        self.w, self.h, self.W, self.H, self.cfg = w, h, W, H, cfg
        ox, oy = output_axes(W, H)
        self.px1 = ox * np.float32(w / W)
        self.py1 = oy * np.float32(h / H)
        self.px, self.py = np.meshgrid(self.px1, self.py1)
        self.scale = W / w
        if cfg.box_static is None:
            self.box_static = min(20.0, (W * H / (w * h)) ** 3)
        else:
            self.box_static = cfg.box_static
        self.reset()
        self.stats = {"cuts": 0, "frames": 0}

    def reset(self):
        self.history = None
        self.prev_pyr = None
        self.pred = None

    def process(self, src: np.ndarray, oracle: np.ndarray | None = None) -> np.ndarray:
        cfg = self.cfg
        w, h, W, H = self.w, self.h, self.W, self.H
        y0 = luma(src)
        pyr = [y0, down2(y0), down2(down2(y0))]
        bh, bw = -(-h // cfg.block), -(-w // cfg.block)
        if self.pred is None:
            self.pred = np.zeros((bh, bw, 2), np.float32)

        reset = self.history is None
        mv = np.zeros((H, W, 2), np.float32)
        conf = np.ones((H, W), np.float32)
        if not reset:
            if cfg.motion == "estimated":
                m = estimate_motion(pyr, self.prev_pyr, self.pred, cfg)
                self.pred = m.vectors
                if m.cut:
                    reset = True
                    self.stats["cuts"] += 1
                bx = self.px1 / cfg.block
                by = self.py1 / cfg.block
                mv = bilinear_sep(m.vectors, bx, by)
                conf = bilinear_sep(m.confidence, bx, by)
            elif cfg.motion == "oracle":
                if oracle is None:
                    raise ValueError("oracle motion requested without an oracle field")
                mv = bilinear_sep(oracle, self.px1, self.py1)
            elif cfg.motion != "zero":
                raise ValueError(cfg.motion)
        self.prev_pyr = pyr
        self.stats["frames"] += 1

        out, hist = self._upscale(src, mv, conf, reset)
        self.history = quantize(hist, cfg.history_format)
        if cfg.output_usm:
            out = np.clip(out + cfg.output_usm * (out - blur121(out)), 0.0, 1.0)
        return out

    def _upscale(self, src, mv, conf, reset):
        """SGSR 2's 2-pass upscale with zero jitter; depthfactor := 1 - match confidence."""
        cfg = self.cfg
        f32 = np.float32
        w, h, W, H = self.w, self.h, self.W, self.H
        px, py = self.px, self.py
        ipx = np.floor(self.px1).astype(np.int64)
        ipy = np.floor(self.py1).astype(np.int64)
        offx = (ipx + 0.5 - self.px1).astype(f32)[None, :]
        offy = (ipy + 0.5 - self.py1).astype(f32)[:, None]

        # Motion in SGSR 2's units: NDC length in output pixels (twice the pixel distance).
        motion_len = 2.0 * np.hypot(mv[..., 0], mv[..., 1]) * f32(self.scale)
        depthfactor = 1.0 - conf

        biasmax = self.scale
        biasmin = max(1.0, 0.3 + 0.3 * biasmax)
        kernelbias = (biasmax + (biasmin - biasmax) * 0.25 * depthfactor) * 0.5
        kb2 = kernelbias * kernelbias
        curvebias = -2.0 - np.clip(motion_len * 0.02, 0.0, 1.0)

        csum = 0.0
        wsum = 0.0
        bcenter = 0.0
        bvar = 0.0
        bweight = 0.0
        bmin = None
        bmax = None
        for dx, dy in ((0, 1), (1, 0), (-1, 0), (0, 0), (0, -1)):
            s = fetch_sep(src, ipy + dy, ipx + dx)
            ox = offx + dx
            oy = offy + dy
            d2 = ox * ox + oy * oy
            wgt = fast_lanczos_sgsr2(np.clip(d2 * kb2, 0.0, 1.0))
            csum = csum + s * wgt[..., None]
            wsum = wsum + wgt
            bw_ = np.exp(d2 * curvebias)[..., None]
            bcenter = bcenter + s * bw_
            bvar = bvar + s * s * bw_
            bweight = bweight + bw_
            bmin = s if bmin is None else np.minimum(bmin, s)
            bmax = s if bmax is None else np.maximum(bmax, s)
        bcenter = bcenter / bweight
        bvar = np.sqrt(np.abs(bvar / bweight - bcenter * bcenter))

        detail = 0.0
        if cfg.current == "lanczos5":
            cur = np.clip(csum / np.maximum(wsum, 1e-6)[..., None], bmin - 0.075, bmax + 0.075)
        elif cfg.current == "bilinear":
            cur = upscale_bilinear(src, W, H)
        elif cfg.current == "detail":
            # History accumulates the plain bilinear picture; SGSR 1's edge detail of the current
            # frame is added on the way out and never fed back, so clamping cannot eat it.
            cur = upscale_bilinear(src, W, H)
            detail = upscale_sgsr1(src, W, H, cfg.edge_sharpness) - cur
        else:
            cur = upscale_sgsr1(src, W, H, cfg.edge_sharpness)
        weight = wsum * (1.0 / 3.0) * cfg.alpha_scale

        if reset:
            return np.clip(cur + detail, 0.0, 1.0).astype(f32), cur.astype(f32)

        hx = (px + mv[..., 0]) * f32(W / w)
        hy = (py + mv[..., 1]) * f32(H / h)
        inside = (hx >= 0) & (hx <= W) & (hy >= 0) & (hy <= H)
        sampler = catmull_rom if cfg.history_filter == "catmull_rom" else bilinear
        hist = sampler(self.history, hx, hy)

        baseupdate = 1.0 - depthfactor
        baseupdate = np.minimum(baseupdate, baseupdate + (weight * 10.0 - baseupdate) * np.clip(10.0 * motion_len, 0, 1))
        baseupdate = np.minimum(baseupdate, baseupdate + (weight - baseupdate) * np.clip(motion_len * 0.05, 0, 1))
        basealpha = baseupdate

        boxscale = np.maximum(depthfactor, np.clip(motion_len * 0.05, 0.0, 1.0))
        boxsize = self.box_static + (1.0 - self.box_static) * boxscale
        sbox = bvar * boxsize[..., None]
        rmax = np.minimum(bmax, bcenter + sbox)
        rmin = np.maximum(bmin, bcenter - sbox)
        clamped = np.clip(hist, rmin, rmax)
        still = motion_len < 2.0 / 64.0
        start_lerp = np.where(still, cfg.min_lerp, 0.0)
        outside = ((rmin > hist) | (hist > rmax)).any(-1)
        lerp = np.where(outside, start_lerp, 1.0)
        hist = clamped + (hist - clamped) * lerp[..., None]
        basemin = np.minimum(basealpha, 0.1)
        basealpha = basemin + (basealpha - basemin) * lerp

        alphasum = np.maximum(1.192e-7, basealpha + weight)
        alpha = np.clip(weight / alphasum, 0.0, 1.0)
        alpha = np.where(inside, alpha, 1.0)
        out = hist + (cur - hist) * alpha[..., None]
        return np.clip(out + detail, 0.0, 1.0).astype(f32), out.astype(f32)


@dataclass(frozen=True)
class FusionConfig:
    """Multi-frame fusion: the current and the last frames - 1 decoded frames, sampled at their
    motion-compensated positions in screen space; no recursion, no added delay."""

    frames: int = 4
    motion: str = "median"  # median (one vector per frame pair) | block | oracle
    sigma: float = 0.35  # Gaussian sample weight, stream pixels
    tau: float | None = 0.06  # earlier samples far from the current bilinear picture fade out
    usm: float = 1.0  # unsharp mask on the way out
    edge_sharpness: float = 1.5  # SGSR 1 on frames without history


class FusionUpscaler:
    """16 * frames fetches per output pixel plus one motion pass per earlier frame."""

    def __init__(self, w: int, h: int, W: int, H: int, cfg: FusionConfig):
        self.w, self.h, self.W, self.H, self.cfg = w, h, W, H, cfg
        ox, oy = output_axes(W, H)
        self.px1 = ox * np.float32(w / W)
        self.py1 = oy * np.float32(h / H)
        self.px, self.py = np.meshgrid(self.px1, self.py1)
        self.history = []  # newest first: (frame, luma pyramid)
        self.motion_cfg = TemporalConfig(subpel="lk")
        self.stats = {"cuts": 0, "frames": 0}

    def process(self, src: np.ndarray, oracle: np.ndarray | None = None) -> np.ndarray:
        cfg = self.cfg
        y = luma(src)
        pyr = [y, down2(y), down2(down2(y))]
        self.stats["frames"] += 1
        bh, bw = -(-self.h // 8), -(-self.w // 8)
        pred = np.zeros((bh, bw, 2), np.float32)
        taps = []
        for k, (frame, fpyr) in enumerate(self.history, start=1):
            if cfg.motion == "oracle":
                if oracle is None:
                    raise ValueError("oracle motion requested without an oracle field")
                mv = k * bilinear_sep(oracle, self.px1, self.py1)  # constant-velocity clips
                cf = np.ones(mv.shape[:2], np.float32)
            else:
                m = estimate_motion(pyr, fpyr, pred, self.motion_cfg)
                if k == 1 and m.cut:
                    self.stats["cuts"] += 1
                    self.history = []
                    taps = []
                    break
                pred = m.vectors * (k + 1) / k
                vec = m.vectors
                if cfg.motion == "median":
                    vec = np.broadcast_to(np.median(vec.reshape(-1, 2), 0), vec.shape)
                mv = bilinear_sep(np.ascontiguousarray(vec), self.px1 / 8, self.py1 / 8)
                cf = bilinear_sep(m.confidence, self.px1 / 8, self.py1 / 8)
            taps.append((frame, mv, cf))
        self.history = ([(src, pyr)] + self.history)[: cfg.frames - 1]
        if not taps:
            return upscale_sgsr1(src, self.W, self.H, cfg.edge_sharpness)

        base = upscale_bilinear(src, self.W, self.H)
        acc = 0.0
        wacc = 0.0
        zero = (np.zeros(self.px.shape + (2,), np.float32), None)
        for k, (frame, mv, cf) in enumerate([(src,) + zero] + taps):
            qx = self.px + mv[..., 0] - 0.5
            qy = self.py + mv[..., 1] - 0.5
            x0 = np.floor(qx).astype(np.int64)
            y0 = np.floor(qy).astype(np.int64)
            for sy in (-1, 0, 1, 2):
                for sx in (-1, 0, 1, 2):
                    dx = x0 + sx - qx
                    dy = y0 + sy - qy
                    smp = fetch(frame, y0 + sy, x0 + sx)
                    wgt = np.exp(-(dx * dx + dy * dy) / (2.0 * cfg.sigma * cfg.sigma))
                    if k:
                        wgt = wgt * cf
                        if cfg.tau is not None:
                            diff = np.abs(smp - base).max(-1)
                            wgt = wgt * np.exp(-(diff * diff) / (2.0 * cfg.tau * cfg.tau))
                    acc = acc + smp * wgt[..., None]
                    wacc = wacc + wgt
        o = acc / np.maximum(wacc, 1e-8)[..., None]
        return np.clip(o + cfg.usm * (o - blur121(o)), 0.0, 1.0).astype(np.float32)


def fetch_cost(cfg: TemporalConfig, w: int, h: int, W: int, H: int) -> dict:
    """Texture fetches per frame by pass: a GPU cost proxy until it is timed on hardware."""
    B = cfg.block
    blocks = -(-w // B) * -(-h // B)
    R = cfg.search
    coarse = 16 + (2 * R + 1) ** 2 * 16 + 16
    half = 64 + 9 * 64
    full = B * B + 9 * B * B
    sub = 4 * B * B if cfg.subpel == "parabola" else (5 * B * B if cfg.subpel == "lk" else 0)
    motion = blocks * (coarse + half + full + sub)
    luma_pass = w * h * 1
    current = 13 if cfg.current == "sgsr1" else 0
    per_px = 5 + current + 2 + (9 if cfg.history_filter == "catmull_rom" else 1)
    return {
        "luma_and_mips": luma_pass,
        "motion": motion,
        "motion_per_block": coarse + half + full + sub,
        "upscale": W * H * per_px,
        "upscale_per_pixel": per_px,
        "sgsr1_per_pixel": 13,
    }

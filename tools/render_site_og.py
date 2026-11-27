#!/usr/bin/env python3
"""Render Rubylight 7.0 Obsidian Studio social card (site/og.png, 1200x630) and poster (site/poster.jpg, 1280x720).

Uses the published 7.0 console captures in docs/media/readme/ and bundled Figtree typography—zero AI art.
"""

from __future__ import annotations

from pathlib import Path
import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
FONT_REG = ROOT / "client/protocol/desktop/assets/Figtree-400.ttf"
FONT_BOLD = ROOT / "client/protocol/desktop/assets/Figtree-600.ttf"
README_MEDIA = ROOT / "docs/media/readme"
SITE = ROOT / "site"

BG_DEEP = (9, 10, 13)      # #090A0D
BG_PANEL = (17, 19, 25)    # #111319
RUBY = (206, 30, 73)       # #CE1E49
RUBY_BRAND = (196, 18, 66) # #C41242
RUBY_TEXT = (255, 117, 150)
INK = (245, 247, 250)
INK_2 = (200, 206, 219)
MUTED = (154, 163, 181)
LINE = (54, 62, 82)


def load_font(size: int, bold: bool = True) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(str(FONT_BOLD if bold else FONT_REG), size)


def studio_canvas(w: int, h: int) -> Image.Image:
    yy = np.linspace(0, 1, h, dtype=np.float32)[:, None]
    xx = np.linspace(0, 1, w, dtype=np.float32)[None, :]
    v = np.clip(1 - (((xx - 0.56) * 1.1) ** 2 + ((yy - 0.42) * 1.2) ** 2), 0, 1)
    arr = np.zeros((h, w, 3), np.float32)
    for k in range(3):
        arr[..., k] = BG_DEEP[k] + (BG_PANEL[k] - BG_DEEP[k]) * (0.2 + 0.8 * v)
    # Soft ruby studio glow behind the right-hand console frames.
    d_ruby = np.sqrt(((xx - 0.78) * 1.35) ** 2 + ((yy - 0.46) * 1.45) ** 2)
    g_ruby = (np.clip(1 - d_ruby, 0, 1) ** 2.1)[..., None] * 0.24
    arr = arr * (1 - g_ruby) + np.array(RUBY, np.float32) * g_ruby
    return Image.fromarray(np.clip(arr, 0, 255).astype(np.uint8), "RGB").convert("RGBA")


def rounded_shot(path: Path, w: int, h: int, radius: int = 16) -> Image.Image:
    src = Image.open(path).convert("RGB")
    scale = max(w / src.width, h / src.height)
    resized = src.resize((round(src.width * scale), round(src.height * scale)), Image.LANCZOS)
    cropped = resized.crop((0, 0, w, h))

    pad = 36
    card = Image.new("RGBA", (w + pad * 2, h + pad * 2), (0, 0, 0, 0))
    sh = Image.new("L", card.size, 0)
    ImageDraw.Draw(sh).rounded_rectangle((pad, pad + 10, pad + w, pad + h + 10), radius, fill=165)
    sh = sh.filter(ImageFilter.GaussianBlur(16))
    sh_rgba = Image.new("RGBA", card.size, (0, 0, 0, 255))
    sh_rgba.putalpha(sh)
    card.alpha_composite(sh_rgba)

    ss = 2
    mask_big = Image.new("L", (w * ss, h * ss), 0)
    ImageDraw.Draw(mask_big).rounded_rectangle((0, 0, w * ss - 1, h * ss - 1), radius * ss, fill=255)
    mask = mask_big.resize((w, h), Image.LANCZOS)

    frame = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    frame.paste(cropped, (0, 0), mask)
    rim_big = Image.new("RGBA", (w * ss, h * ss), (0, 0, 0, 0))
    ImageDraw.Draw(rim_big).rounded_rectangle(
        (1, 1, w * ss - 2, h * ss - 2), radius * ss, outline=LINE + (230,), width=2
    )
    frame.alpha_composite(rim_big.resize((w, h), Image.LANCZOS))
    card.alpha_composite(frame, (pad, pad))
    return card


def draw_mark(cv: Image.Image, x: int, y: int, size: int) -> None:
    ss = 4
    big = Image.new("RGBA", (size * ss, size * ss), (0, 0, 0, 0))
    d = ImageDraw.Draw(big)
    r = round(size * 0.26 * ss)
    d.rounded_rectangle((0, 0, size * ss - 1, size * ss - 1), r, fill=RUBY_BRAND + (255,))
    d.rounded_rectangle((2, 2, size * ss - 3, size * ss - 3), r, outline=(255, 255, 255, 42), width=ss)
    f = load_font(round(size * 0.62 * ss), bold=True)
    bbox = d.textbbox((0, 0), "R", font=f)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    d.text(((size * ss - tw) / 2 - bbox[0], (size * ss - th) / 2 - bbox[1]), "R", font=f, fill=(255, 255, 255, 255))
    cv.alpha_composite(big.resize((size, size), Image.LANCZOS), (x, y))


def render_card(w: int, h: int) -> Image.Image:
    cv = studio_canvas(w, h)
    stream_shot = rounded_shot(README_MEDIA / "console-stream-dark.webp", round(w * 0.41), round(h * 0.50), 16)
    home_shot = rounded_shot(README_MEDIA / "console-home-dark.webp", round(w * 0.34), round(h * 0.41), 14)
    cv.alpha_composite(home_shot, (round(w * 0.58), round(h * 0.06)))
    cv.alpha_composite(stream_shot, (round(w * 0.53), round(h * 0.35)))

    d = ImageDraw.Draw(cv)
    mx, my = round(w * 0.055), round(h * 0.11)
    draw_mark(cv, mx, my, 50)
    d.text((mx + 66, my + 5), "Rubylight", font=load_font(35, True), fill=INK + (255,))

    y = round(h * 0.30)
    d.text((mx, y), "Low-latency PC", font=load_font(round(h * 0.090), True), fill=INK + (255,))
    y += round(h * 0.102)
    d.text((mx, y), "game streaming.", font=load_font(round(h * 0.090), True), fill=RUBY_TEXT + (255,))
    y += round(h * 0.145)
    d.text(
        (mx, y),
        "Built in Rust for AMD Radeon & NVIDIA.\nFirst-party display & gamepad drivers,\ntrue HDR and Moonlight compatibility.",
        font=load_font(round(h * 0.034), False),
        fill=INK_2 + (255,),
        spacing=8,
    )
    d.text(
        (mx, round(h * 0.86)),
        "Windows 10 & 11  ·  AMF & NVENC  ·  GPL-3.0",
        font=load_font(round(h * 0.028), False),
        fill=MUTED + (255,),
    )
    return cv.convert("RGB")


def main() -> None:
    og = render_card(1200, 630)
    og_path = SITE / "og.png"
    og.save(og_path, "PNG", optimize=True)
    print(f"{og_path.relative_to(ROOT)}  {og.width}x{og.height}  {og_path.stat().st_size // 1024} KB")

    poster = render_card(1280, 720)
    poster_path = SITE / "poster.jpg"
    poster.save(poster_path, "JPEG", quality=90, optimize=True)
    print(f"{poster_path.relative_to(ROOT)}  {poster.width}x{poster.height}  {poster_path.stat().st_size // 1024} KB")


if __name__ == "__main__":
    main()

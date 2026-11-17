#!/usr/bin/env python3
"""Builds the pictures and short films of both READMEs from real captures.

    python docs/media/build_readme_media.py <captures> [console] [nightjar]

<captures> holds what the capture steps wrote:

    console/stills/<page>-<dark|light>.png   node docs/media/capture_readme.cjs <captures>/console stills
    console/tour/frame-*.jpg, frames.txt     node docs/media/capture_readme.cjs <captures>/console tour
    android/raw/<state>.png, clips/*.mp4     client/scripts/demo/readme-capture.sh <captures>/android
    phone/<name>.png                         the same offline demo on an upright phone (--es state home, detail)
    desktop/<name>.png                       the desktop app's own page screenshots (its page_screenshots tests)

Everything shown is a demo: the console runs against made-up answers (no host, no token, no address), Nightjar
plays its bundled sample scene with a made-up library. Nothing here is a measurement.

Outputs: docs/media/readme/ (Rubylight) and client/docs/assets/readme/ (Nightjar). Needs Pillow and FFmpeg.
"""
import subprocess
import sys
from pathlib import Path

from PIL import Image

REPO = Path(__file__).resolve().parents[2]
HOST_OUT = REPO / 'docs/media/readme'
NIGHTJAR_OUT = REPO / 'client/docs/assets/readme'

# Console pages the README shows, each in the dark and the light theme.
CONSOLE_STILLS = ['home', 'games', 'game', 'add-games', 'settings', 'stream', 'sessions', 'coop']

# Nightjar stills: output name -> (source, crop box or None, output width). The handheld layout keeps Android's
# status bar (clock, battery), so those shots lose their top strip; streams and the TV layout are full screen.
STATUS_BAR = 56
NIGHTJAR_STILLS = {
    'tv-home': ('android/raw/tv-home.png', None, 1600),
    'tv-game': ('android/raw/tv-detail.png', None, 1600),
    'handheld-home': ('android/raw/home.png', (0, STATUS_BAR, 1920, 1080), 1600),
    'handheld-light': ('android/raw/home-day.png', (0, STATUS_BAR, 1920, 1080), 1600),
    'stream-menu': ('android/raw/stream-menu.png', None, 1600),
    'stream-controls': ('android/raw/stream-modern.png', None, 1600),
    'stream-keyboard': ('android/raw/stream-keyboard.png', None, 1600),
    'stream-stats': ('android/raw/stream-stats.png', None, 1600),
    'phone-home': ('phone/android-phone-home-setup-card.png', (0, 84, 1080, 1920), 720),
    'phone-game': ('phone/android-game-sheet.png', (0, 84, 1080, 1920), 720),
    'desktop-home': ('desktop/home-1280x800.png', None, 1600),
    'desktop-game': ('desktop/game-1280x800.png', None, 1600),
    'desktop-settings': ('desktop/settings-1280x800.png', None, 1600),
    'desktop-all-games': ('desktop/all-games-1280x800.png', None, 1600),
}

# The Nightjar film: (clip, first second, last second or None for the end, seconds the last picture is held).
NIGHTJAR_FILM = [
    ('android/clips/tv-home.mp4', 0.6, None, 1.8),
    ('android/clips/stream-menu.mp4', 2.6, 8.0, 0.0),
    ('android/clips/controls.mp4', 1.0, 8.0, 0.6),
]
CROSSFADE = 0.4


def ffmpeg(*args):
    subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', *map(str, args)], check=True)


def duration(path):
    out = subprocess.run(['ffprobe', '-v', 'error', '-show_entries', 'format=duration', '-of', 'csv=p=0', str(path)],
                         check=True, capture_output=True, text=True).stdout
    return float(out.strip())


def still(source, target, crop, width, quality=88):
    image = Image.open(source).convert('RGB')
    if crop:
        image = image.crop(crop)
    if image.width > width:
        image = image.resize((width, round(image.height * width / image.width)), Image.LANCZOS)
    target.parent.mkdir(parents=True, exist_ok=True)
    image.save(target, 'WEBP', quality=quality, method=6)
    print(f'{target.relative_to(REPO)}  {image.width}x{image.height}  {target.stat().st_size // 1024} KB')


def animated(source, target, width, fps, quality):
    """The inline version: an animated WebP GitHub plays in the README itself."""
    ffmpeg('-i', source, '-vf', f'fps={fps},scale={width}:-2:flags=lanczos', '-c:v', 'libwebp_anim', '-lossless', 0,
           '-q:v', quality, '-compression_level', 6, '-loop', 0, '-an', target)
    print(f'{target.relative_to(REPO)}  {target.stat().st_size // 1024} KB')


def console(captures):
    for name in CONSOLE_STILLS:
        for theme in ('dark', 'light'):
            still(captures / f'console/stills/{name}-{theme}.png', HOST_OUT / f'console-{name}-{theme}.webp', None, 1600)
    film = HOST_OUT / 'console-tour.mp4'
    # frames.txt lists each captured picture with the time it stayed on screen.
    ffmpeg('-f', 'concat', '-safe', 0, '-i', captures / 'console/tour/frames.txt', '-vf', 'fps=30,format=yuv420p',
           '-c:v', 'libx264', '-preset', 'veryslow', '-crf', 21, '-movflags', '+faststart', '-an', film)
    print(f'{film.relative_to(REPO)}  {film.stat().st_size // 1024} KB  {duration(film):.1f} s')
    animated(film, HOST_OUT / 'console-tour.webp', 1280, 12, 70)


def nightjar(captures):
    for name, (source, crop, width) in NIGHTJAR_STILLS.items():
        still(captures / source, NIGHTJAR_OUT / f'{name}.webp', crop, width)
    inputs, filters, lengths = [], [], []
    for index, (clip, first, last, hold) in enumerate(NIGHTJAR_FILM):
        path = captures / clip
        end = last if last is not None else duration(path)
        inputs += ['-i', path]
        length = end - first + hold
        # screenrecord writes a picture only when the screen changes, so a clip's last picture can come well before
        # its end: hold it, then cut to the exact length the crossfade offsets count on.
        filters.append(f'[{index}:v]fps=30,trim={first}:{end},setpts=PTS-STARTPTS,scale=1280:720:flags=lanczos,'
                       f'format=yuv420p,tpad=stop_mode=clone:stop_duration=30,trim=0:{length:.3f}[v{index}]')
        lengths.append(length)
    previous, offset = 'v0', 0.0
    for index in range(1, len(NIGHTJAR_FILM)):
        offset += lengths[index - 1] - CROSSFADE
        filters.append(f'[{previous}][v{index}]xfade=transition=fade:duration={CROSSFADE}:offset={offset:.3f}[x{index}]')
        previous = f'x{index}'
    film = NIGHTJAR_OUT / 'nightjar-tour.mp4'
    film.parent.mkdir(parents=True, exist_ok=True)
    ffmpeg(*inputs, '-filter_complex', ';'.join(filters), '-map', f'[{previous}]', '-c:v', 'libx264', '-preset',
           'veryslow', '-crf', 23, '-movflags', '+faststart', '-an', film)
    print(f'{film.relative_to(REPO)}  {film.stat().st_size // 1024} KB  {duration(film):.1f} s')
    animated(film, NIGHTJAR_OUT / 'nightjar-tour.webp', 960, 15, 60)


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    captures = Path(sys.argv[1])
    parts = sys.argv[2:] or ['console', 'nightjar']
    if 'console' in parts:
        console(captures)
    if 'nightjar' in parts:
        nightjar(captures)


if __name__ == '__main__':
    main()

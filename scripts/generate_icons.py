"""Derive the application icon files from one square RGBA source image.

Requires Pillow (development only; nothing is generated at build time):

    python scripts/generate_icons.py docs/icon/syncengine.png

Writes apps/desktop/assets/: syncengine.ico (Windows executable resource),
syncengine.png (256 px, Linux archive) and icon-64.rgba (straight RGBA,
64 x 64, embedded for the window and notification-area icons).
"""
import argparse
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "apps/desktop/assets"
RAW_SIZE = 64  # charts::ICON_SIZE


def trimmed(image):
    """Crop the transparent margin, centered square: the icon then fills the
    canvas like other applications' icons (taskbar, Start menu)."""
    box = image.getchannel("A").point(lambda v: 255 if v > 10 else 0).getbbox()
    if not box:
        raise SystemExit("fully transparent source image")
    side = max(box[2] - box[0], box[3] - box[1])
    cx, cy = (box[0] + box[2]) // 2, (box[1] + box[3]) // 2
    square = Image.new("RGBA", (side, side))
    square.paste(image.crop((cx - side // 2, cy - side // 2, cx - side // 2 + side, cy - side // 2 + side)))
    return square


def generate(source):
    image = trimmed(Image.open(source).convert("RGBA"))
    if image.width != image.height:
        raise SystemExit("square source image required")
    ASSETS.mkdir(parents=True, exist_ok=True)
    image.resize((256, 256), Image.LANCZOS).save(ASSETS / "syncengine.png", optimize=True)
    image.save(ASSETS / "syncengine.ico",
               sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    (ASSETS / "icon-64.rgba").write_bytes(image.resize((RAW_SIZE, RAW_SIZE), Image.LANCZOS).tobytes())


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("source", type=Path)
    generate(parser.parse_args().source)

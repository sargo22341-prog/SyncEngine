"""Draw the SyncEngine logo (docs/icon/syncengine.png) from its geometry.

The logo is the one of the interface (`apps/desktop/src/charts.rs`,
`logo_paths`): a rounded square split by a flowing S-curve, lime above and
violet below. Requires Pillow (development only):

    python scripts/render_icon.py
    python scripts/generate_icons.py docs/icon/syncengine.png

The picture is drawn at 4096 px then reduced, so edges stay clean at 16 px.
"""
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "docs/icon/syncengine.png"
SIZE = 1024
SCALE = 4
LIME = (200, 245, 60, 255)
VIOLET = (106, 92, 255, 255)
RADIUS = 0.30
WAVE = 0.17


def bezier(p0, p1, p2, p3, steps=400):
    points = []
    for i in range(steps + 1):
        t = i / steps
        u = 1 - t
        points.append((
            u**3 * p0[0] + 3 * u**2 * t * p1[0] + 3 * u * t**2 * p2[0] + t**3 * p3[0],
            u**3 * p0[1] + 3 * u**2 * t * p1[1] + 3 * u * t**2 * p2[1] + t**3 * p3[1],
        ))
    return points


def render():
    s = SIZE * SCALE
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, s - 1, s - 1), radius=int(RADIUS * s), fill=255)

    yl, yr = 0.46 * s, 0.54 * s
    wave = WAVE * s
    curve = bezier((0, yl), (0.35 * s, yl - wave - 0.2 * s), (0.65 * s, yr + wave + 0.2 * s), (s, yr))
    lower = Image.new("L", (s, s), 0)
    ImageDraw.Draw(lower).polygon(curve + [(s, s), (0, s)], fill=255)

    image = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    image.paste(Image.new("RGBA", (s, s), LIME), mask=mask)
    violet_mask = Image.composite(lower, Image.new("L", (s, s), 0), mask)
    image.paste(Image.new("RGBA", (s, s), VIOLET), mask=violet_mask)
    return image.resize((SIZE, SIZE), Image.LANCZOS)


if __name__ == "__main__":
    OUT.parent.mkdir(parents=True, exist_ok=True)
    render().save(OUT)
    print(f"written {OUT}")

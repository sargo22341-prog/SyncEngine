"""Generate SyncEngine's original geometric icon (Pillow, development only)."""
from pathlib import Path
from PIL import Image, ImageDraw

root = Path(__file__).resolve().parents[1] / "src-tauri" / "icons"
root.mkdir(parents=True, exist_ok=True)
image = Image.new("RGBA", (256, 256), "#142e47")
draw = ImageDraw.Draw(image)
draw.line([(64, 92), (188, 92)], fill="#64dfc7", width=20)
draw.polygon([(188, 62), (220, 92), (188, 122)], fill="#64dfc7")
draw.line([(68, 164), (192, 164)], fill="white", width=20)
draw.polygon([(68, 134), (36, 164), (68, 194)], fill="white")
image.save(root / "icon.png")
image.save(root / "icon.ico", sizes=[(16, 16), (32, 32), (48, 48), (256, 256)])

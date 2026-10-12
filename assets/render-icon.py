"""Renders icon.svg to icon.png (1024 px, with transparency) on a Mac, with no extra tools.

    python3 render-icon.py        # then: npx tauri icon assets/icon.png

Quick Look (qlmanage) draws an SVG on an opaque white background. So the icon is drawn twice, on white
and on black, and the alpha channel and the true colors are worked out from the two pictures.
Needs Pillow.
"""
import subprocess
import tempfile
from pathlib import Path

from PIL import Image

here = Path(__file__).parent
svg = (here / "icon.svg").read_text()
head = svg.index(">", svg.index("<svg")) + 1

with tempfile.TemporaryDirectory() as tmp:
    tmp = Path(tmp)
    for name, color in (("white", "#fff"), ("black", "#000")):
        (tmp / f"{name}.svg").write_text(svg[:head] + f'<rect width="1024" height="1024" fill="{color}"/>' + svg[head:])
    subprocess.run(["qlmanage", "-t", "-s", "1024", "-o", str(tmp), str(tmp / "white.svg"), str(tmp / "black.svg")],
                   check=True, capture_output=True)
    white = Image.open(tmp / "white.svg.png").convert("RGB")
    black = Image.open(tmp / "black.svg.png").convert("RGB")
    wp, kp = white.load(), black.load()
    size = white.size[0]
    out = Image.new("RGBA", white.size)
    op = out.load()
    for y in range(size):
        for x in range(size):
            w, k = wp[x, y], kp[x, y]
            alpha = max(0.0, min(255.0, 255 - sum(w[i] - k[i] for i in range(3)) / 3))
            if alpha <= 0.5:
                op[x, y] = (0, 0, 0, 0)
                continue
            scale = 255 / alpha
            op[x, y] = tuple(min(255, round(k[i] * scale)) for i in range(3)) + (round(alpha),)
    out.save(here / "icon.png")
print("written icon.png")

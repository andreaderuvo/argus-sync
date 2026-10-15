"""argus-sync's mark and icons, in the family's drawing.

Argus is an eye ringed by twelve teal dots, one of them amber at the upper right; Panoptes is the
same eye in a grid of eight (the machines). argus-sync keeps the eye and the dots and turns them
into two arcs chasing each other round it — each dot bigger than the one before, so an arc reads as
an arrow, and the two together as the sign for sync. The head of one arc is the amber dot, where
Argus has its own.

    python3 assets/make_icons.py      # assets/*.svg, *.png, the app's icons, the window's logo
"""

from __future__ import annotations

import math
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent.parent
TEAL, AMBER, IRIS, PUPIL, SHINE, TILE, TILE_PUPIL = "#1f7f79", "#e3a93b", "#9fe2c6", "#15232d", "#f4f7f6", "#0f1820", "#0a1117"
# Angles clockwise from the top. Both arcs turn anticlockwise, tail to head.
ARCS = [([150, 127, 104, 81, 58], AMBER), ([330, 307, 284, 261, 238], TEAL)]
SIZES = [2.3, 3.1, 3.9, 4.8, 6.0]


def shapes(size: float, tile: bool):
    """(kind, x, y, r, colour) for a drawing `size` wide; on a tile, inset as Argus's app icon is."""
    s = size / 100
    c = size / 2
    ring, k, iris = (32.0, 0.82, 20.5) if tile else (40.0, 1.0, 25.0)
    out = []
    for angles, head in ARCS:
        for i, a in enumerate(angles):
            t = math.radians(a)
            out.append(("dot", c + ring * s * math.sin(t), c - ring * s * math.cos(t), SIZES[i] * k * s,
                        head if i == len(angles) - 1 else TEAL))
    r = iris * s
    out += [("dot", c, c, r, IRIS), ("dot", c, c, r * 0.56, TILE_PUPIL if tile else PUPIL),
            ("dot", c - r * 0.2, c - r * 0.2, r * 0.136, SHINE)]
    return out


def svg(size: int, tile: bool) -> str:
    lines = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" width="{size}" height="{size}">',
             "  <title>argus-sync</title>"]
    if tile:
        lines.append(f'  <rect width="{size}" height="{size}" rx="{size * 0.2266:.0f}" fill="{TILE}"/>')
    lines += [f'  <circle cx="{x:.2f}" cy="{y:.2f}" r="{r:.2f}" fill="{col}"/>' for _k, x, y, r, col in shapes(size, tile)]
    return "\n".join(lines + ["</svg>", ""])


def png(size: int, tile: bool) -> Image.Image:
    k = 4
    w = size * k
    im = Image.new("RGBA", (w, w), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    if tile:
        d.rounded_rectangle([0, 0, w - 1, w - 1], radius=int(w * 0.2266), fill=TILE)
    for _k, x, y, r, col in shapes(w, tile):
        d.ellipse([x - r, y - r, x + r, y + r], fill=col)
    return im.resize((size, size), Image.LANCZOS)


def main() -> None:
    assets = HERE / "assets"
    icons = HERE / "app" / "src-tauri" / "icons"
    (assets / "mark.svg").write_text(svg(100, False))
    (assets / "icon.svg").write_text(svg(512, True))
    png(512, True).save(assets / "icon-512.png")
    png(512, False).save(assets / "mark-512.png")
    png(1024, True).save(icons / "icon.png")
    for size, name in ((32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png")):
        png(size, True).save(icons / name)
    png(1024, True).save(icons / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    png(1024, True).save(icons / "icon.icns")
    png(256, False).save(HERE / "app" / "ui" / "logo.png")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Draw the neutral stand-ins for Nexus's app icon and app mark, which the open-source repo leaves out (B2).

    python3 -I assets/held/make_app_icon.py

Writes beside this file, byte-stable, standard library only, no logo and no trademark:
- app-icon.png (256 px), app-icon.ico and app-icon.icns: a grey rounded tile with a white ring, the same ring as
  stand-in.svg. packaging/ uses them when `assets/icons/` is not in the tree.
- app-mark.svg: eleven concentric rings in the frame of `assets/brand/app-mark.svg` (its viewBox and scale, one closed
  96-point path per layer, rim first), so the code that cuts the mark into layers and animates them runs unchanged.
  build.rs uses it in place of `brand/app-mark.svg`.
"""

import math
import struct
import zlib
from pathlib import Path

SIZE = 256
TILE = (0x6B, 0x72, 0x80)  # neutral grey
RING = (0xFF, 0xFF, 0xFF)
RADIUS, CORNER = 82.0, 56.0  # the ring's centre line, and the tile's corner radius
STROKE = 18.0


def coverage(distance: float) -> float:
    """How much of a pixel lies inside an edge `distance` px away (negative is inside), over one pixel."""
    return min(1.0, max(0.0, 0.5 - distance))


def tile_distance(x: float, y: float) -> float:
    """Signed distance from (x, y) to the rounded tile's edge."""
    half = SIZE / 2
    qx, qy = abs(x - half) - (half - CORNER), abs(y - half) - (half - CORNER)
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    return outside + min(max(qx, qy), 0.0) - CORNER


def pixels() -> bytes:
    rows = bytearray()
    for j in range(SIZE):
        rows.append(0)  # filter: none
        for i in range(SIZE):
            x, y = i + 0.5, j + 0.5
            alpha = coverage(tile_distance(x, y))
            ring = coverage(abs(math.hypot(x - SIZE / 2, y - SIZE / 2) - RADIUS) - STROKE / 2)
            rgb = [round(t + (r - t) * ring) for t, r in zip(TILE, RING)]
            rows += bytes(rgb) + bytes([round(alpha * 255)])
    return bytes(rows)


def png() -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)  # 8-bit RGBA
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(pixels(), 9)) + chunk(b"IEND", b"")


def ico(image: bytes) -> bytes:
    """One 256 px PNG entry (Windows Vista and later read PNG inside .ico; 0 means 256)."""
    return struct.pack("<HHH", 0, 1, 1) + struct.pack("<BBBBHHII", 0, 0, 0, 0, 1, 32, len(image), 6 + 16) + image


def icns(image: bytes) -> bytes:
    """One `ic08` entry: a 256 px PNG."""
    entry = b"ic08" + struct.pack(">I", 8 + len(image)) + image
    return b"icns" + struct.pack(">I", 8 + len(entry)) + entry


def app_mark() -> str:
    """Eleven rings, rim first, each one closed path of 96 points in the app mark's units."""
    paths = []
    for layer in range(11):
        radius = 92 - 7.2 * layer
        points = [(radius * math.cos(k / 96 * math.tau), radius * math.sin(k / 96 * math.tau)) for k in range(96)]
        steps = " ".join(f"{'M' if k == 0 else 'L'}{x:.2f},{y:.2f}" for k, (x, y) in enumerate(points))
        paths.append(f'<path d="{steps} Z" fill="none" stroke="#000000" stroke-width="1.20" stroke-linejoin="round"/>')
    return ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="-84 -84 168 168"><g transform="scale(0.8853)">'
            + "".join(paths) + "</g></svg>\n")


if __name__ == "__main__":
    here = Path(__file__).resolve().parent
    image = png()
    (here / "app-icon.png").write_bytes(image)
    (here / "app-icon.ico").write_bytes(ico(image))
    (here / "app-icon.icns").write_bytes(icns(image))
    (here / "app-mark.svg").write_text(app_mark(), encoding="utf-8", newline="\n")

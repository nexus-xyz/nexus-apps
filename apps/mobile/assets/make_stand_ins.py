#!/usr/bin/env python3
"""Draw the neutral stand-ins for Nexus's app icon and splash, which the open-source repo leaves out (B2).

    python3 -I assets/make_stand_ins.py

Writes the nine images app.json names beside this file, byte-stable, standard library only, no logo and no
trademark: a white ring on a grey tile, as desktop's `assets/held/make_app_icon.py` draws. Each file keeps the size
and the color type of Nexus's own, so a build that uses the stand-ins behaves as one with the real icons.
A store release copies Nexus's icons over these (the private `nexus-brand` repository, .github/workflows/mobile-release.yml)
and refuses to build when any file still is a stand-in.
"""

import math
import struct
import zlib
from pathlib import Path

GREY = (0x6B, 0x72, 0x80)  # desktop's tile
DARK = (0x18, 0x18, 0x1A)  # the dark splash background
BLACK = (0x00, 0x00, 0x00)
WHITE = (0xFF, 0xFF, 0xFF)
LIGHT_RING = (0xD1, 0xD5, 0xDB)

# name: (size, background or None for transparent, ring color, ring radius and stroke as fractions of the size)
IMAGES = {
    "icon.png": (1024, GREY, WHITE, 0.32, 0.07),
    "icon-dark.png": (1024, DARK, LIGHT_RING, 0.32, 0.07),
    "icon-tinted.png": (1024, BLACK, WHITE, 0.32, 0.07),
    "android-icon.png": (512, GREY, WHITE, 0.32, 0.07),
    "android-icon-background.png": (432, GREY, None, 0.0, 0.0),
    "android-icon-foreground.png": (432, None, WHITE, 0.2, 0.045),  # inside the adaptive icon's safe zone
    "android-icon-monochrome.png": (432, None, BLACK, 0.2, 0.045),
    "splash-icon.png": (1024, None, GREY, 0.32, 0.07),
    "splash-icon-dark.png": (1024, None, LIGHT_RING, 0.32, 0.07),
}


def coverage(distance: float) -> float:
    """How much of a pixel lies inside an edge `distance` px away (negative is inside), over one pixel."""
    return min(1.0, max(0.0, 0.5 - distance))


def png(size: int, background, ring, radius: float, stroke: float) -> bytes:
    rgba = background is None
    rows = bytearray()
    half = size / 2
    for j in range(size):
        rows.append(0)  # filter: none
        for i in range(size):
            x, y = i + 0.5, j + 0.5
            cover = 0.0
            if ring is not None:
                cover = coverage(abs(math.hypot(x - half, y - half) - radius * size) - stroke * size / 2)
            if rgba:
                rows += bytes(ring) + bytes([round(cover * 255)])
            else:
                rows += bytes(round(b + (r - b) * cover) for b, r in zip(background, ring or background))

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    header = struct.pack(">IIBBBBB", size, size, 8, 6 if rgba else 2, 0, 0, 0)  # 8-bit RGBA or RGB
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(bytes(rows), 9)) + chunk(b"IEND", b"")


if __name__ == "__main__":
    here = Path(__file__).resolve().parent
    for name, spec in IMAGES.items():
        (here / name).write_bytes(png(*spec))

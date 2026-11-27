"""Tests for check.py: write small synthetic captures as PNGs and check the exit status.

    python3 -m unittest test_check      (from this directory)

Layout matches `src/web/selftest_pattern.rs`: bars on the top half, a check band on the next quarter (1 px squares
left, 2 px right), eight swatches on the bottom quarter.
"""

import os
import struct
import subprocess
import sys
import tempfile
import unittest
import zlib

import check

W, H = 640, 320


def pattern():
    rows = []
    for y in range(H):
        band = min(max(y * 4 // H, 1), 3)
        row = []
        for x in range(W):
            if band == 1:
                row.append((255, 255, 255) if x // 64 % 2 == 0 else (255, 255, 0))
            elif band == 2:
                size = 1 if x < W // 2 else 2
                row.append((0, 0, 0) if (x // size + y // size) % 2 == 0 else (255, 255, 255))
            else:
                row.append(check.SWATCHES[min(x * 8 // W, 7)])
        rows.append(row)
    return rows


def blur_band(rows, horizontal_only=False):
    """Average each band pixel with its right neighbour (and the one below): a frame drawn between pixels."""
    out = [list(r) for r in rows]
    for y in range(H // 2, H * 3 // 4):
        for x in range(W - 1):
            here, right = rows[y][x], rows[y][x + 1]
            below = rows[y + 1][x] if y + 1 < H else here
            out[y][x] = tuple((a + b + c) // 3 for a, b, c in zip(here, right, below))
    return out


def scale_band(rows):
    """Checks lost to a heavy downscale: the band becomes flat grey. Both halves score 0%."""
    out = [list(r) for r in rows]
    for y in range(H // 2, H * 3 // 4):
        out[y] = [(128, 128, 128)] * W
    return out


def repeat_row(rows):
    """The bug's case: a frame squeezed by one pixel resamples one row, which repeats its neighbour. The rest of the
    band stays exact (about 97% of each half), so only a tight EXACT catches it."""
    out = [list(r) for r in rows]
    y = H * 5 // 8
    out[y] = list(rows[y - 1])
    return out


def repeat_column(rows):
    """The same along x: one column of the 1 px half repeats its neighbour (about 99.3% of that half stays exact)."""
    out = [list(r) for r in rows]
    x = W // 4
    for y in range(H // 2, H * 3 // 4):
        out[y][x] = rows[y][x - 1]
    return out


def swap(rows):
    return [[(b, g, r) for r, g, b in row] for row in rows]


def shift_swatches(rows, by):
    out = [list(r) for r in rows]
    for y in range(H * 3 // 4, H):
        out[y] = [tuple(min(255, max(0, c + by)) for c in p) for p in rows[y]]
    return out


def png(rows, colour=2, interlace=0):
    raw = b"".join(b"\0" + b"".join(bytes(p) for p in row) for row in rows)
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, colour, 0, 0, interlace))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


class CheckTest(unittest.TestCase):
    def run_check(self, data):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "shot.png")
            with open(path, "wb") as f:
                f.write(data)
            here = os.path.dirname(os.path.abspath(__file__))
            done = subprocess.run([sys.executable, os.path.join(here, "check.py"), path],
                                  capture_output=True, text=True)
            return done.returncode, done.stdout

    def status(self, data):
        return self.run_check(data)[0]

    def test_sharp_passes(self):
        self.assertEqual(self.status(png(pattern())), 0)

    def test_scaled_fails(self):
        self.assertEqual(self.status(png(scale_band(pattern()))), 1)

    def test_one_repeated_row_fails(self):
        self.assertEqual(self.status(png(repeat_row(pattern()))), 1)

    def test_one_repeated_column_fails(self):
        self.assertEqual(self.status(png(repeat_column(pattern()))), 1)

    def test_half_pixel_blur_fails(self):
        self.assertEqual(self.status(png(blur_band(pattern()))), 1)

    def test_swapped_channels_fail(self):
        self.assertEqual(self.status(png(swap(pattern()))), 1)

    def test_colour_profile_shift_only_warns(self):
        status, out = self.run_check(png(shift_swatches(pattern(), 15)))
        self.assertEqual(status, 0)
        self.assertIn("WARN", out)

    def test_no_self_test_is_2(self):
        self.assertEqual(self.status(png([[(10, 10, 10)] * W for _ in range(H)])), 2)

    def test_unsupported_png_is_3(self):
        self.assertEqual(self.status(b"not a png at all"), 3)
        self.assertEqual(self.status(png(pattern(), interlace=1)), 3)

    def test_truncated_or_corrupt_png_is_3(self):
        data = png(pattern())
        # Cut inside the IHDR chunk (a short header), and halfway through the image data.
        self.assertEqual(self.status(data[:20]), 3)
        self.assertEqual(self.status(data[: len(data) // 2]), 3)
        # Image data that does not inflate: zero the bytes after IDAT's zlib header.
        start = data.index(b"IDAT") + 6
        self.assertEqual(self.status(data[:start] + bytes(64) + data[start + 64 :]), 3)


if __name__ == "__main__":
    unittest.main()

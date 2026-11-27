#!/usr/bin/env python3
"""Judge a screenshot of the web self-test (`NEXUS_WEB_SELFTEST=surface` or `surface-cpu`): PASS when the frame is
drawn pixel for pixel, FAIL when it is scaled, blurred or between pixels, or when red and blue are swapped.

    python3 check.py <screenshot.png>

Take the screenshot at the screen's full resolution, with the self-test window in front:
  macOS:   screencapture -x shot.png        (not a scaled copy, not a Retina "1x" export)
  Windows: Win+PrintScreen (Pictures\\Screenshots), at 100% zoom, not resized

The pattern (`src/web/selftest_pattern.rs`): moving colour bars on the top half; a check band on the next quarter, 1
device pixel squares on its left half and 2 pixel squares on its right; eight colour swatches on the bottom quarter.
On a high-density screen the 1 pixel checks look like flat grey to the eye, so this script judges them.

Standard library only (zlib for PNG). Exit status: 0 PASS, 1 FAIL, 2 the self-test was not found,
3 the file is not a PNG this script reads. PASS needs the checks
exact on both halves and red and blue in their places; a swatch off by more than a few levels only warns.
"""

import struct
import sys
import zlib

# The swatches, left to right, as red, green, blue (`SWATCHES` in selftest_pattern.rs).
SWATCHES = [
    (255, 0, 0),
    (0, 255, 0),
    (0, 0, 255),
    (128, 128, 128),
    (0x33, 0x66, 0x99),
    (0xF0, 0x80, 0x20),
    (255, 255, 255),
    (0, 0, 0),
]
# How far a pixel may be from black or white, and a swatch from its colour. Swatch colours only warn: a screenshot in
# the display's colour profile (macOS saves one) moves them. Red and blue swapped fails.
PURE = 24
COLOUR = 6
# How far a pixel may be from a swatch colour when looking for the swatch row. Wider than COLOUR, so a colour
# profile shift of more than COLOUR levels warns instead of hiding the row.
DETECT = 60
# The share of check pixels that must be exact.
EXACT = 0.999
# Pixels left out at each edge of a half, where the window's own edge or the middle seam may blend.
MARGIN = 6


def fail_input(message):
    print(message, file=sys.stderr)
    sys.exit(3)


def read_png(path):
    """Width, height and rows of (r, g, b) tuples, from an 8 or 16 bit RGB or RGBA PNG without interlace."""
    with open(path, "rb") as f:
        data = f.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        fail_input(f"{path}: not a PNG")
    try:
        return decode_png(path, data)
    except (struct.error, zlib.error) as error:
        # A short chunk or header (struct), or image data that does not inflate (zlib): truncated or corrupt.
        fail_input(f"{path}: truncated or corrupt PNG ({error})")


def decode_png(path, data):
    pos, idat, header = 8, [], None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat.append(body)
        elif kind == b"IEND":
            break
    if header is None:
        fail_input(f"{path}: PNG has no IHDR header")
    width, height, depth, colour, _, _, interlace = header
    if colour not in (2, 6) or depth not in (8, 16) or interlace:
        fail_input(f"{path}: PNG colour type {colour}, depth {depth}, interlace {interlace}: want RGB or RGBA, 8 or 16 bit")
    channels = 3 if colour == 2 else 4
    bpp = channels * depth // 8
    stride = width * bpp
    raw = zlib.decompress(b"".join(idat))
    if len(raw) < height * (stride + 1):
        fail_input(f"{path}: PNG image data is short: {len(raw)} bytes for {height} rows of {stride + 1}")
    rows, prev = [], bytearray(stride)
    for y in range(height):
        start = y * (stride + 1)
        kind = raw[start]
        line = bytearray(raw[start + 1 : start + 1 + stride])
        if kind == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 255
        elif kind == 2:
            line = bytearray((a + b) & 255 for a, b in zip(line, prev))
        elif kind == 3:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 255
        elif kind == 4:
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        prev = line
        step = depth // 8
        rows.append([
            (line[x], line[x + step], line[x + 2 * step]) for x in range(0, stride, bpp)
        ])
    return width, height, rows


def near(pixel, colour, tolerance):
    return all(abs(a - b) <= tolerance for a, b in zip(pixel, colour))


def runs(row, colour):
    """(start, end) of each run of at least 8 pixels near `colour` in `row`."""
    found, x = [], 0
    while x < len(row):
        if near(row[x], colour, DETECT):
            start = x
            while x < len(row) and near(row[x], colour, DETECT):
                x += 1
            if x - start >= 8:
                found.append((start, x))
        else:
            x += 1
    return found


def find_swatches(height, rows):
    """The swatch row, searched from the bottom: a run of red (or blue, if the channels are swapped) whose
    neighbour runs are green and then blue (or red). Returns the eight swatch centres, the row's top and bottom, and
    whether red and blue are swapped; or None."""
    for y in range(height - 1, -1, -1):
        row = rows[y]
        for first, third, swapped in ((SWATCHES[0], SWATCHES[2], False), (SWATCHES[2], SWATCHES[0], True)):
            for start, end in runs(row, first):
                run = end - start
                green = end + run // 2
                blue = end + run + run // 2
                if blue >= len(row) or not near(row[green], SWATCHES[1], DETECT) or not near(row[blue], third, DETECT):
                    continue
                # Swatch 8 (black) ends the row; its centre is 7 swatches on from the red one's.
                centre = (start + end) / 2
                black = [r for r in runs(row, SWATCHES[7]) if r[0] > end]
                if not black:
                    continue
                pitch = ((black[0][0] + black[0][1]) / 2 - centre) / 7
                if abs(pitch - run) > 3:
                    # The black run went on past the view (a black background): take the red run's width.
                    pitch = run
                centres = [round(centre + i * pitch) for i in range(8)]
                top = y
                while top > 0 and near(rows[top - 1][round(centre)], first, DETECT):
                    top -= 1
                return centres, pitch, top, y + 1, swapped
    return None


def check_band(rows, left, right, top, bottom, size):
    """The share of pixels in this rectangle that are pure black or white and alternate with the pixel `size` to the
    right and `size` below, as a checkerboard of `size` pixel squares does."""
    good = total = 0
    for y in range(top, bottom - size):
        row, below = rows[y], rows[y + size]
        for x in range(left, right - size):
            total += 1
            here = row[x]
            black = near(here, (0, 0, 0), PURE)
            white = near(here, (255, 255, 255), PURE)
            if not (black or white):
                continue
            target = (255, 255, 255) if black else (0, 0, 0)
            if near(row[x + size], target, PURE) and near(below[x], target, PURE):
                good += 1
    return good / total if total else 0.0, total


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    path = sys.argv[1]
    width, height, rows = read_png(path)
    print(f"{path}: {width}x{height} px")
    found = find_swatches(height, rows)
    if found is None:
        print("FAIL: no self-test found (no row of red, green and blue swatches). Is the window in front, and the "
              "screenshot at full resolution?")
        sys.exit(2)
    centres, pitch, top, bottom, swapped = found
    left = round(centres[0] - pitch / 2)
    view_width = round(pitch * 8)
    quarter = bottom - top
    band_top, band_bottom = top - quarter, top
    middle = left + view_width // 2
    print(f"self-test view at x {left}, about {view_width} px wide; check band rows {band_top} to {band_bottom}")
    if swapped:
        print("FAIL: red and blue are swapped (wrong channel order)")
    failed = swapped
    band_failed = False

    for name, size, (x0, x1) in (
        ("1 px checks (left half)", 1, (left + MARGIN, middle - MARGIN)),
        ("2 px checks (right half)", 2, (middle + MARGIN, left + view_width - MARGIN)),
    ):
        share, total = check_band(rows, x0, x1, band_top + MARGIN, band_bottom - MARGIN, size)
        ok = total > 0 and share >= EXACT
        band_failed |= not ok
        print(f"{'PASS' if ok else 'FAIL'}: {name}: {share * 100:.2f}% of {total} pixels exact")
    failed |= band_failed
    if band_failed:
        print("  Checks that are not exact mean the frame is scaled or drawn between pixels (or the screenshot "
              "is scaled).")

    y = (top + bottom) // 2
    for index, (colour, x) in enumerate(zip(SWATCHES, centres)):
        if x >= width:
            break
        got = rows[y][x]
        ok = near(got, colour, COLOUR)
        print(f"{'ok  ' if ok else 'WARN'}: swatch {index + 1} want #{bytes(colour).hex()} got #{bytes(got).hex()}")

    print("FAIL" if failed else "PASS")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()

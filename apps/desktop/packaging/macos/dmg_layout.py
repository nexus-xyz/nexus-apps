"""The macOS installer's window: write the .DS_Store that Finder reads when it opens the disk image.

    python3 -I packaging/macos/dmg_layout.py <mounted volume>

The window is the usual drag-to-install one: Nexus.app on the left and the Applications link on the right, at 128 px,
in a compact window with no toolbar or sidebar. Without this file, Finder opens the image with each
person's own default view: a large window with small icons.

The file is written here, with the standard library, so the release needs no Finder scripting (AppleScript times out
on CI runners) and no third-party package. The format is the one Finder writes: a buddy-allocated file whose B-tree
holds one record per (file name, property). test_dmg_layout.py reads it back.
"""

import plistlib
import struct
import sys
import time
from pathlib import Path

# The window's content area, in points. The title bar is added on top of it.
WINDOW_WIDTH = 600
WINDOW_HEIGHT = 380
# Where the window opens. Finder counts from the screen's bottom left corner.
WINDOW_LEFT = 200
WINDOW_BOTTOM = 200
ICON_SIZE = 128
TEXT_SIZE = 13
# Each icon's centre, in the window's content area. The icon and its label below it (about 140 points together)
# sit in the middle of the content area, so the icon's centre is 22 points above the middle.
APP_NAME = "Nexus.app"
BACKGROUND_IMAGE = Path(".background") / "Installer.tiff"
APP_POSITION = (160, 168)
APPLICATIONS_POSITION = (440, 168)
# The title bar of a window with no toolbar. Finder's window bounds include it.
TITLE_BAR = 28


def background_alias(root):
    """A Finder v2 alias to the TIFF on this HFS+ volume. Use only on-volume IDs and paths."""
    volume = root.stat()
    image = root / BACKGROUND_IMAGE
    target = image.stat()

    # HFS+ IDs survive hdiutil convert. Alias dates count seconds from 1904 in local time.
    def dates(stat):
        born = int(stat.st_birthtime)
        utc = born + 2082844800
        return utc + time.localtime(born).tm_gmtoff, utc << 16

    def field(kind, data):
        return struct.pack(">hH", kind, len(data)) + data + bytes(len(data) % 2)

    volume_date, volume_utc = dates(volume)
    file_date, file_utc = dates(target)
    relative = "/" + image.relative_to(root).as_posix()
    extra = b"".join((
        field(0, b".background"),
        field(16, struct.pack(">Q", volume_utc)),
        field(17, struct.pack(">Q", file_utc)),
        field(18, relative.encode("ascii")),
        field(-1, b""),
    ))
    # Alias Manager v2 fixed record: volume, parent ID, file name and ID, Finder metadata.
    record = struct.Struct(">4xHHh28pI2shI64pII8xhhI2x10x")
    return record.pack(
        record.size + len(extra), 2, 0, b"Nexus", volume_date, b"H+", 5,
        image.parent.stat().st_ino, image.name.encode("ascii"), target.st_ino,
        file_date, -1, -1, 0,
    ) + extra


def window_settings():
    """The window: its frame, and no toolbar, sidebar, path bar or status bar (`bwsp`)."""
    bounds = f"{{{{{WINDOW_LEFT}, {WINDOW_BOTTOM}}}, {{{WINDOW_WIDTH}, {WINDOW_HEIGHT + TITLE_BAR}}}}}"
    return {
        "WindowBounds": bounds,
        "ShowToolbar": False,
        "ShowSidebar": False,
        "ContainerShowSidebar": False,
        "ShowPathbar": False,
        "ShowStatusBar": False,
        "ShowTabView": False,
        "PreviewPaneVisibility": False,
        "SidebarWidth": 0,
    }


def icon_view_settings(background_alias):
    """The icon view: icon and text size, labels below, free placement (`icvp`)."""
    return {
        "viewOptionsVersion": 1,
        "arrangeBy": "none",
        "iconSize": float(ICON_SIZE),
        "textSize": float(TEXT_SIZE),
        "labelOnBottom": True,
        "showIconPreview": True,
        "showItemInfo": False,
        "gridSpacing": 100.0,
        "gridOffsetX": 0.0,
        "gridOffsetY": 0.0,
        "scrollPositionX": 0.0,
        "scrollPositionY": 0.0,
        "backgroundType": 2,
        "backgroundImageAlias": background_alias,
        "backgroundColorRed": 1.0,
        "backgroundColorGreen": 1.0,
        "backgroundColorBlue": 1.0,
    }


def records(background_alias):
    """Each (file name, property code, type code, value), in the B-tree's order: by name, ignoring case, then by code."""
    def blob(data):
        return struct.pack(">I", len(data)) + data

    def location(x, y):
        return blob(struct.pack(">IIII", x, y, 0xFFFFFFFF, 0xFFFF0000))

    rows = [
        (".", b"bwsp", b"blob", blob(plistlib.dumps(window_settings(), fmt=plistlib.FMT_BINARY))),
        (".", b"icvp", b"blob", blob(plistlib.dumps(icon_view_settings(background_alias), fmt=plistlib.FMT_BINARY))),
        (".", b"vSrn", b"long", struct.pack(">I", 1)),
        (".", b"vstl", b"type", b"icnv"),
        ("Applications", b"Iloc", b"blob", location(*APPLICATIONS_POSITION)),
        (APP_NAME, b"Iloc", b"blob", location(*APP_POSITION)),
    ]
    return sorted(rows, key=lambda r: (r[0].lower(), r[1]))


def encode_record(name, code, kind, value):
    utf16 = name.encode("utf-16-be")
    return struct.pack(">I", len(utf16) // 2) + utf16 + code + kind + value


# Where each block sits in the allocator's address space (file offset minus 4), and its size as a power of two.
# 0 to 32 is the header. Every other aligned block of the 2 GiB space that holds none of these is on a free list.
INFO_BLOCK = (2048, 11)
DSDB_BLOCK = (32, 5)
NODE_BLOCK = (4096, 12)
FREE = {6: [64], 7: [128], 8: [256], 9: [512], 10: [1024], **{n: [1 << n] for n in range(13, 31)}}


def ds_store(background_alias):
    """The whole .DS_Store file: one leaf node holding every record."""
    rows = records(background_alias)
    node = struct.pack(">II", 0, len(rows)) + b"".join(encode_record(*r) for r in rows)
    assert len(node) <= 1 << NODE_BLOCK[1], "the records outgrow one node"
    # DSDB: the root node's block number, the tree's internal levels, records, nodes, and the page size.
    dsdb = struct.pack(">IIIII", 2, 0, len(rows), 1, 0x1000)
    blocks = [INFO_BLOCK, DSDB_BLOCK, NODE_BLOCK]
    info = struct.pack(">II", len(blocks), 0)
    info += b"".join(struct.pack(">I", offset | log2) for offset, log2 in blocks)
    info += b"\0" * 4 * (256 - len(blocks))
    info += struct.pack(">I", 1) + struct.pack(">B", 4) + b"DSDB" + struct.pack(">I", 1)
    for n in range(32):
        free = FREE.get(n, [])
        info += struct.pack(">I", len(free)) + b"".join(struct.pack(">I", offset) for offset in free)
    space = bytearray(NODE_BLOCK[0] + (1 << NODE_BLOCK[1]))
    root, root_size = INFO_BLOCK[0], 1 << INFO_BLOCK[1]
    space[0:32] = b"Bud1" + struct.pack(">III", root, root_size, root) + b"\0" * 16
    for (offset, log2), data in zip(blocks, (info, dsdb, node)):
        assert len(data) <= 1 << log2
        space[offset : offset + len(data)] = data
    return struct.pack(">I", 1) + bytes(space)


def main(argv):
    if len(argv) != 2:
        print("usage: dmg_layout.py <mounted volume>", file=sys.stderr)
        return 2
    root = Path(argv[1])
    for name in (APP_NAME, "Applications"):
        if not (root / name).exists() and not (root / name).is_symlink():
            print(f"no {name} in {root}", file=sys.stderr)
            return 1
    if not (root / BACKGROUND_IMAGE).is_file():
        print(f"no {BACKGROUND_IMAGE} in {root}", file=sys.stderr)
        return 1
    (root / ".DS_Store").write_bytes(ds_store(background_alias(root)))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

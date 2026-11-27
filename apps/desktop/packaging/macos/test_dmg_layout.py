"""Tests for dmg_layout.py: the .DS_Store reads back as Finder reads it, and the layout fits its window.

    python3 -I -m unittest discover -s packaging/macos
"""

import plistlib
import struct
import tempfile
import unittest
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import dmg_layout as layout


def read_records(data):
    """Parse a .DS_Store as Finder does: the header, the info block, DSDB, then the one leaf node's records."""
    assert data[:8] == b"\0\0\0\x01Bud1", "not a .DS_Store"
    root, size, root_again = struct.unpack(">III", data[8:20])
    assert root == root_again
    info = data[4 + root : 4 + root + size]
    count, _ = struct.unpack(">II", info[:8])
    addresses = struct.unpack(f">{count}I", info[8 : 8 + 4 * count])
    toc = 8 + 4 * 256
    (entries,) = struct.unpack(">I", info[toc : toc + 4])
    assert entries == 1 and info[toc + 4] == 4 and info[toc + 5 : toc + 9] == b"DSDB"
    (dsdb_id,) = struct.unpack(">I", info[toc + 9 : toc + 13])

    def block(n):
        address = addresses[n]
        offset = address & ~0x1F
        return data[4 + offset : 4 + offset + (1 << (address & 0x1F))]

    node_id, levels, total, nodes, page = struct.unpack(">IIIII", block(dsdb_id)[:20])
    assert (levels, nodes, page) == (0, 1, 0x1000)
    node = block(node_id)
    leaf, n = struct.unpack(">II", node[:8])
    assert leaf == 0 and n == total
    out, at = {}, 8
    for _ in range(n):
        (length,) = struct.unpack(">I", node[at : at + 4])
        name = node[at + 4 : at + 4 + 2 * length].decode("utf-16-be")
        at += 4 + 2 * length
        code, kind = node[at : at + 4], node[at + 4 : at + 8]
        at += 8
        if kind == b"blob":
            (length,) = struct.unpack(">I", node[at : at + 4])
            value, at = node[at + 4 : at + 4 + length], at + 4 + length
        elif kind in (b"long", b"type"):
            value, at = node[at : at + 4], at + 4
        else:
            raise AssertionError(f"unexpected type {kind!r}")
        out[(name, code)] = (kind, value)
    return out


class DsStore(unittest.TestCase):
    def setUp(self):
        self.alias = b"test volume alias"
        self.records = read_records(layout.ds_store(self.alias))

    def test_it_holds_the_window_the_view_and_both_icons(self):
        self.assertEqual(
            set(self.records),
            {(".", b"bwsp"), (".", b"icvp"), (".", b"vSrn"), (".", b"vstl"),
             ("Applications", b"Iloc"), ("Nexus.app", b"Iloc")},
        )
        self.assertEqual(self.records[(".", b"vstl")], (b"type", b"icnv"))

    def test_the_window_has_no_toolbar_or_sidebar_and_fits_the_content(self):
        window = plistlib.loads(self.records[(".", b"bwsp")][1])
        self.assertEqual(window["WindowBounds"], "{{200, 200}, {600, 408}}")
        self.assertFalse(window["ShowToolbar"] or window["ShowSidebar"] or window["ShowStatusBar"])

    def test_the_icons_are_128_with_a_readable_label(self):
        view = plistlib.loads(self.records[(".", b"icvp")][1])
        self.assertEqual((view["iconSize"], view["textSize"], view["arrangeBy"]), (128.0, 13.0, "none"))
        self.assertTrue(view["labelOnBottom"])

    def test_the_view_uses_the_alias_from_the_mounted_image(self):
        view = plistlib.loads(self.records[(".", b"icvp")][1])
        self.assertEqual(view["backgroundType"], 2)
        self.assertEqual(view["backgroundImageAlias"], self.alias)

    def test_the_app_is_left_of_applications_on_one_line(self):
        def position(name):
            kind, value = self.records[(name, b"Iloc")]
            return struct.unpack(">II", value[:8])

        self.assertEqual(position("Nexus.app"), (160, 168))
        self.assertEqual(position("Applications"), (440, 168))

    def test_the_records_are_in_finders_order(self):
        names = [(name.lower(), code) for name, code in self.records]
        self.assertEqual(names, sorted(names))


class Layout(unittest.TestCase):
    def test_the_window_is_a_compact_installer(self):
        self.assertTrue(540 <= layout.WINDOW_WIDTH <= 660)
        self.assertTrue(360 <= layout.WINDOW_HEIGHT <= 420)

    def test_both_icons_and_labels_fit_inside_the_window_with_a_margin(self):
        half, label = layout.ICON_SIZE // 2, 40
        for x, y in (layout.APP_POSITION, layout.APPLICATIONS_POSITION):
            self.assertGreaterEqual(x - half, 48)
            self.assertLessEqual(x + half, layout.WINDOW_WIDTH - 48)
            self.assertGreaterEqual(y - half, 48)
            self.assertLessEqual(y + half + label, layout.WINDOW_HEIGHT - 48)

    def test_the_icons_are_centred_and_apart(self):
        (ax, ay), (bx, by) = layout.APP_POSITION, layout.APPLICATIONS_POSITION
        self.assertEqual(ay, by)
        self.assertEqual(ax + bx, layout.WINDOW_WIDTH)
        self.assertGreaterEqual(bx - ax, 2 * layout.ICON_SIZE)


class Main(unittest.TestCase):
    def test_alias_locates_the_file_after_the_build_mount_is_gone(self):
        root = Path("/private/build/temporary-mount")
        target = root / ".background" / "Installer.tiff"
        ids = {root: 2, target.parent: 27, target: 28}

        def hfs_stat(path):
            return SimpleNamespace(st_birthtime=1700000000, st_ino=ids[path])

        with patch.object(Path, "stat", hfs_stat):
            alias = layout.background_alias(root)
        self.assertEqual(struct.unpack_from(">HH", alias, 4), (len(alias), 2))
        self.assertEqual(struct.unpack_from(">I", alias, 46)[0], 27)
        self.assertEqual(struct.unpack_from(">I", alias, 114)[0], 28)
        self.assertIn(b"/.background/Installer.tiff", alias)
        self.assertNotIn(bytes(root), alias)

    def test_it_writes_the_file_into_the_images_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Nexus.app").mkdir()
            (root / "Applications").symlink_to("/Applications")
            (root / ".background").mkdir()
            (root / ".background" / "Installer.tiff").touch()
            with patch.object(layout, "background_alias", return_value=b"test alias") as alias:
                self.assertEqual(layout.main(["dmg_layout.py", str(root)]), 0)
                alias.assert_called_once_with(root)
            self.assertEqual((root / ".DS_Store").read_bytes(), layout.ds_store(b"test alias"))

    def test_it_names_the_missing_background(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Nexus.app").mkdir()
            (root / "Applications").symlink_to("/Applications")
            stderr = StringIO()
            with redirect_stderr(stderr):
                self.assertEqual(layout.main(["dmg_layout.py", str(root)]), 1)
            self.assertIn(f"no .background/Installer.tiff in {root}", stderr.getvalue())
            self.assertFalse((root / ".DS_Store").exists())

    def test_it_refuses_a_root_without_the_app(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(layout.main(["dmg_layout.py", tmp]), 1)
            self.assertFalse((Path(tmp) / ".DS_Store").exists())


if __name__ == "__main__":
    unittest.main()

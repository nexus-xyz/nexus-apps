"""Tests for feed.py. Run them with `python3 -m unittest apps/desktop/packaging/test_feed.py`.

`testdata/latest.json` is the manifest feed.py writes for the files `release()` makes. The updater crate parses the
same file (crates/nexus-desktop-updates/tests/release_feed.rs), so the script and the app agree on the format.
"""

import base64
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import feed  # noqa: E402

VERSION = "1.2.3-beta.4"
BASE = "https://downloads.example/desktop-oss/beta/1.2.3-beta.4"
GOLDEN = Path(__file__).parent / "testdata" / "latest.json"


def signature(file: str, version: str = VERSION) -> str:
    """A stand-in with the layout of a minisign signature file. No key signed it."""
    text = (
        "untrusted comment: signature from minisign secret key\n"
        "c3RhbmQtaW4=\n"
        f"trusted comment: timestamp:1700000000\tfile:{file}\tversion:{version}\n"
        "c3RhbmQtaW4=\n"
    )
    return base64.b64encode(text.encode()).decode()


def release(folder: Path) -> None:
    files = [feed.package_file(VERSION, suffix) for _, _, suffix in feed.PACKAGES]
    files += [feed.host_file(VERSION, arch) for arch in feed.HOST_ARCHES]
    for file in files:
        (folder / file).write_text(f"stand-in for {file}\n")
        (folder / f"{file}.sig").write_text(signature(file) + "\n")


def build(folder: Path):
    return feed.build(folder, VERSION, "beta", BASE + "/", "0123abc", "2026-01-02T03:04:05Z")


class FeedTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.folder = Path(self.temp.name)
        release(self.folder)

    def tearDown(self):
        self.temp.cleanup()

    def test_the_manifest_is_the_golden_file(self):
        manifest, sums = build(self.folder)
        self.assertEqual(manifest, json.loads(GOLDEN.read_text()))
        self.assertEqual(len(sums.splitlines()), 5)
        self.assertTrue(all(line.split("  ")[1] in {f.name for f in self.folder.iterdir()} for line in sums.splitlines()))

    def test_the_app_keys_and_the_host_entries(self):
        manifest, _ = build(self.folder)
        self.assertEqual(
            sorted(manifest["platforms"]), ["darwin-aarch64-dmg", "linux-x86_64-tarball", "windows-x86_64-exe"]
        )
        for arch in ("x86_64", "aarch64"):
            host = manifest["downloads"][f"linux-{arch}-nexus-host"]
            self.assertEqual(host["url"], f"{BASE}/nexus-host_{VERSION}_linux-{arch}.gz")
            self.assertIn("signature", host)
        self.assertNotIn("signature", manifest["downloads"]["macos-arm64-dmg"])

    def test_a_missing_package_or_signature_stops_the_feed(self):
        dmg = feed.package_file(VERSION, "macos-arm64.dmg")
        (self.folder / f"{dmg}.sig").unlink()
        with self.assertRaisesRegex(feed.FeedError, "has no signature"):
            build(self.folder)
        (self.folder / dmg).unlink()
        with self.assertRaisesRegex(feed.FeedError, "is missing"):
            build(self.folder)

    def test_a_signature_for_another_file_or_version_stops_the_feed(self):
        dmg = feed.package_file(VERSION, "macos-arm64.dmg")
        (self.folder / f"{dmg}.sig").write_text(signature("another.dmg"))
        with self.assertRaisesRegex(feed.FeedError, "signed for the file"):
            build(self.folder)
        (self.folder / f"{dmg}.sig").write_text(signature(dmg, "1.2.3-beta.3"))
        with self.assertRaisesRegex(feed.FeedError, "signed for version"):
            build(self.folder)
        (self.folder / f"{dmg}.sig").write_text("not base64")
        with self.assertRaisesRegex(feed.FeedError, "not the base64"):
            build(self.folder)

    def test_a_file_that_is_not_part_of_the_release_stops_the_feed(self):
        (self.folder / "notes.txt").write_text("x")
        with self.assertRaisesRegex(feed.FeedError, "not part of the release: notes.txt"):
            build(self.folder)

    def test_a_bad_version_stops_the_feed(self):
        with self.assertRaisesRegex(feed.FeedError, "is not a version"):
            feed.build(self.folder, "v1.2", "beta", BASE, "0123abc", "2026-01-02T03:04:05Z")

    def test_the_text_keeps_each_download_on_one_line(self):
        manifest, _ = build(self.folder)
        text = feed.render(manifest)
        self.assertEqual(text, GOLDEN.read_text())
        self.assertEqual(json.loads(text), manifest)
        downloads = [line.rstrip(",") for line in text.split('"downloads": {\n')[1].splitlines()]
        for kind, entry in manifest["downloads"].items():
            self.assertIn(f'    "{kind}": {json.dumps(entry, separators=(",", ":"))}', downloads)

    def test_a_version_or_folder_of_another_channel_stops_the_feed(self):
        date = "2026-01-02T03:04:05Z"
        with self.assertRaisesRegex(feed.FeedError, "is not a stable version"):
            feed.build(self.folder, VERSION, "stable", BASE, "0123abc", date)
        with self.assertRaisesRegex(feed.FeedError, "is not a channel"):
            feed.build(self.folder, VERSION, "internal", BASE, "0123abc", date)
        stable_base = f"https://downloads.example/desktop/stable/{VERSION}"
        with self.assertRaisesRegex(feed.FeedError, "is not the beta folder"):
            feed.build(self.folder, VERSION, "beta", stable_base, "0123abc", date)


class StableFeedTest(unittest.TestCase):
    """Stable writes the folder and the layout that Nexus's stable builds and installers already read."""

    def test_the_text_is_the_earlier_release_s(self):
        # desktop/stable/latest.json as Nexus's earlier release published it for 0.11.2, byte for byte.
        published = (Path(__file__).parent / "testdata" / "stable-0.11.2.json").read_text()
        self.assertEqual(feed.render(json.loads(published)), published)

    def test_a_stable_release(self):
        version = "1.3.0"
        base = f"https://downloads.nexus.inc/desktop/stable/{version}"
        date = "2026-01-02T03:04:05Z"
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            files = [feed.package_file(version, suffix) for _, _, suffix in feed.PACKAGES]
            files += [feed.host_file(version, arch) for arch in feed.HOST_ARCHES]
            for file in files:
                (folder / file).write_text(f"stand-in for {file}\n")
                (folder / f"{file}.sig").write_text(signature(file, version) + "\n")
            manifest, _ = feed.build(folder, version, "stable", base, "0123abc", date)
            with self.assertRaisesRegex(feed.FeedError, "is not a beta version"):
                feed.build(folder, version, "beta", base, "0123abc", date)
        self.assertEqual(manifest["channel"], "stable")
        self.assertEqual(manifest["notes"], f"Nexus {version} (stable)")
        self.assertEqual(
            list(manifest["platforms"]), ["darwin-aarch64-dmg", "windows-x86_64-exe", "linux-x86_64-tarball"]
        )
        self.assertEqual(
            manifest["downloads"]["linux-x86_64-tarball"]["url"], f"{base}/nexus-desktop-{version}-linux-x86_64.tar.xz"
        )


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Write a release's update feed: `latest.json` and `SHA256SUMS`, from a folder of signed packages.

    feed.py --dir <packages> --version 0.12.0-beta.1 --channel beta \
        --base https://downloads.nexus.inc/desktop-oss/beta/0.12.0-beta.1 --commit <sha> --date <RFC 3339>

The folder holds each package and host, with its signature beside it as `<file>.sig`: the base64 of a minisign
signature file, the format the app's updater reads (crates/nexus-desktop-updates, `verify`).

The app reads `platforms["<os>-<arch>-<format>"]` for its update and `downloads["linux-<arch>-nexus-host"]` for the
Nexus host. A download page reads `downloads`, every file by kind.

The script does not check a signature against a key. It checks what a manifest can get wrong: a missing package, a
missing signature, and a signature whose trusted comment names another file or another version. The release
workflow verifies each signature with minisign before it runs this script.
"""

import argparse
import base64
import hashlib
import json
import re
import sys
from pathlib import Path

# The packages a release must hold: (download kind, the app's update key, the file's name after the version).
PACKAGES = [
    ("linux-x86_64-tarball", "linux-x86_64-tarball", "linux-x86_64.tar.xz"),
    ("macos-arm64-dmg", "darwin-aarch64-dmg", "macos-arm64.dmg"),
    ("windows-x86_64-nsis", "windows-x86_64-exe", "windows-x86_64.exe"),
]
HOST_ARCHES = ["x86_64", "aarch64"]
VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$")


class FeedError(Exception):
    """The folder cannot make a feed. The message says which file and why."""


def package_file(version: str, suffix: str) -> str:
    return f"nexus-desktop-{version}-{suffix}"


def host_file(version: str, arch: str) -> str:
    return f"nexus-host_{version}_linux-{arch}.gz"


def trusted_fields(signature: str, file: str) -> dict:
    """The fields of a signature's trusted comment, such as `file` and `version`."""
    try:
        text = base64.b64decode(signature, validate=True).decode()
    except (ValueError, UnicodeDecodeError) as error:
        raise FeedError(f"{file}.sig is not the base64 of a minisign signature file") from error
    for line in text.splitlines():
        if line.startswith("trusted comment: "):
            pairs = (field.split(":", 1) for field in line[len("trusted comment: ") :].split("\t"))
            return {pair[0]: pair[1] for pair in pairs if len(pair) == 2}
    raise FeedError(f"{file}.sig has no trusted comment")


def signature_of(folder: Path, file: str, version: str) -> str:
    """A file's signature, after a check that it names this file and this version."""
    path = folder / f"{file}.sig"
    if not path.is_file():
        raise FeedError(f"{file} has no signature ({file}.sig)")
    signature = path.read_text().strip()
    fields = trusted_fields(signature, file)
    if fields.get("file") != file:
        raise FeedError(f"{file}.sig is signed for the file {fields.get('file')!r}")
    if fields.get("version", "").removeprefix("v") != version:
        raise FeedError(f"{file}.sig is signed for version {fields.get('version')!r}, not {version}")
    return signature


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def build(folder: Path, version: str, channel: str, base: str, commit: str, date: str) -> tuple[dict, str]:
    """The manifest and the text of SHA256SUMS."""
    if not VERSION.match(version):
        raise FeedError(f"{version!r} is not a version (MAJOR.MINOR.PATCH[-prerelease])")
    base = base.rstrip("/")
    expected = [package_file(version, suffix) for _, _, suffix in PACKAGES]
    expected += [host_file(version, arch) for arch in HOST_ARCHES]
    for file in expected:
        if not (folder / file).is_file():
            raise FeedError(f"{file} is missing")
    known = set(expected) | {f"{file}.sig" for file in expected}
    extra = sorted(entry.name for entry in folder.iterdir() if entry.name not in known)
    if extra:
        raise FeedError(f"files that are not part of the release: {', '.join(extra)}")

    platforms, downloads = {}, {}
    for kind, key, suffix in PACKAGES:
        file = package_file(version, suffix)
        path = folder / file
        url = f"{base}/{file}"
        platforms[key] = {"url": url, "signature": signature_of(folder, file, version)}
        downloads[kind] = {"url": url, "sha256": sha256(path), "size": path.stat().st_size}
    for arch in HOST_ARCHES:
        file = host_file(version, arch)
        path = folder / file
        downloads[f"linux-{arch}-nexus-host"] = {
            "url": f"{base}/{file}",
            "sha256": sha256(path),
            "size": path.stat().st_size,
            "signature": signature_of(folder, file, version),
        }
    manifest = {
        "version": version,
        "notes": f"Nexus {version} ({channel})",
        "pub_date": date,
        "channel": channel,
        "commit": commit,
        "platforms": platforms,
        "downloads": downloads,
    }
    sums = "".join(f"{sha256(folder / file)}  {file}\n" for file in sorted(expected))
    return manifest, sums


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    for name in ("dir", "version", "channel", "base", "commit", "date"):
        parser.add_argument(f"--{name}", required=True)
    args = parser.parse_args()
    folder = Path(args.dir)
    try:
        manifest, sums = build(folder, args.version, args.channel, args.base, args.commit, args.date)
    except FeedError as error:
        print(f"feed.py: {error}", file=sys.stderr)
        return 1
    # The two files are written last, so a failed run leaves the folder as it was.
    (folder / "SHA256SUMS").write_text(sums)
    (folder / "latest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())

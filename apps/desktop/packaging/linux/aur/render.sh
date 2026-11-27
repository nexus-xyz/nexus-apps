#!/usr/bin/env bash
# Move nexus-desktop-bin/PKGBUILD and .SRCINFO to a stable release's version and tarball checksum.
#
#   packaging/linux/aur/render.sh <latest.json>     # desktop/stable/latest.json from downloads.nexus.inc
#
# It reads the version and downloads.linux-x86_64-tarball.sha256 from the manifest, and the license from the app's
# Cargo.toml. Publishing to the AUR is a push of this folder to ssh://aur@aur.archlinux.org/nexus-desktop-bin.git.
set -euo pipefail

manifest="${1:?usage: render.sh <latest.json>}"
here="$(cd "$(dirname "$0")" && pwd)"
pkg="$here/nexus-desktop-bin"

version="$(jq -r 'select(.channel == "stable") | .version' "$manifest")"
sha="$(jq -r '.downloads["linux-x86_64-tarball"].sha256' "$manifest")"
url="$(jq -r '.downloads["linux-x86_64-tarball"].url' "$manifest")"
license="$(sed -n 's/^license = "\(.*\)"$/\1/p' "$here/../../../Cargo.toml" | head -n 1)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not a stable manifest: $manifest" >&2; exit 1; }
[[ "$sha" =~ ^[0-9a-f]{64}$ ]] || { echo "no tarball sha256 in $manifest" >&2; exit 1; }
[[ "$url" == "https://downloads.nexus.inc/desktop/stable/$version/nexus-desktop-$version-linux-x86_64.tar.xz" ]] ||
  { echo "unexpected tarball url: $url" >&2; exit 1; }
[[ -n "$license" ]] || { echo "no license in Cargo.toml" >&2; exit 1; }

pkgbuild="$(sed -e "s/^pkgver=.*/pkgver=$version/" -e "s/^pkgrel=.*/pkgrel=1/" \
  -e "s/^sha256sums=.*/sha256sums=('$sha')/" -e "s/^license=.*/license=('$license')/" "$pkg/PKGBUILD")"
printf '%s\n' "$pkgbuild" > "$pkg/PKGBUILD"

# Write .SRCINFO without makepkg so the renderer also runs on macOS.
deps="$(sed -n "/^depends=(/,/)/p" "$pkg/PKGBUILD" | tr -d "()'" | sed 's/^depends=//' | tr -s ' \n' '\n' | sed '/^$/d')"
{
  echo "pkgbase = nexus-desktop-bin"
  echo "	pkgdesc = $(sed -n 's/^pkgdesc="\(.*\)"$/\1/p' "$pkg/PKGBUILD")"
  echo "	pkgver = $version"
  echo "	pkgrel = 1"
  echo "	url = https://nexus.inc/"
  echo "	arch = x86_64"
  echo "	license = $license"
  while read -r dep; do echo "	depends = $dep"; done <<< "$deps"
  echo "	optdepends = xdg-utils: open nexus:// links in the app"
  echo "	provides = nexus-desktop"
  echo "	conflicts = nexus-desktop"
  echo "	options = !strip"
  echo "	options = !debug"
  echo "	source = $url"
  echo "	sha256sums = $sha"
  echo
  echo "pkgname = nexus-desktop-bin"
} > "$pkg/.SRCINFO"
echo "nexus-desktop-bin $version-1"

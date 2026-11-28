#!/usr/bin/env bash
# Make the Linux system packages from the release tarball that package.sh wrote:
#
#   packaging/linux/packages.sh ../../target/package/nexus-desktop-<ver>-linux-x86_64.tar.xz ../../target/package
#
# writes, beside the tarball:
#   nexus-desktop_<ver>_amd64.deb                 Debian, Ubuntu (22.04 or later: the binary needs glibc 2.35)
#   nexus-desktop-<ver>-1.x86_64.rpm              Fedora, RHEL 10, openSUSE
#   nexus-desktop-<ver>-linux-x86_64.AppImage     any x86_64 distribution with FUSE
#
# The .deb and .rpm install the app in /opt/nexus-desktop, with /usr/bin/nexus-desktop-gpui, a menu entry and the
# nexus:// handler. They leave out install.sh, so the app's own updater stays off (src/updates/install.rs: `packaged`
# needs install.sh beside the binary). Install updates manually until a package repository exists.
# The AppImage also leaves out install.sh because its files are read-only.
#
# Needs nfpm and appimagetool on PATH. A release build installs both at pinned versions.
set -euo pipefail

tarball="$(realpath "${1:?usage: packages.sh <tarball> <out dir>}")"
out="$(realpath "${2:?usage: packages.sh <tarball> <out dir>}")"
here="$(cd "$(dirname "$0")" && pwd)"
bin=nexus-desktop-gpui

name="$(basename "$tarball" .tar.xz)"
[[ "$name" =~ ^nexus-desktop-(.+)-linux-x86_64$ ]] || { echo "not a Linux release tarball: $tarball" >&2; exit 1; }
full="${BASH_REMATCH[1]}"
# An internal build is <ver>-internal.<run>. Packages sort a "~" pre-release before the release, so it becomes
# 0.11.2~internal.123, and the next stable 0.11.2 upgrades it.
version="${full%%-*}"
prerelease=""
[[ "$full" == *-* ]] && prerelease="${full#*-}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "bad version in $name: $full" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
tar -C "$work" -xJf "$tarball"
app="$work/$name"
# CEF-LICENSE.txt is in every tarball from 0.11.2 on; older ones lack it, and the packages still build.
for need in "$bin" libcef.so "$bin.desktop" "$bin.png"; do
  [[ -e "$app/$need" ]] || { echo "$name has no $need" >&2; exit 1; }
done

# The tree both system packages install. nfpm.yaml names each part.
root="$work/root"
mkdir -p "$root/opt/nexus-desktop" "$root/share"
cp -a "$app/." "$root/opt/nexus-desktop/"
rm -f "$root/opt/nexus-desktop/install.sh" "$root/opt/nexus-desktop/$bin.desktop" "$root/opt/nexus-desktop/$bin.png"
cp "$app/$bin.desktop" "$root/share/$bin.desktop"
cp "$app/$bin.png" "$root/share/$bin.png"

# The app crate's own license field, so the packages never disagree with the code.
license="$(sed -n 's/^license = "\(.*\)"$/\1/p' "$here/../../Cargo.toml" | head -n 1)"
[[ -n "$license" ]] || { echo "no license field in Cargo.toml" >&2; exit 1; }
# nfpm expands environment variables in only a few fields, not in `contents`, so the placeholders are filled here.
sed -e "s|\${NEXUS_ROOT}|$root|g" -e "s|\${NEXUS_LINUX}|$here|g" -e "s|\${NEXUS_VERSION}|$version|g" \
  -e "s|\${NEXUS_PRERELEASE}|$prerelease|g" -e "s|\${NEXUS_LICENSE}|$license|g" "$here/nfpm.yaml" > "$work/nfpm.yaml"
if grep -q '\${NEXUS_' "$work/nfpm.yaml"; then echo "nfpm.yaml has an unfilled placeholder" >&2; exit 1; fi
deb="$out/nexus-desktop_${full}_amd64.deb"
rpm="$out/nexus-desktop-${full}-1.x86_64.rpm"
nfpm package --config "$work/nfpm.yaml" --packager deb --target "$deb"
nfpm package --config "$work/nfpm.yaml" --packager rpm --target "$rpm"

# The AppImage: AppRun starts the app from its own folder, where CEF sits beside the binary.
appdir="$work/Nexus.AppDir"
mkdir -p "$appdir/usr/lib/nexus-desktop"
cp -a "$root/opt/nexus-desktop/." "$appdir/usr/lib/nexus-desktop/"
install -m 755 "$here/AppRun" "$appdir/AppRun"
sed -e "s|^Exec=.*|Exec=$bin %u|" "$app/$bin.desktop" > "$appdir/$bin.desktop"
cp "$app/$bin.png" "$appdir/$bin.png"
ln -s "$bin.png" "$appdir/.DirIcon"
image="$out/nexus-desktop-${full}-linux-x86_64.AppImage"
# APPIMAGE_EXTRACT_AND_RUN: CI runners have no FUSE for appimagetool's own AppImage. A pinned runtime, not a download.
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 appimagetool --no-appstream ${APPIMAGE_RUNTIME:+--runtime-file "$APPIMAGE_RUNTIME"} \
  "$appdir" "$image" > /dev/null
chmod 755 "$image"

ls -l "$deb" "$rpm" "$image"

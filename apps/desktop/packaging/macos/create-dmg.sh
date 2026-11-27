#!/usr/bin/env bash
# Build the Finder layout on the writable volume so its background alias belongs to this image.
set -euo pipefail

root=$1
output=$2
assets="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d "$(dirname "$output")/dmg-build.XXXXXX")"
mount="$work/volume"
mkdir "$mount"
mounted=false

detach_volume() {
  local delay
  for delay in 1 2; do
    if hdiutil detach -quiet "$mount"; then
      mounted=false
      return 0
    fi
    sleep "$delay"
  done
  if ! hdiutil detach -quiet -force "$mount"; then
    echo "Could not detach $mount" >&2
    return 1
  fi
  mounted=false
}

cleanup() {
  if [[ "$mounted" == true ]]; then
    detach_volume || return 1
  fi
  rmdir "$mount" || return 1
  rm -f "$work/writable.dmg" "$work/installer.dmg"
  rmdir "$work"
}
trap cleanup EXIT

# installer-background.tiff is Nexus artwork, drawn for this installer: the Nexus wordmark, a drag arrow and a contour
# pattern, with no third-party source. The wordmark is a Nexus mark (TRADEMARKS.md): a fork replaces this image.
mkdir -p "$root/.background"
cp "$assets/installer-background.tiff" "$root/.background/Installer.tiff"
hdiutil create -quiet -fs HFS+ -volname Nexus -srcfolder "$root" -format UDRW "$work/writable.dmg"
hdiutil attach -quiet -nobrowse -noautoopen -mountpoint "$mount" "$work/writable.dmg"
mounted=true
python3 -I "$assets/dmg_layout.py" "$mount"
detach_volume
hdiutil convert -quiet "$work/writable.dmg" -format ULMO -o "$work/installer.dmg"
mv -f "$work/installer.dmg" "$output"

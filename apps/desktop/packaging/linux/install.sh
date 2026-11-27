#!/usr/bin/env sh
# Add Nexus to the app launcher, run from this folder where it is unpacked. Move the folder, run it again.
# It also opens nexus:// links (links.rs reads them from the arguments), and takes over from an older Nexus
# install (0.10.x and before, per user): its menu entries go, so the menu shows one Nexus, and its
# login item (shared by name, nexus-desktop-core's LOGIN_ITEM) starts this binary when it is on.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
apps="$data/applications"
id="${NEXUS_DESKTOP_APP_ID:-nexus-desktop-gpui}"
case "$id" in
  . | .. | *[!a-zA-Z0-9._-]*) echo "Invalid NEXUS_DESKTOP_APP_ID: $id" >&2; exit 1 ;;
esac
mkdir -p "$apps"
sed -e "s|^Exec=.*|Exec=env NEXUS_DESKTOP_APP_ID=$id \"$here/nexus-desktop-gpui\" %u|" -e "s|^Icon=.*|Icon=$here/nexus-desktop-gpui.png|" \
  -e "s|^StartupWMClass=.*|StartupWMClass=$id|" \
  "$here/nexus-desktop-gpui.desktop" > "$apps/$id.desktop"
echo "Added $apps/$id.desktop"

for old in nexus-desktop xyz.nexus.desktop; do
  if [ "$old" != "$id" ] && [ -f "$apps/$old.desktop" ]; then
    rm -f "$apps/$old.desktop" && echo "Removed the older Nexus install's $apps/$old.desktop"
  fi
done
if command -v update-desktop-database > /dev/null 2>&1; then
  update-desktop-database -q "$apps" > /dev/null 2>&1 || true
fi
if command -v xdg-mime > /dev/null 2>&1; then
  xdg-mime default "$id.desktop" x-scheme-handler/nexus > /dev/null 2>&1 || true
fi

autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/Nexus.desktop"
if [ -f "$autostart" ]; then
  sed -i "s|^Exec=.*|Exec=$here/nexus-desktop-gpui --autostart|" "$autostart" && echo "Pointed the login item at $here"
fi

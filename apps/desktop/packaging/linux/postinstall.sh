#!/bin/sh
# After the .deb or .rpm installs, upgrades or removes Nexus: refresh the menu and icon caches, so the launcher shows
# the entry and nexus:// links open the app. Each tool is optional; a missing one is not an error.
if command -v update-desktop-database > /dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache > /dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
exit 0

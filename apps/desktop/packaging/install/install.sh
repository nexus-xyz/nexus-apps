#!/bin/sh
# Installs Nexus Desktop from the public stable channel. Run it with:
#
#   curl -fsSL https://nexus.inc/install | sh
#
# Windows has its own script: irm https://nexus.inc/install.ps1 | iex
# (/install.sh is the CLI's installer, not this one.)
#
# It reads https://downloads.nexus.inc/desktop/stable/latest.json, downloads the file for this machine, checks its
# SHA-256 and installs it. Run it again to update. It needs no account, no gcloud, no jq and no python.
#
# macOS (Apple silicon): Nexus.app into /Applications, or ~/Applications when /Applications is not writable.
#   It installs only when Apple confirms a notarized Developer ID.
# Linux (x86_64, glibc): the .tar.xz folder in ~/.local/share/nexus-desktop-gpui, a link at
#   ~/.local/bin/nexus-desktop-gpui, and a menu entry from the package's own install.sh.
#
# Options, on the sh side of the pipe: `... | sh -s -- --no-launch`
#   --no-launch, NEXUS_NO_LAUNCH=1   do not open the app at the end
#   NEXUS_APP_DIR=<dir>              macOS: where Nexus.app goes (default /Applications)
#   NEXUS_INSTALL_DIR=<dir>          Linux: where the folder goes (default ~/.local/share/nexus-desktop-gpui)
#
# POSIX sh. Everything runs from main() at the last line, so a download cut short runs nothing.
set -eu

MANIFEST_URL="https://downloads.nexus.inc/desktop/stable/latest.json"
FILE_PREFIX="https://downloads.nexus.inc/desktop/stable/"

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}
need() { command -v "$1" > /dev/null 2>&1 || die "$1 is required. $2"; }

# entry <manifest> <kind>: the text inside the `downloads` entry named <kind>, empty when there is none.
entry() { tr -d '\r\n' < "$1" | sed -n "s/.*\"$2\": *{\\([^}]*\\)}.*/\\1/p"; }
# field <entry> <name>: a string field of an entry.
field() { printf '%s\n' "$1" | sed -n "s/.*\"$2\": *\"\\([^\"]*\\)\".*/\\1/p"; }
# number <entry> <name>: a number field of an entry.
number() { printf '%s\n' "$1" | sed -n "s/.*\"$2\": *\\([0-9][0-9]*\\).*/\\1/p"; }

sha256() {
  if command -v sha256sum > /dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif command -v shasum > /dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    die "sha256sum or shasum is required to check the download"
  fi
}

# fetch <kind>: downloads the file for <kind> into $TMP, checks it, and prints its path.
fetch() {
  line="$(entry "$MANIFEST" "$1")"
  [ -n "$line" ] || die "Nexus $VERSION has no $1 download"
  url="$(field "$line" url)"
  want="$(field "$line" sha256)"
  size="$(number "$line" size)"
  [ -n "$url" ] && [ -n "$want" ] || die "the $1 entry in the manifest has no url or sha256"
  case "$url" in
    "$FILE_PREFIX"*) ;;
    *) die "the $1 download address is outside $FILE_PREFIX" ;;
  esac
  case "$want" in
    *[!0-9a-f]*) die "the $1 sha256 in the manifest is not a lowercase hex digest" ;;
  esac
  [ "${#want}" -eq 64 ] || die "the $1 sha256 in the manifest is not 64 characters"
  file="$TMP/$(basename "$url")"
  # fetch's stdout is the file's path, so its messages go to stderr.
  if [ -n "$size" ]; then
    say "Downloading $(basename "$url") ($((size / 1048576)) MB)" >&2
  else
    say "Downloading $(basename "$url")" >&2
  fi
  # A stalled connection (under 1 KB/s for 60 s) fails instead of hanging.
  curl -fSL --progress-bar --retry 3 --connect-timeout 15 --speed-limit 1024 --speed-time 60 -o "$file" "$url" \
    || die "could not download $url"
  got="$(sha256 "$file")"
  [ "$got" = "$want" ] || die "checksum mismatch for $(basename "$url"): expected $want, got $got. Nothing was installed."
  say "Checksum ok" >&2
  printf '%s\n' "$file"
}

launchable() { [ "$NO_LAUNCH" != 1 ]; }

# is_nexus_install <dir>: the folder holds a regular, executable nexus-desktop-gpui and the package's own install.sh.
# A directory that merely has an entry with that name does not count. The Linux swap deletes only such a folder.
is_nexus_install() { [ -f "$1/nexus-desktop-gpui" ] && [ -x "$1/nexus-desktop-gpui" ] && [ -f "$1/install.sh" ]; }

install_macos() {
  # Rosetta makes uname report x86_64 in a translated shell, so ask the CPU.
  [ "$(sysctl -n hw.optional.arm64 2> /dev/null || echo 0)" = 1 ] \
    || die "Nexus Desktop needs a Mac with Apple silicon. There is no Intel Mac build."
  need hdiutil "It ships with macOS."
  dmg="$(fetch macos-arm64-dmg)"
  dest="${NEXUS_APP_DIR:-/Applications}"
  mkdir -p "$dest" 2> /dev/null || true
  if [ ! -w "$dest" ]; then
    [ -z "${NEXUS_APP_DIR:-}" ] || die "$dest is not writable"
    dest="$HOME/Applications"
    mkdir -p "$dest"
  fi
  MOUNT="$TMP/mnt"
  mkdir -p "$MOUNT"
  hdiutil attach -quiet -nobrowse -readonly -mountpoint "$MOUNT" "$dmg" < /dev/null \
    || die "could not open the disk image"
  [ -d "$MOUNT/Nexus.app" ] || die "the disk image holds no Nexus.app"
  # Copied beside the final place, so the swap is a rename on one volume and a failed copy leaves the old app alone.
  stage="$dest/.Nexus.app.install.$$"
  rm -rf "$stage"
  ditto "$MOUNT/Nexus.app" "$stage" || die "could not copy Nexus.app to $dest"
  STAGE="$stage"
  hdiutil detach -quiet "$MOUNT" > /dev/null 2>&1 || true
  MOUNT=""
  # The checks Gatekeeper makes: a valid signature, then Apple's notarization (the ticket is stapled, so it works offline).
  codesign --verify --deep --strict "$stage" > /dev/null 2>&1 \
    || die "Nexus.app is not validly signed. Nothing was installed."
  assessment="$(LC_ALL=C spctl --assess --type execute --verbose=4 "$stage" 2>&1)" \
    || die "Gatekeeper did not accept Nexus.app as notarized. Nothing was installed."
  printf '%s\n' "$assessment" | grep -qx 'source=Notarized Developer ID' \
    || die "Gatekeeper did not confirm a notarized Developer ID for Nexus.app. Nothing was installed."
  say "Signature and notarization ok"
  if pgrep -f "$dest/Nexus.app/Contents/MacOS/" > /dev/null 2>&1; then
    say "Quitting the running Nexus"
    # By path, not by bundle id: Nexus Internal and Nexus Prod share the id, and must not be quit.
    osascript -e "tell application \"$dest/Nexus.app\" to quit" > /dev/null 2>&1 || true
    sleep 2
  fi
  old="$dest/.Nexus.app.old.$$"
  # Record the backup before moving the app, so EXIT can put it back after an interrupt (as the Linux path does).
  if [ -e "$dest/Nexus.app" ]; then
    MAC_APP="$dest/Nexus.app"
    MAC_OLD="$old"
    mv "$dest/Nexus.app" "$old"
  fi
  if mv "$stage" "$dest/Nexus.app"; then
    STAGE=""
    MAC_OLD=""
    rm -rf "$old"
  else
    MAC_OLD=""
    [ ! -e "$old" ] || mv "$old" "$dest/Nexus.app"
    die "could not move Nexus.app into $dest. The old app is back in place."
  fi
  say "Installed $dest/Nexus.app"
  if launchable; then open "$dest/Nexus.app" || warn "could not open Nexus.app"; fi
}

install_linux() {
  [ "$(uname -m)" = x86_64 ] || die "only x86_64 Linux builds exist so far (this machine is $(uname -m))"
  if command -v ldd > /dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
    die "Nexus Desktop needs glibc 2.35 or later. This system uses musl."
  fi
  need tar "Install it with your package manager."
  need xz "Install xz-utils (Debian, Ubuntu) or xz (Fedora)."
  data="${XDG_DATA_HOME:-$HOME/.local/share}"
  app="${NEXUS_INSTALL_DIR:-$data/nexus-desktop-gpui}"
  # The launcher link below points at $app from ~/.local/bin, so a relative path would dangle.
  case "$app" in
    /*) ;;
    *) app="$PWD/$app" ;;
  esac
  bin="$HOME/.local/bin"
  # The swap below deletes $app: only ever an earlier Nexus install, never a folder someone named by mistake ($HOME).
  if [ -e "$app" ] && ! is_nexus_install "$app"; then
    die "$app exists and is not a Nexus install. Choose another NEXUS_INSTALL_DIR. Nothing was installed."
  fi
  pkg="$(fetch linux-x86_64-tarball)"
  # Stage beside the old folder, so both moves are renames on one file system.
  parent="$(dirname "$app")"
  mkdir -p "$parent" "$bin"
  STAGE="$(mktemp -d "$parent/.nexus-desktop-install.XXXXXX")"
  mkdir "$STAGE/new"
  xz -dc "$pkg" | tar -C "$STAGE/new" -xf - || die "could not unpack $(basename "$pkg")"
  set -- "$STAGE/new"/*
  [ "$#" -eq 1 ] && [ -x "$1/nexus-desktop-gpui" ] && [ -f "$1/install.sh" ] \
    || die "$(basename "$pkg") does not hold one nexus-desktop-gpui folder. Nothing was installed."
  # Record the destination before moving the old folder so EXIT can restore it after an interrupt.
  LINUX_APP="$app"
  if [ -e "$app" ]; then
    mv -T "$app" "$STAGE/previous" || die "could not move the existing install aside. Nothing was installed."
    is_nexus_install "$STAGE/previous" \
      || die "the install directory changed during the update. Nothing was installed."
  fi
  mv -T "$1" "$app" || die "could not move the new version into place."
  LINUX_APP=""
  rm -rf "$STAGE"
  STAGE=""
  # The package's own script adds the menu entry and the nexus:// link handler.
  sh "$app/install.sh" > /dev/null < /dev/null || warn "could not add the menu entry"
  ln -sf "$app/nexus-desktop-gpui" "$bin/nexus-desktop-gpui"
  say "Installed $app"
  case ":$PATH:" in
    *":$bin:"*) ;;
    *) say "Add $bin to your PATH to start it as nexus-desktop-gpui" ;;
  esac
  if ! launchable; then
    say "Start Nexus from your app menu, or run nexus-desktop-gpui"
  elif [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    say "Opening Nexus"
    if command -v setsid > /dev/null 2>&1; then
      setsid "$app/nexus-desktop-gpui" > /dev/null 2>&1 < /dev/null &
    else
      nohup "$app/nexus-desktop-gpui" > /dev/null 2>&1 < /dev/null &
    fi
  else
    say "No display found. Start Nexus from your app menu, or run nexus-desktop-gpui"
  fi
}

cleanup() {
  if [ -n "${MOUNT:-}" ]; then hdiutil detach -quiet "$MOUNT" > /dev/null 2>&1 || true; fi
  if [ -n "$MAC_OLD" ] && [ -e "$MAC_OLD" ] && [ ! -e "$MAC_APP" ]; then
    trap '' INT TERM
    mv "$MAC_OLD" "$MAC_APP" || warn "could not restore the previous app. It is at $MAC_OLD"
  fi
  if [ -n "$LINUX_APP" ] && { [ -e "$STAGE/previous" ] || [ -L "$STAGE/previous" ]; }; then
    trap '' INT TERM
    if [ -e "$LINUX_APP" ] || [ -L "$LINUX_APP" ] || ! mv -T "$STAGE/previous" "$LINUX_APP"; then
      warn "could not restore the previous install. It is at $STAGE/previous"
      STAGE=""
    fi
  fi
  if [ -n "${STAGE:-}" ]; then rm -rf "$STAGE"; fi
  rm -rf "$TMP"
}

main() {
  NO_LAUNCH="${NEXUS_NO_LAUNCH:-0}"
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --no-launch) NO_LAUNCH=1 ;;
      -h | --help)
        say "Usage: curl -fsSL https://nexus.inc/install | sh [-s -- --no-launch]"
        exit 0
        ;;
      *) die "unknown option: $1 (the one option is --no-launch)" ;;
    esac
    shift
  done
  [ "$NO_LAUNCH" = 0 ] || NO_LAUNCH=1
  umask 022
  need curl "Install it with your package manager."
  MOUNT=""
  STAGE=""
  LINUX_APP=""
  MAC_APP=""
  MAC_OLD=""
  TMP="$(mktemp -d 2> /dev/null || mktemp -d -t nexus-install)"
  trap cleanup EXIT
  trap 'exit 130' INT TERM
  MANIFEST="$TMP/latest.json"
  curl -fsSL --retry 3 --connect-timeout 15 --max-time 60 -o "$MANIFEST" "$MANIFEST_URL" \
    || die "could not read $MANIFEST_URL. Check your network and try again."
  VERSION="$(tr -d '\r\n' < "$MANIFEST" | grep -o '"version": *"[^"]*"' | head -n 1 | sed 's/.*: *"\(.*\)"/\1/' || true)"
  [ -n "$VERSION" ] || die "$MANIFEST_URL has no version"
  say "Installing Nexus Desktop $VERSION"
  case "$(uname -s)" in
    Darwin) install_macos ;;
    Linux) install_linux ;;
    *) die "no Nexus Desktop build for $(uname -s). On Windows, run: irm https://nexus.inc/install.ps1 | iex" ;;
  esac
  say "Sign in with your browser on first launch."
}

main "$@"

#!/usr/bin/env bash
# Build Nexus (the GPUI app) and package it with CEF, for Linux (x86_64) or macOS (arm64). Run it from anywhere.
#
#   packaging/package.sh                        build with the `dist` profile (Cargo.toml), then package it
#   NO_BUILD=1 packaging/package.sh             package the binaries already in target/dist
#   NEXUS_PROFILE=debug NO_BUILD=1 packaging/package.sh  package another profile's binaries (to test this script)
#   NO_ARCHIVE=1 packaging/package.sh           stop at the folder (Linux) or the signed Nexus.app (macOS) and print
#                                               its path, with no .tar.xz or .dmg and no notarization
#                                               (a local dev build installs it itself)
#
# Linux: target/package/nexus-desktop-<version>-linux-x86_64.tar.xz. CEF sits beside the binary, which finds
# it through its `$ORIGIN` rpath (build.rs).
# macOS: target/package/nexus-desktop-<version>-macos-arm64.dmg, holding `Nexus.app` with CEF's framework
# and Chromium's helper apps (src/web/mac.rs). It is signed ad hoc, or with a Developer ID and notarized:
#   APPLE_SIGNING_IDENTITY="Developer ID Application: … (TEAMID)"  sign with it, hardened runtime, packaging/macos/*.entitlements
#   APPLE_API_KEY_PATH, APPLE_API_KEY, APPLE_API_ISSUER            also notarize and staple the app and the .dmg (needs jq)
# The release workflow sets both, from Nexus's signing secrets.
#
# What keeps the package small: the `dist` profile (thin LTO, stripped), only CEF's English locale (src/web/host.rs
# sets it), xz or LZMA compression instead of gzip or zip, and on macOS one small helper binary (src/helper.rs) in
# the four helper apps instead of four copies of the app.
# Windows: packaging/package.ps1.
set -euo pipefail

crate="$(cd "$(dirname "$0")/.." && pwd)"
cd "$crate"
bin=nexus-desktop-gpui
# The release sets NEXUS_DESKTOP_BUILD_VERSION (an internal build's `<version>-internal.<n>`); else Cargo.toml's.
version="${NEXUS_DESKTOP_BUILD_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
out="$crate/target/package"
cef_root="${CEF_PATH:-$crate/target/cef}"

profile="${NEXUS_PROFILE:-dist}"
if [[ -z "${NO_BUILD:-}" ]]; then
  # --timings writes target/cargo-timings/cargo-timing.html: each crate's compile time and the build's concurrency.
  # The release keeps it as an artifact. It changes nothing in the binaries.
  cargo build --profile "$profile" --locked --bins --timings
fi
built="target/$profile"
[[ -x "$built/$bin" ]] || { echo "no $built/$bin; build first" >&2; exit 1; }
rm -rf "$out"
mkdir -p "$out"

# CEF's files a running app needs. The rest of the distribution is for building (headers, the wrapper's sources,
# cmake files), or `chrome-sandbox`, the setuid helper. A package cannot install it setuid root, and an unprivileged
# copy makes Chromium abort, so Linux uses the namespace sandbox instead, when asked for (`sandbox_on` in
# src/web/host.rs).
copy_cef_runtime() {
  local from="$1" to="$2"
  local entry name
  for entry in "$from"/*; do
    name="$(basename "$entry")"
    case "$name" in
      include | libcef_dll | cmake | CMakeLists.txt | archive.json | chrome-sandbox | *.lib | bootstrap*.exe) ;;
      locales)
        # Only English: see `locale` in src/web/host.rs. Chromium also reads the gendered variants
        # (`en-US_FEMININE.pak` and the others): without them every page's renderer stops.
        mkdir -p "$to/locales"
        cp "$entry"/en-US.pak "$entry"/en-US_*.pak "$to/locales/"
        ;;
      *) cp -R "$entry" "$to/" ;;
    esac
  done
  copy_cef_notices "$from" "$to" CREDITS.html
}

# CEF's license (BSD-3-Clause) and Chromium's credits: every package carries them beside CEF's binaries. CEF's
# archive keeps its license at its root, and download-cef does not keep that file, so packaging/ holds the text.
# $1 the folder with CREDITS.html (download-cef puts it beside libcef or the framework), $2 the destination,
# $3 the name the credits get there.
copy_cef_notices() {
  local from="$1" to="$2" credits="$3"
  [[ -f "$from/CREDITS.html" ]] || { echo "no CREDITS.html in $from; a package must carry Chromium's credits" >&2; exit 1; }
  cp "$from/CREDITS.html" "$to/$credits"
  cp "$crate/packaging/CEF-LICENSE.txt" "$to/CEF-LICENSE.txt"
  copy_nexus_notices "$to"
}

# Nexus's own license (MIT), its NOTICE and the third-party notices of what the binary embeds: every package carries
# them beside CEF's. LICENSE and NOTICE are in oss/community here, and at the root of the public repository, two
# folders above the app. $1 the destination.
copy_nexus_notices() {
  local to="$1" file src
  [[ -f "$crate/THIRD_PARTY_NOTICES.txt" ]] || { echo "no THIRD_PARTY_NOTICES.txt in $crate; a package must carry it" >&2; exit 1; }
  cp "$crate/THIRD_PARTY_NOTICES.txt" "$to/THIRD_PARTY_NOTICES.txt"
  for file in LICENSE NOTICE; do
    src="$crate/oss/community/$file"
    [[ -f "$src" ]] || src="$crate/../../$file"
    [[ -f "$src" ]] || { echo "no $file in $crate/oss/community or $crate/../..; a package must carry it" >&2; exit 1; }
    cp "$src" "$to/$file"
  done
}

# Nexus's app icon `assets/icons/$1`, or the neutral `assets/held/$2` where the marks are not in the tree (the
# open-source repo: export manifest, B2).
app_icon() {
  if [[ -f "assets/icons/$1" ]]; then echo "assets/icons/$1"; else echo "assets/held/$2"; fi
}

package_linux() {
  local libcef dir name stage
  libcef="$(find "$cef_root" -maxdepth 4 -name libcef.so -print -quit)"
  [[ -n "$libcef" ]] || { echo "no libcef.so under $cef_root" >&2; exit 1; }
  dir="$(dirname "$libcef")"
  name="nexus-desktop-$version-linux-x86_64"
  stage="$out/$name"
  mkdir -p "$stage"
  cp "$built/$bin" "$stage/"
  strip "$stage/$bin"
  copy_cef_runtime "$dir" "$stage"
  # Linux's libcef.so ships with its debug info and symbol table: 1.4 GB, against 280 MB without.
  strip --strip-unneeded "$stage"/*.so "$stage"/*.so.*
  # The binary must find libcef.so beside itself, wherever the folder is unpacked.
  if ! readelf -d "$stage/$bin" | grep -q '\$ORIGIN'; then
    echo "$bin has no \$ORIGIN rpath; rebuild after build.rs adds it" >&2
    exit 1
  fi
  cp "$(app_icon 128x128@2x.png app-icon.png)" "$stage/$bin.png"
  cp packaging/linux/nexus-desktop-gpui.desktop "$stage/"
  cp packaging/linux/install.sh "$stage/"
  chmod +x "$stage/install.sh"
  if [[ -n "${NO_ARCHIVE:-}" ]]; then
    echo "$stage"
    return
  fi
  # xz makes CEF about a third smaller than gzip does (libcef.so: 74 MB against 107).
  tar -C "$out" -cf - "$name" | xz -9 -T0 > "$out/$name.tar.xz"
  echo "$out/$name.tar.xz"
}

# Write an app's Info.plist. $1 the plist, $2 the executable, $3 the bundle id, $4 the name, $5 1 for a helper.
write_plist() {
  local plist="$1" exe="$2" id="$3" name="$4" helper="$5"
  {
    cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>$exe</string>
  <key>CFBundleIdentifier</key><string>$id</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>$name</string>
  <key>CFBundleDisplayName</key><string>$name</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>LSEnvironment</key><dict><key>MallocNanoZone</key><string>0</string></dict>
PLIST
    if [[ "$helper" == 1 ]]; then
      echo "  <key>LSUIElement</key><string>1</string>"
    else
      echo "  <key>CFBundleIconFile</key><string>icon.icns</string>"
      # Dictation, and the screen recordings for agents (src/record.rs): the reasons macOS shows in its permission prompts.
      echo "  <key>NSMicrophoneUsageDescription</key><string>Nexus uses the microphone for dictation and for voice mode in the Assistant, and records your voice with the screen when you record something for an agent (⌥⇧S). The recording stays on this Mac.</string>"
      echo "  <key>NSSpeechRecognitionUsageDescription</key><string>Nexus transcribes what you say in a screen recording on this Mac, so an agent can read it. Your voice is not sent anywhere.</string>"
      echo "  <key>NSAppleEventsUsageDescription</key><string>Nexus notes the address of the browser tab you record, so an agent knows which page it is.</string>"
    fi
    echo "</dict>"
    echo "</plist>"
  } > "$plist"
}

package_macos() {
  local framework app contents helper suffix helper_name helper_app id name
  framework="$(find "$cef_root" -maxdepth 4 -type d -name 'Chromium Embedded Framework.framework' -print -quit)"
  [[ -n "$framework" ]] || { echo "no Chromium Embedded Framework.framework under $cef_root" >&2; exit 1; }
  # The bundle's name is the product's; its id stays xyz.nexus.desktop-gpui, so installs keep their data and updates.
  app="$out/Nexus.app"
  contents="$app/Contents"
  mkdir -p "$contents/MacOS" "$contents/Frameworks" "$contents/Resources"
  cp "$built/$bin" "$contents/MacOS/"
  strip -x "$contents/MacOS/$bin"
  cp -R "$framework" "$contents/Frameworks/"
  # Only English: see `locale` in src/web/host.rs. The framework keeps each locale in `Resources/<locale>.lproj`.
  local resources="$contents/Frameworks/Chromium Embedded Framework.framework/Resources"
  [[ -d "$resources/en.lproj" ]] || { echo "no en.lproj in CEF's framework" >&2; exit 1; }
  find "$resources" -maxdepth 1 -name '*.lproj' ! -name en.lproj -exec rm -rf {} +
  cp "$(app_icon icon.icns app-icon.icns)" "$contents/Resources/icon.icns"
  copy_cef_notices "$(dirname "$framework")" "$contents/Resources" CHROMIUM-CREDITS.html
  write_plist "$contents/Info.plist" "$bin" "xyz.nexus.desktop-gpui" "Nexus" 0
  # Chromium starts its helpers from "<executable> Helper.app" and its variants, beside the framework. Each holds the
  # small helper binary (src/helper.rs), under the name Chromium expects. No " (Alerts)" helper: it only shows
  # Chromium's notifications, which the app turns off (src/web/handlers.rs), and macOS would list it in Notifications.
  for suffix in "" " (GPU)" " (Renderer)" " (Plugin)"; do
    helper_name="$bin Helper$suffix"
    helper_app="$contents/Frameworks/$helper_name.app"
    id="xyz.nexus.desktop-gpui.helper$(echo "$suffix" | tr -d ' ()' | tr '[:upper:]' '[:lower:]' | sed 's/^./.&/')"
    mkdir -p "$helper_app/Contents/MacOS"
    cp "$built/$bin-helper" "$helper_app/Contents/MacOS/$helper_name"
    strip -x "$helper_app/Contents/MacOS/$helper_name"
    write_plist "$helper_app/Contents/Info.plist" "$helper_name" "$id" "$helper_name" 1
  done
  # Signatures from the inside out: Apple silicon runs no unsigned code, and a bundle's signature covers what it holds.
  # First CEF's own libraries, then its framework, then the helper apps, then the app.
  local fw="$contents/Frameworks/Chromium Embedded Framework.framework" file
  while IFS= read -r -d '' file; do
    sign "$file"
  done < <(find "$fw" -type f \( -name '*.dylib' -o -name '*.so' \) -print0)
  sign "$fw"
  for helper in "$contents/Frameworks/"*Helper*.app; do
    sign "$helper" packaging/macos/helper.entitlements
  done
  sign "$app" packaging/macos/app.entitlements
  check_signature "$app"
  if [[ -n "${NO_ARCHIVE:-}" ]]; then
    echo "$app"
    return
  fi
  # Notarized before the disk image is made, so the app inside it carries its own ticket and opens offline.
  if notarizing; then
    ditto -c -k --keepParent "$app" "$out/app.zip"
    notarize "$out/app.zip" "$app"
    rm "$out/app.zip"
  fi
  # A disk image with LZMA compression (ULMO), smaller than a zip, with the usual Applications link to drag onto.
  name="nexus-desktop-$version-macos-arm64"
  local dmg_root="$out/dmg" dmg="$out/$name.dmg"
  mkdir -p "$dmg_root"
  mv "$app" "$dmg_root/"
  ln -s /Applications "$dmg_root/Applications"
  packaging/macos/create-dmg.sh "$dmg_root" "$dmg"
  # Gatekeeper checks a downloaded disk image before the app inside it, so the .dmg is signed and notarized too.
  if [[ "$identity" != - ]]; then
    codesign --force --sign "$identity" --timestamp "$dmg"
    codesign --verify --strict "$dmg"
  fi
  if notarizing; then
    notarize "$dmg" "$dmg"
    gatekeeper "$dmg_root/Nexus.app" "$dmg"
  fi
  echo "$dmg"
}

# macOS signing (the header): a Developer ID with the hardened runtime and a secure timestamp, or ad hoc ("-").
identity="${APPLE_SIGNING_IDENTITY:--}"

# Sign $1, with the entitlements file $2 if given.
sign() {
  local args=(--force --sign "$identity")
  [[ "$identity" == - ]] || args+=(--options runtime --timestamp)
  [[ -z "${2:-}" ]] || args+=(--entitlements "$2")
  codesign "${args[@]}" "$1"
}

# What a person's Mac checks: a valid signature on every nested part, and with a Developer ID, its authority and the
# hardened runtime that notarization needs.
check_signature() {
  local details
  codesign --verify --deep --strict --verbose=2 "$1"
  [[ "$identity" != - ]] || return 0
  details="$(codesign --display --verbose=2 "$1" 2>&1)"
  grep -q '^Authority=Developer ID Application: ' <<< "$details" \
    || { echo "$1 is not signed with a Developer ID Application identity" >&2; echo "$details" >&2; exit 1; }
  grep -q '^CodeDirectory .*flags=.*runtime' <<< "$details" \
    || { echo "$1 is signed without the hardened runtime" >&2; echo "$details" >&2; exit 1; }
}

notarizing() {
  [[ "$identity" != - && -n "${APPLE_API_KEY_PATH:-}" && -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_ISSUER:-}" ]]
}

# Send $1 to Apple's notary service and staple the ticket to $2. `--wait` can exit 0 when Apple answers Invalid, so
# the status is checked, and Apple's log says why.
notarize() {
  local out auth=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
  out="$(xcrun notarytool submit "$1" "${auth[@]}" --wait --output-format json)"
  echo "$out" >&2
  if [[ "$(jq -r .status <<< "$out")" != Accepted ]]; then
    xcrun notarytool log "$(jq -r .id <<< "$out")" "${auth[@]}" >&2 || true
    echo "Apple did not accept $(basename "$1")" >&2
    exit 1
  fi
  xcrun stapler staple "$2" >&2
  xcrun stapler validate "$2" >&2
}

# Gatekeeper's own verdict on the app and the disk image, when the runner has assessments on.
gatekeeper() {
  local out
  if ! spctl --status 2>&1 | grep -q 'assessments enabled'; then
    echo "spctl assessments are off on this machine; the stapled tickets are the check" >&2
    return 0
  fi
  out="$(spctl --assess --verbose=4 --type execute "$1" 2>&1 || true)"
  if ! grep -q 'source=Notarized Developer ID' <<< "$out"; then
    echo "Gatekeeper refuses $1: $out" >&2
    exit 1
  fi
  out="$(spctl --assess --verbose=4 --type open --context context:primary-signature "$2" 2>&1 || true)"
  if ! grep -q 'source=Notarized Developer ID' <<< "$out"; then
    echo "Gatekeeper refuses $2: $out" >&2
    exit 1
  fi
}

case "$(uname -s)" in
  Linux) package_linux ;;
  Darwin) package_macos ;;
  *) echo "use packaging/package.ps1 on Windows" >&2; exit 1 ;;
esac

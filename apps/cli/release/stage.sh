#!/usr/bin/env bash
# Stage a CLI release for the downloads bucket, as .github/workflows/cli-release.yml publishes it.
#
#   stage.sh <binaries> <out> <version> <channel> <commit>
#
# <binaries> holds one `nexus-<os>-<arch>` file per platform (`nexus-win32-x64.exe` on Windows). The script writes a
# `.sha256` beside each binary, and `SHA256SUMS` and the version's `latest.json` in <out>. Each Gateway reads that
# `latest.json` at `cli/<channel>/<its version>/` (NEXUS_CLI_DOWNLOADS), so the names and the layout must not change.
# Every target in REQUIRED_TARGETS must be there, or the script fails and writes no manifest.
set -euo pipefail

bins="${1:?usage: stage.sh <binaries> <out> <version> <channel> <commit>}"
out="${2:?usage: stage.sh <binaries> <out> <version> <channel> <commit>}"
version="${3:?usage: stage.sh <binaries> <out> <version> <channel> <commit>}"
channel="${4:?usage: stage.sh <binaries> <out> <version> <channel> <commit>}"
commit="${5:?usage: stage.sh <binaries> <out> <version> <channel> <commit>}"
here="$(cd "$(dirname "$0")" && pwd)"
downloads="${DOWNLOADS_URL:-https://downloads.nexus.inc}"
required="${REQUIRED_TARGETS:-darwin-arm64,darwin-x64,linux-x64,linux-arm64}"

mkdir -p "$out"
cd "$bins"
binaries=()
for bin in nexus-*; do
  case "$bin" in *.sha256 | *.sig) ;; *) binaries+=("$bin") ;; esac
done
[ "${#binaries[@]}" -gt 0 ] || { echo "stage.sh: no nexus-<target> binary in $bins" >&2; exit 1; }
for bin in "${binaries[@]}"; do
  sha256sum "$bin" > "$bin.sha256"
done
printf '%s\n' "${binaries[@]}" | sort | xargs sha256sum > "$out/SHA256SUMS"
node "$here/cli-manifest.mjs" --dir . --version "$version" --build "$version+${commit::7}" --channel "$channel" \
  --commit "$commit" --base "$downloads/cli/$channel/$version" --require "$required" > "$out/latest.json"
cat "$out/SHA256SUMS" "$out/latest.json"

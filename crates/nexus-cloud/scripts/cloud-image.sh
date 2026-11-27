#!/bin/sh
# Builds the saved machine image ("golden image") that new Nexus cloud machines start from: a builder
# machine on boat, the install (cloud-image-install.sh) over boat's /commands, a check that no secret got in, then
# the saved snapshots `nexus-cloud-<version>-<hash>` and `nexus-cloud` (the alias the Gateway creates machines from). It
# keeps the three newest versioned snapshots and always deletes the builder, also when a step fails.
#
# It costs money (a builder machine and saved snapshots), so it builds only when an input changed: <hash> is the first 8
# characters of a SHA-256 over the host's release version (the app's, without its pre-release: the host updates itself
# on connect, so a new internal build of the same release does not need a new image), the pinned Claude Code, Codex, Playwright MCP and
# uv versions, and cloud-image-install.sh itself. When the newest versioned snapshot already has this hash, and the
# alias came from the same builder, it prints "unchanged" and calls boat for nothing but the two reads. FORCE=1 builds
# anyway.
#
#   BOAT_API_KEY=boat_... NEXUS_HOST_URL=https://.../nexus-host_<v>_linux-x86_64.gz VERSION=<v> scripts/cloud-image.sh
#
# Environment:
#   BOAT_API_KEY      required. A boat key that may create machines, run commands, save snapshots and delete its own.
#   BOAT_API_BASE     default https://boat.dev/api/v1 (https, or http on localhost for a test)
#   BOAT_ORG          optional organization (sent as X-Boat-Org)
#   NEXUS_HOST_URL    where the builder downloads nexus-host (x86_64, the gzipped release file or a plain binary), or
#   NEXUS_HOST_PATH   a file on this computer (uploaded to the builder through boat)
#   NEXUS_HOST_SHA256 checksum of that file: required with NEXUS_HOST_URL, optional with NEXUS_HOST_PATH
#   VERSION           the desktop version the host is built from; names the snapshot (default: the app's version in
#                     desktop-gpui/Cargo.toml)
#   IMAGE_HOST_VERSION  the host's part of the hash (default: VERSION's release version, without its pre-release)
#   FORCE             1: build even when nothing changed
#   IMAGE_NAME        the alias and the versioned snapshots' prefix (default nexus-cloud). Give a test image its own name,
#                     one that does not start with "nexus-cloud-": this script deletes old snapshots by that prefix.
#   CLAUDE_CODE_VERSION, CODEX_VERSION, OPENCODE_VERSION, UV_VERSION   the pinned tools (defaults below; raise them here in a PR). The
#                     desktop app pins none of them; the defaults were the latest on npm and PyPI on 2026-11-04.
#   KEEP              versioned snapshots to keep (default 3)
#   CLOUD_IMAGE_TIMEOUT_S   how long to wait for each boat state (default 600); CLOUD_IMAGE_POLL_S the pause between asks (3)
#   CLOUD_IMAGE_DEADLINE_S  the whole build's limit (default 1200); past it the build stops and the builder is deleted
# Needs curl and jq. Nothing here prints the API key.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"
base="${BOAT_API_BASE:-https://boat.dev/api/v1}"
base="${base%/}"
case "$base" in
  https://*|http://127.0.0.1:*|http://localhost:*) ;;
  *) echo "cloud-image: BOAT_API_BASE must be https (the API key goes in every request)" >&2; exit 1 ;;
esac
: "${BOAT_API_KEY:?BOAT_API_KEY is required}"
# The app's `[package]` version (desktop-gpui/Cargo.toml), three folders up from this crate's scripts/.
app_version="$(sed -n '/^\[package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$here/../../../Cargo.toml" | head -1)"
version="${VERSION:-$app_version}"
claude_version="${CLAUDE_CODE_VERSION:-2.1.318}"
codex_version="${CODEX_VERSION:-0.166.0}"
opencode_version="${OPENCODE_VERSION:-2.3.0}"
uv_version="${UV_VERSION:-0.13.7}"
keep="${KEEP:-3}"
wait_s="${CLOUD_IMAGE_TIMEOUT_S:-600}"
deadline_s="${CLOUD_IMAGE_DEADLINE_S:-1200}"
started_at="$(date +%s)"
poll_s="${CLOUD_IMAGE_POLL_S:-3}"
# link.rs's PLAYWRIGHT_MCP is the one source: read it, so the image and the app never name two versions.
playwright="$(sed -n 's/^pub const PLAYWRIGHT_MCP: &str = "\(.*\)";/\1/p' "$here/../src/link.rs")"
[ -n "$playwright" ] || { echo "cloud-image: could not read PLAYWRIGHT_MCP from link.rs" >&2; exit 1; }
if [ -z "${NEXUS_HOST_URL:-}" ] && [ ! -f "${NEXUS_HOST_PATH:-/nonexistent}" ]; then
  echo "cloud-image: set NEXUS_HOST_URL or NEXUS_HOST_PATH (an existing file)" >&2
  exit 1
fi
if [ -n "${NEXUS_HOST_URL:-}" ] && [ -z "${NEXUS_HOST_SHA256:-}" ]; then
  echo "cloud-image: NEXUS_HOST_SHA256 is required with NEXUS_HOST_URL (the builder runs the file it downloads)" >&2
  exit 1
fi
command -v jq >/dev/null && command -v curl >/dev/null || { echo "cloud-image: curl and jq are required" >&2; exit 1; }

# boat's saved-snapshot names: lowercase letters, digits and dashes.
slug="$(printf '%s' "$version" | tr 'A-Z' 'a-z' | sed 's/[^a-z0-9]/-/g')"
host_release="${IMAGE_HOST_VERSION:-${version%%-*}}"
hash="$(printf 'host=%s\nclaude=%s\ncodex=%s\nopencode=%s\nplaywright=%s\nuv=%s\ninstall=%s\n' "$host_release" "$claude_version" \
  "$codex_version" "$opencode_version" "$playwright" "$uv_version" "$(sha256sum < "$here/cloud-image-install.sh" | cut -d ' ' -f 1)" \
  | sha256sum | cut -c1-8)"
image_name="${IMAGE_NAME:-nexus-cloud}"
name="$image_name-$slug-$hash"
alias_name="$image_name"

say() { echo "cloud-image: $*" >&2; }
fail() { say "$*"; exit 1; }
# sq VALUE: VALUE as one single-quoted shell word, for the command the builder runs.
sq() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }

# The key goes to curl in a file, not on its command line, where `ps` would show it. printf is a shell builtin.
auth="$(mktemp)"
printf 'Authorization: Bearer %s\n' "$BOAT_API_KEY" > "$auth"

# api METHOD PATH [JSON]: prints the answer's body; fails (and says boat's message, never the key) on a non-2xx.
api() {
  method="$1"; path="$2"; body="${3:-}"
  out="$(mktemp)"
  set -- -sS -m 120 -o "$out" -w '%{http_code}' -X "$method" -H "@$auth" \
    -H 'Accept: application/json'
  [ -z "${BOAT_ORG:-}" ] || set -- "$@" -H "X-Boat-Org: $BOAT_ORG"
  [ -z "${confirm:-}" ] || set -- "$@" -H "X-Ascii-Confirm-Delete: $confirm"
  [ -z "${idem:-}" ] || set -- "$@" -H "Idempotency-Key: $idem"
  [ -z "$body" ] || set -- "$@" -H 'Content-Type: application/json' -d "$body"
  code="$(curl "$@" "$base$path")" || { rm -f "$out"; say "could not reach boat for $method $path"; return 1; }
  if [ "$code" -lt 200 ] || [ "$code" -ge 300 ]; then
    say "$method $path answered $code: $(jq -r '.error.message // .message // "no message"' < "$out" 2>/dev/null || echo unreadable)"
    rm -f "$out"
    return 1
  fi
  cat "$out"
  rm -f "$out"
}

builder=""
cleanup() {
  status=$?
  if [ -n "$builder" ]; then
    confirm="$builder" api DELETE "/sandboxes/$builder" >/dev/null || say "could not delete the builder $builder: delete it by hand"
    say "builder $builder deleted"
  fi
  rm -f "$auth"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

past_deadline() { [ $(( $(date +%s) - started_at )) -lt "$deadline_s" ] || fail "past the ${deadline_s}s limit for the whole build"; }

# wait_for DESCRIPTION COMMAND...: runs the command every $poll_s seconds until it succeeds, or fails after $wait_s.
wait_for() {
  what="$1"; shift
  end=$(( $(date +%s) + wait_s ))
  until "$@"; do
    [ "$(date +%s)" -lt "$end" ] || fail "timed out waiting for $what"
    past_deadline
    sleep "$poll_s"
  done
}
sandbox_is() { [ "$(api GET "/sandboxes/$builder" | jq -r '.sandbox.state')" = "$1" ]; }
sandbox_up() { case "$(api GET "/sandboxes/$builder" | jq -r '.sandbox.state')" in ready|idle|running) return 0 ;; failed|error) fail "the builder failed" ;; *) return 1 ;; esac; }
snapshot_ready() {
  case "$(api GET "/saved-snapshots/$1" | jq -r '.status // .snapshot.status')" in
    ready) return 0 ;;
    failed) fail "saving $1 failed" ;;
    *) return 1 ;;
  esac
}

# run_on_builder COMMAND [SECONDS]: the command on the builder; prints its output, fails on a non-zero exit.
run_on_builder() {
  answer="$(api POST "/sandboxes/$builder/commands" "$(jq -n --arg c "$1" --argjson t "${2:-60}" '{command: $c, timeoutSeconds: $t}')")" || return 1
  printf '%s\n' "$answer" | jq -r '.stdout, .stderr' >&2
  [ "$(printf '%s' "$answer" | jq -r '.exitCode')" = 0 ] && [ "$(printf '%s' "$answer" | jq -r '.timedOut')" != true ]
}

# Unchanged? The newest versioned snapshot carries this hash, and the alias was saved from the same builder.
if [ "${FORCE:-}" != 1 ]; then
  newest="$(api GET /saved-snapshots | jq -r --arg prefix "$image_name-" '(.snapshots // .savedSnapshots // [])
    | map(select(.name | startswith($prefix))) | sort_by(.createdAt) | last | .name // empty')" || exit 1
  if [ -n "$newest" ] && [ "${newest##*-}" = "$hash" ]; then
    source_of() { api GET "/saved-snapshots/$1" | jq -r '.snapshot.sourceSandboxId // .sourceSandboxId // empty'; }
    from_newest="$(source_of "$newest")" || exit 1
    from_alias="$(source_of "$alias_name" 2>/dev/null || true)"
    if [ -n "$from_newest" ] && [ "$from_newest" = "$from_alias" ]; then
      say "unchanged: $alias_name is already $newest (inputs hash $hash); nothing built"
      exit 0
    fi
  fi
fi

say "creating the builder (no environment, small)"
idem="cloud-image-$$-$(date +%s)"
created="$(api POST /sandboxes "$(jq -n --arg n "nexus-cloud-image-builder-$slug" \
  '{name: $n, type: "small", noEnv: true, ttlSeconds: 1800, labels: {purpose: "nexus-cloud-image"}}')")" || exit 1
idem=""
builder="$(printf '%s' "$created" | jq -r '.sandbox.id')"
[ -n "$builder" ] && [ "$builder" != null ] || fail "boat did not answer a machine id"
wait_for "the builder to be ready" sandbox_up

# nexus-host: a download on the builder, or a file uploaded through boat's upload URL.
host_env="HOST_DIR=$(sq "$slug")"
if [ -n "${NEXUS_HOST_URL:-}" ]; then
  host_env="$host_env HOST_URL=$(sq "$NEXUS_HOST_URL")"
else
  target=".nexus-image/nexus-host.upload"
  url="$(api POST "/sandboxes/$builder/files/upload-url" "$(jq -n --arg p "$target" '{path: $p, ttlSeconds: 900}')" | jq -r '.url')"
  # The pre-signed URL goes in on stdin too: it grants the upload until it expires.
  printf 'url = "%s"\n' "$url" | curl -fsS -m 600 -X PUT --data-binary "@$NEXUS_HOST_PATH" -K - >/dev/null \
    || fail "could not upload nexus-host"
  host_env="$host_env HOST_FILE=\"\$HOME/$target\""
fi
[ -z "${NEXUS_HOST_SHA256:-}" ] || host_env="$host_env HOST_SHA256=$(sq "$NEXUS_HOST_SHA256")"

say "installing over /commands (version $version, claude $claude_version, codex $codex_version, opencode $opencode_version, $playwright, uv $uv_version)"
script="$(base64 < "$here/cloud-image-install.sh" | tr -d '\n')"
# One command per stage: boat ends a command after 10 minutes.
for stage in host agents browser media finish; do
  past_deadline
  say "stage $stage"
  run_on_builder "printf %s '$script' | base64 -d | STAGE=$stage $host_env CLAUDE_CODE_VERSION=$(sq "$claude_version") CODEX_VERSION=$(sq "$codex_version") OPENCODE_VERSION=$(sq "$opencode_version") PLAYWRIGHT_MCP=$(sq "$playwright") UV_VERSION=$(sq "$uv_version") sh -s" 600 \
    || fail "the install failed in stage $stage"
done

say "checking that the image holds no secret and every tool"
# Each of these appears only when something signed in or a host ran here; any one in the image would be copied into
# every person's machine.
run_on_builder '
bad=""
for f in .nexus/host/agent.env .nexus/host/mcp.json .nexus/host/token .nexus/cloud/stop-key.header \
  .config/gh/hosts.yml .config/ascii/boat/config.json .claude/.credentials.json .codex/auth.json \
  .local/share/opencode/auth.json .git-credentials .netrc; do
  if [ -e "$HOME/$f" ]; then bad="$bad $f"; fi
done
if [ -n "$(grep -rls "sk-ant-" "$HOME/.claude" "$HOME/.claude.json" "$HOME/.codex" "$HOME/.nexus" 2>/dev/null)" ]; then bad="$bad sk-ant-token"; fi
if [ -n "$bad" ]; then echo "secret or state found:$bad" >&2; exit 1; fi
# And the tools the image is for are there.
missing=""
for t in "$HOME/.local/bin/claude" "$HOME/.local/bin/codex" "$HOME/.local/bin/opencode" "$HOME/.local/bin/uv" \
  "$HOME/.nexus/host/*/nexus-host" "$HOME/.boat/pre-stop" "$HOME/.boatignore"; do
  ls $t >/dev/null 2>&1 || missing="$missing $t"
done
command -v ffmpeg >/dev/null 2>&1 || missing="$missing ffmpeg"
ls -d "$HOME"/.nexus/browsers/chromium-* >/dev/null 2>&1 || missing="$missing chromium"
if [ -n "$missing" ]; then echo "missing:$missing" >&2; exit 1; fi
echo "clean"
' || fail "the image holds a secret or host state, or lacks a tool: it was not saved"

say "stopping the builder so its disk is saved whole"
api POST "/sandboxes/$builder/stop" '{}' >/dev/null || exit 1
wait_for "the builder to be archived" sandbox_is archived

for saved in "$name" "$alias_name"; do
  say "saving $saved"
  api POST /saved-snapshots "$(jq -n --arg s "$builder" --arg n "$saved" '{sandboxId: $s, name: $n}')" >/dev/null || exit 1
  wait_for "$saved to be ready" snapshot_ready "$saved"
done

# Keep the newest $keep versioned snapshots (the alias never matches $image_name-<slug>'s prefix test below).
old="$(api GET /saved-snapshots | jq -r --arg prefix "$image_name-" --argjson keep "$keep" '
  (.snapshots // .savedSnapshots // [])
  | map(select(.name | startswith($prefix)))
  | sort_by(.createdAt) | reverse | .[$keep:] | .[].name')"
for stale in $old; do
  say "deleting the old snapshot $stale"
  api DELETE "/saved-snapshots/$stale" >/dev/null || say "could not delete $stale"
done

say "done: $name and $alias_name"

#!/bin/sh
# Runs ON the builder machine, sent by cloud-image.sh over boat's /commands. It puts into the home folder what a new
# cloud machine needs before the app first connects, so the app's own provisioning (nexus-cloud's link.rs, PREPARE) has
# little left to do. It writes no secret: the Claude token, the stop key and the person's git name arrive at connect.
#
# Input, all as environment variables:
#   HOST_DIR           a staging folder under ~/.nexus/host (cloud-image.sh passes the snapshot's slug). The host
#                      then moves to the folder of the version it reports (`nexus-host version`, with its +sha):
#                      the folder the app's installer looks for (nexus-remote install.rs).
#   HOST_URL           where to download nexus-host (a .gz as the release makes it, or the plain file), or
#   HOST_FILE          a file already on this machine (same two forms)
#   HOST_SHA256        the checksum of that file as downloaded: required with HOST_URL, optional with HOST_FILE
#   CLAUDE_CODE_VERSION, CODEX_VERSION, OPENCODE_VERSION, PLAYWRIGHT_MCP (the package with its version, as link.rs names it), UV_VERSION
#   STAGE              one of: host agents browser media finish. cloud-image.sh runs them in this order, one command
#                      each, because boat ends a command after 10 minutes at most.
set -eu
umask 077
: "${STAGE:?}" "${HOST_DIR:?}" "${CLAUDE_CODE_VERSION:?}" "${CODEX_VERSION:?}" "${OPENCODE_VERSION:?}" "${PLAYWRIGHT_MCP:?}" "${UV_VERSION:?}"
# HOST_DIR is one folder name under ~/.nexus/host: the host stage moves and deletes folders by it.
case "$HOST_DIR" in
  */*|.*) echo "image: HOST_DIR must be one folder name, not $HOST_DIR" >&2; exit 1 ;;
esac
# A downloaded binary runs in the host stage and goes into every cloud machine: never without its checksum.
if [ -n "${HOST_URL:-}" ] && [ -z "${HOST_SHA256:-}" ]; then
  echo "image: HOST_SHA256 is required with HOST_URL" >&2
  exit 1
fi
export PATH="$HOME/.local/bin:$PATH"
root="$HOME/.nexus/host"
# The browsers live here, not in ~/.cache: boat's snapshot does not keep ~/.cache.
browsers="$HOME/.nexus/browsers"
log() { echo "image: $*"; }

stage_host() {
# 1. nexus-host, in the folder the app's installer looks in (nexus-remote install.rs), so a connect finds it present.
mkdir -p "$root/$HOST_DIR" "$HOME/.local/bin"
chmod 700 "$root" "$root/$HOST_DIR"
part="$root/$HOST_DIR/nexus-host.part"
if [ -n "${HOST_URL:-}" ]; then
  curl -fsSL --retry 3 --connect-timeout 15 -o "$part.dl" "$HOST_URL"
elif [ -n "${HOST_FILE:-}" ]; then
  cp "$HOST_FILE" "$part.dl"
else
  echo "image: HOST_URL or HOST_FILE is required" >&2
  exit 1
fi
if [ -n "${HOST_SHA256:-}" ]; then
  got="$(sha256sum "$part.dl" | cut -d ' ' -f 1)"
  if [ "$got" != "$HOST_SHA256" ]; then echo "image: nexus-host checksum is $got, not $HOST_SHA256" >&2; exit 1; fi
fi
if gzip -t "$part.dl" 2>/dev/null; then gzip -dc "$part.dl" > "$part"; else cp "$part.dl" "$part"; fi
rm -f "$part.dl"
chmod 700 "$part"
mv -f "$part" "$root/$HOST_DIR/nexus-host"
reported="$("$root/$HOST_DIR/nexus-host" version)"
log "nexus-host: $reported"
# The folder is the version the host reports (`0.10.0-internal.388+abc1234`), not the slug: an app of that version
# finds it, and an app whose own version is not published finds it as the host already on the machine.
version="${reported#nexus-host }"
case "$version" in
  ""|*/*|.*|*" "*) echo "image: nexus-host reported no usable version: $reported" >&2; exit 1 ;;
esac
if [ "$version" != "$HOST_DIR" ]; then
  rm -rf "${root:?}/$version"
  mv "$root/$HOST_DIR" "$root/$version"
fi
log "nexus-host in $root/$version"

# 2. nexus-host as a systemd user unit, enabled but not started: a started host would write its token and database
# into the image. The unit starts the newest folder under ~/.nexus/host, as the pre-stop hook finds it (the app's
# installer puts an update beside this one and touches it), and runs with linger so it outlives SSH sessions.
unit_dir="$HOME/.config/systemd/user"
mkdir -p "$unit_dir/default.target.wants"
cat > "$unit_dir/nexus-host.service" <<'UNIT'
[Unit]
Description=Nexus host (agent sessions for the Nexus desktop app)

[Service]
Environment=NEXUS_HOST_DIR=%h/.nexus/host
Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin
Environment=DISABLE_AUTOUPDATER=1
Environment=PLAYWRIGHT_BROWSERS_PATH=%h/.nexus/browsers
ExecStart=/bin/sh -c 'exec "$(ls -1t "$HOME"/.nexus/host/*/nexus-host | head -n 1)" serve'
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
UNIT
ln -sf ../nexus-host.service "$unit_dir/default.target.wants/nexus-host.service"
me="$(id -un)"
if [ "$(loginctl show-user "$me" --property=Linger --value 2>/dev/null || true)" != yes ]; then
  sudo -n loginctl enable-linger "$me"
fi
log "systemd user unit enabled, linger on"

}

stage_agents() {
# 3. Claude Code, pinned, in ~/.local/bin (it shadows boat's own copy). No self-update; the first-run screen done.
npm install -g --prefix "$HOME/.local" "@anthropic-ai/claude-code@$CLAUDE_CODE_VERSION"
case "$("$HOME/.local/bin/claude" --version)" in
  "$CLAUDE_CODE_VERSION"*) ;;
  *) echo "image: claude is not $CLAUDE_CODE_VERSION" >&2; exit 1 ;;
esac
mkdir -p "$HOME/.claude"
python3 - "$HOME/.claude.json" "$HOME/.claude/settings.json" <<'PY'
import json, os, sys

def edit(path, change):
    try:
        with open(path) as f:
            data = json.load(f)
    except FileNotFoundError:
        data = {}
    if not isinstance(data, dict):
        sys.exit("%s is not a JSON object" % path)
    change(data)
    part = path + ".part"
    with open(part, "w") as f:
        json.dump(data, f, indent=2)
        f.write("\n")
    os.replace(part, path)

def state(data):
    data["hasCompletedOnboarding"] = True
    data["autoUpdates"] = False

def settings(data):
    env = data.setdefault("env", {})
    env["DISABLE_AUTOUPDATER"] = "1"
    env["PLAYWRIGHT_BROWSERS_PATH"] = os.path.expanduser("~/.nexus/browsers")

edit(sys.argv[1], state)
edit(sys.argv[2], settings)
PY
log "claude $("$HOME/.local/bin/claude" --version)"

# Codex, pinned the same way, with no sign-in: the person signs in on their own machine.
npm install -g --prefix "$HOME/.local" "@openai/codex@$CODEX_VERSION"
case "$("$HOME/.local/bin/codex" --version)" in
  *"$CODEX_VERSION"*) ;;
  *) echo "image: codex is not $CODEX_VERSION" >&2; exit 1 ;;
esac
log "codex $("$HOME/.local/bin/codex" --version)"

# OpenCode v2, pinned the same way on npm, with no sign-in: the Nexus host injects OPENCODE_CONFIG_CONTENT per run.
npm install -g --prefix "$HOME/.local" "@opencode/cli@$OPENCODE_VERSION"
case "$("$HOME/.local/bin/opencode" --version)" in
  *"$OPENCODE_VERSION"*) ;;
  *) echo "image: opencode is not $OPENCODE_VERSION" >&2; exit 1 ;;
esac
log "opencode $("$HOME/.local/bin/opencode" --version)"

# 5. uv, pinned, in ~/.local/bin.
curl -LsSf "https://astral.sh/uv/$UV_VERSION/install.sh" | UV_UNMANAGED_INSTALL="$HOME/.local/bin" sh
"$HOME/.local/bin/uv" --version
}

stage_browser() {
# 4. The Playwright MCP package, as link.rs installs it when mcp.json names it (global, with a sudo fallback).
if ! npm ls -g "$PLAYWRIGHT_MCP" >/dev/null 2>&1; then
  npm install -g "$PLAYWRIGHT_MCP" || sudo -n npm install -g "$PLAYWRIGHT_MCP"
fi
log "playwright mcp: $PLAYWRIGHT_MCP"
# Its Chromium and the system libraries it needs, of the playwright version the MCP package depends on, so a session
# takes screenshots and records video without a download first.
playwright="$(python3 - "$(npm root -g)/${PLAYWRIGHT_MCP%@*}/package.json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["dependencies"]["playwright"])
PY
)"
[ -n "$playwright" ] || { echo "image: could not read the playwright version of $PLAYWRIGHT_MCP" >&2; exit 1; }
mkdir -p "$browsers"
sudo -n env "PLAYWRIGHT_BROWSERS_PATH=$browsers" npx --yes "playwright@$playwright" install-deps chromium
PLAYWRIGHT_BROWSERS_PATH="$browsers" npx --yes "playwright@$playwright" install chromium
log "playwright $playwright: chromium in $browsers"
}

stage_media() {
# ffmpeg, for the agents' video work.
sudo -n env DEBIAN_FRONTEND=noninteractive apt-get update -y
sudo -n env DEBIAN_FRONTEND=noninteractive apt-get install -y ffmpeg
log "ffmpeg installed"
}

stage_finish() {
# 6. boat's pre-stop hook: stop the host cleanly before the disk is saved (the same as link.rs writes at connect).
mkdir -p "$HOME/.boat"
cat > "$HOME/.boat/pre-stop" <<'HOOK'
#!/bin/sh
# Written by Nexus (nexus-cloud image); the app rewrites it on every connect.
root="$HOME/.nexus/host"
export HOME NEXUS_HOST_DIR="$root"
bin=""
pid="$(cat "$root/run/host.lock" 2>/dev/null)" || pid=""
if [ -n "$pid" ]; then bin="$(readlink "/proc/$pid/exe" 2>/dev/null)" || bin=""; fi
if [ -z "$bin" ] || [ ! -x "$bin" ]; then bin="$(ls -1t "$root"/*/nexus-host 2>/dev/null | head -n 1)" || bin=""; fi
if [ -n "$bin" ] && [ -x "$bin" ]; then "$bin" stop >/dev/null 2>&1; fi
exit 0
HOOK
chmod 700 "$HOME/.boat/pre-stop"

# 7. What the snapshot leaves out: caches and logs that only make the image larger and the first start slower.
cat > "$HOME/.boatignore" <<'IGNORE'
.cache/
.npm/
.nexus/host/run/
.nexus/cloud/*.log
IGNORE

# 8. boat writes its own CLI config (.config/ascii) on every new machine, the builder too: it must not be in the image.
rm -rf "$HOME/.config/ascii"
log "done"
# The versions in the image, for whoever debugs a machine.
host_dir="$(basename "$(dirname "$(ls -1t "$root"/*/nexus-host | head -n 1)")")"
python3 - "$HOME/.nexus/cloud-image.json" "$host_dir" "$CLAUDE_CODE_VERSION" "$CODEX_VERSION" "$OPENCODE_VERSION" "$PLAYWRIGHT_MCP" "$UV_VERSION" <<'PY'
import json, sys
keys = ["hostDir", "claudeCode", "codex", "opencode", "playwrightMcp", "uv"]
with open(sys.argv[1], "w") as f:
    json.dump(dict(zip(keys, sys.argv[2:])), f, indent=2)
    f.write("\n")
PY
}

case "$STAGE" in
  host) stage_host ;;
  agents) stage_agents ;;
  browser) stage_browser ;;
  media) stage_media ;;
  finish) stage_finish ;;
  *) echo "image: unknown STAGE $STAGE" >&2; exit 1 ;;
esac

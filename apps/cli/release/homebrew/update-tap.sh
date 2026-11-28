#!/usr/bin/env bash
# Move the Homebrew tap's nexus-cli formula to a stable release that is already on downloads.nexus.inc.
#
#   update-tap.sh formula 0.42.0   # after cli/stable/latest.json names 0.42.0
#
# It waits for the public manifest to name the version (the Worker's copy can be about 60 s old), renders the file with
# render.py, and pushes one commit to the tap. TAP_TOKEN must be able to push to TAP_REPO. A version the tap already
# has is a no-op, so a rerun is safe. A manifest that names a newer version is left alone: the newer run moves the tap.
set -euo pipefail

kind="${1:?usage: update-tap.sh formula <version>}"
version="${2:?usage: update-tap.sh formula <version>}"
repo="${TAP_REPO:-nexus-xyz/homebrew-tap}"
here="$(cd "$(dirname "$0")" && pwd)"

case "$kind" in
  formula) manifest_url="https://downloads.nexus.inc/cli/stable/latest.json"; file="Formula/nexus-cli.rb" ;;
  *) echo "unknown kind: $kind" >&2; exit 2 ;;
esac
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "not a stable version: $version" >&2; exit 2; }
: "${TAP_TOKEN:?TAP_TOKEN is not set}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Up to five minutes for the manifest to name this version.
served=""
for _ in $(seq 1 30); do
  curl -fsS --max-time 20 -H 'Cache-Control: no-cache' "$manifest_url?v=$version" -o "$work/latest.json" || true
  served="$(jq -r '.version // empty' "$work/latest.json" 2> /dev/null || true)"
  [ "$served" = "$version" ] && break
  if [ -n "$served" ] && [ "$(printf '%s\n%s\n' "$served" "$version" | sort -V | tail -1)" = "$served" ]; then
    echo "$manifest_url names $served, newer than $version: the tap follows that release instead."
    exit 0
  fi
  sleep 10
done
if [ "$served" != "$version" ]; then
  echo "$manifest_url still names '${served:-nothing}', not $version." >&2
  exit 1
fi

# TAP_URL overrides the remote, for a local test against a bare repository.
git clone --quiet --depth 1 "${TAP_URL:-https://x-access-token:${TAP_TOKEN}@github.com/${repo}.git}" "$work/tap"
mkdir -p "$work/tap/$(dirname "$file")"
python3 "$here/render.py" "$kind" --manifest "$work/latest.json" > "$work/tap/$file"
cd "$work/tap"
if [ -z "$(git status --porcelain -- "$file")" ]; then
  echo "$repo already has $file at $version."
  exit 0
fi
git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git add "$file"
git commit --quiet -m "$(basename "$file" .rb) $version"
# Another release (the desktop cask) can push at the same time. Each touches its own file, so a rebase never conflicts.
branch="$(git rev-parse --abbrev-ref HEAD)"
for attempt in 1 2 3; do
  git push --quiet origin "HEAD:$branch" && break
  [ "$attempt" = 3 ] && { echo "push to $repo failed three times" >&2; exit 1; }
  git pull --quiet --rebase origin "$branch"
done
echo "Pushed $file $version to $repo."

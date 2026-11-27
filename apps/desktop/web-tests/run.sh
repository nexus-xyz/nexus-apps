#!/usr/bin/env bash
# Open each test page in the GPUI app's page panel, once per engine, and screenshot the window (niri only).
# Usage: ./run.sh [cef|servo|both] [page-glob]. Shots land in shots/<engine>/<page>.png.
set -u
here=$(cd "$(dirname "$0")" && pwd)
app="$here/../target/release/nexus-desktop-gpui"
engines=${1:-both}; [ "$engines" = both ] && engines="cef servo"
glob=${2:-*}
wait_s=${WAIT:-10}
python3 "$here/build.py" >/dev/null
shots_dir="$HOME/Pictures/Screenshots"
for engine in $engines; do
  mkdir -p "$here/shots/$engine"
  for page in "$here"/out/$glob.html; do
    name=$(basename "$page" .html)
    env NEXUS_WEB_URL="${BASE:-file://$here/out}/$name.html" $([ "$engine" = servo ] && echo NEXUS_WEB_ENGINE=servo) "$app" >"$here/shots/$engine/$name.log" 2>&1 &
    pid=$!
    sleep "$wait_s"
    id=$(niri msg -j windows | python3 -c "import json,sys; print(next((w['id'] for w in json.load(sys.stdin) if w.get('pid')==$pid), ''))")
    if [ -n "$id" ]; then
      before=$(ls -t "$shots_dir" 2>/dev/null | head -1)
      niri msg action screenshot-window --id "$id" >/dev/null
      sleep 1
      after=$(ls -t "$shots_dir" | head -1)
      [ "$after" != "$before" ] && mv "$shots_dir/$after" "$here/shots/$engine/$name.png"
    fi
    kill "$pid" 2>/dev/null; sleep 1; kill -9 "$pid" 2>/dev/null
    pkill -9 -P "$pid" 2>/dev/null
    echo "$engine $name ${id:-NO-WINDOW}"
  done
done

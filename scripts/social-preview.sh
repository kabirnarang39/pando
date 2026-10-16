#!/bin/sh
# Render scripts/social-preview.html to the 1280x640 social preview PNGs (needs Google Chrome).
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
chrome="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
"$chrome" --headless=new --disable-gpu --hide-scrollbars --virtual-time-budget=5000 --window-size=1280,640 \
  --screenshot="$root/docs/assets/social-preview.png" "file://$root/scripts/social-preview.html"
cp "$root/docs/assets/social-preview.png" "$root/site/social-preview.png"

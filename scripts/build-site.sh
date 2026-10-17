#!/bin/sh
# Assemble the GitHub Pages site: landing page, brand and chart assets, mdBook docs.
# Usage: scripts/build-site.sh OUT_DIR   (needs `mdbook` on PATH, or MDBOOK=/path/to/mdbook)
set -eu
out="${1:?output directory}"
root="$(cd "$(dirname "$0")/.." && pwd)"
rm -rf "$out"
mkdir -p "$out/charts"
cp "$root/site/index.html" "$root/site/social-preview.png" "$out/"
cp "$root/docs/assets/brand/mark.svg" "$out/mark.svg"
cp "$root"/docs/assets/charts/*.svg "$out/charts/"
"${MDBOOK:-mdbook}" build "$root/docs" -d "$out/docs"
# The console, if built (console/dist): its demo runs entirely in the browser.
if [ -d "$root/console/dist" ]; then cp -R "$root/console/dist" "$out/console"; fi

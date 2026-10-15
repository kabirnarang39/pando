#!/bin/sh
# Install the latest Sapwood release (sap + sapwood) for macOS or Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/kabirnarang39/sapwood/main/install.sh | sh
#
# Environment:
#   SAPWOOD_VERSION   release tag to install (default: latest), e.g. v1.0.1
#   SAPWOOD_INSTALL   install directory (default: ~/.local/bin)
set -eu

REPO="kabirnarang39/sapwood"
DEST="${SAPWOOD_INSTALL:-$HOME/.local/bin}"

fail() { echo "sapwood install: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "'$1' is required"; }
need curl
need tar
need uname

case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux) os=unknown-linux-gnu ;;
  *) fail "unsupported OS $(uname -s); on Windows download the .zip from https://github.com/$REPO/releases" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  arm64 | aarch64) arch=aarch64 ;;
  *) fail "unsupported architecture $(uname -m)" ;;
esac

version="${SAPWOOD_VERSION:-}"
if [ -z "$version" ]; then
  version="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
  [ -n "$version" ] || fail "could not determine the latest release"
fi

name="sapwood-$version-$arch-$os"
url="https://github.com/$REPO/releases/download/$version/$name.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $name"
curl -fsSL "$url" -o "$tmp/$name.tar.gz" || fail "download failed: $url"
curl -fsSL "$url.sha256" -o "$tmp/$name.tar.gz.sha256" || fail "checksum download failed"

expected="$(cut -d' ' -f1 < "$tmp/$name.tar.gz.sha256")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$name.tar.gz" | cut -d' ' -f1)"
else
  actual="$(shasum -a 256 "$tmp/$name.tar.gz" | cut -d' ' -f1)"
fi
[ "$expected" = "$actual" ] || fail "checksum mismatch (expected $expected, got $actual)"

tar xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$DEST"
for bin in sap sapwood; do
  install -m 0755 "$tmp/$name/$bin" "$DEST/$bin"
done

echo "Installed sap and sapwood $version to $DEST"
case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "Add $DEST to your PATH, for example: export PATH=\"$DEST:\$PATH\"" ;;
esac

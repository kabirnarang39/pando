#!/bin/sh
# Print the Homebrew formula for a published release, using its checksum files.
# Usage: scripts/homebrew-formula.sh v1.8.0 > Formula/pando.rb   (needs gh)
set -eu
tag=$1

base="https://github.com/kabirnarang39/pando/releases/download/$tag"
sum() {
  gh release download "$tag" --repo kabirnarang39/pando --pattern "pando-$tag-$1.tar.gz.sha256" --output - | cut -d' ' -f1
}
# Fetch every checksum before printing anything: a release still being built has no assets
# yet, and a formula with an empty checksum must never be written.
for t in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
  s=$(sum "$t") || s=""
  case "$s" in
    [0-9a-f]*) ;;
    *) echo "no checksum for $t in $tag; is the release published?" >&2; exit 1 ;;
  esac
  eval "sum_$(echo "$t" | tr '-' '_')=$s"
done
asset() {
  eval "s=\$sum_$(echo "$1" | tr '-' '_')"
  printf '      url "%s/pando-%s-%s.tar.gz"\n      sha256 "%s"\n' "$base" "$tag" "$1" "$s"
}
cat <<RUBY
class Pando < Formula
  desc "Source control for monorepos at agent scale"
  homepage "https://kabirnarang39.github.io/pando/"
  license "Apache-2.0"

  on_macos do
    on_arm do
$(asset aarch64-apple-darwin)
    end
    on_intel do
$(asset x86_64-apple-darwin)
    end
  end

  on_linux do
    on_arm do
$(asset aarch64-unknown-linux-gnu)
    end
    on_intel do
$(asset x86_64-unknown-linux-gnu)
    end
  end

  def install
    bin.install "pando", "pandod", "git-remote-pando"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/pando --version")
    assert_match version.to_s, shell_output("#{bin}/pandod --version")
  end
end
RUBY

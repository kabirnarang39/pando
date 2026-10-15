#!/bin/sh
# Print the Homebrew formula for a published release, using its checksum files.
# Usage: scripts/homebrew-formula.sh v1.8.0 > Formula/sapwood.rb   (needs gh)
set -eu
tag=$1

base="https://github.com/kabirnarang39/sapwood/releases/download/$tag"
sum() {
  gh release download "$tag" --repo kabirnarang39/sapwood --pattern "sapwood-$tag-$1.tar.gz.sha256" --output - | cut -d' ' -f1
}
asset() {
  printf '      url "%s/sapwood-%s-%s.tar.gz"\n      sha256 "%s"\n' "$base" "$tag" "$1" "$(sum "$1")"
}
cat <<RUBY
class Sapwood < Formula
  desc "Source control for monorepos at agent scale"
  homepage "https://kabirnarang39.github.io/sapwood/"
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
    bin.install "sap", "sapwood", "git-remote-sapwood"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/sap --version")
    assert_match version.to_s, shell_output("#{bin}/sapwood --version")
  end
end
RUBY

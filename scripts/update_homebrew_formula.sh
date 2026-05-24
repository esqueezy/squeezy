#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -ne 3 ]]; then
  echo "usage: $0 <release-tag> <release-assets-dir> <homebrew-tap-dir>" >&2
  exit 2
fi

release_tag="$1"
assets_dir="$2"
tap_dir="$3"
repository="${SQUEEZY_RELEASE_REPOSITORY:-esqueezy/squeezy}"

if [[ ! "$release_tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.+-]+)?$ ]]; then
  echo "release tag must match vMAJOR.MINOR.PATCH[-PRERELEASE]: $release_tag" >&2
  exit 1
fi

if [[ ! -d "$assets_dir" ]]; then
  echo "release assets dir not found: $assets_dir" >&2
  exit 1
fi

if [[ ! -d "$tap_dir" ]]; then
  echo "Homebrew tap dir not found: $tap_dir" >&2
  exit 1
fi

sha256_for() {
  local archive="$1"
  local checksum_file
  checksum_file="$(find "$assets_dir" -type f -name "${archive}.sha256" -print -quit)"
  if [[ -z "$checksum_file" ]]; then
    echo "missing checksum for archive: $archive" >&2
    exit 1
  fi
  awk '{print $1}' "$checksum_file"
}

url_for() {
  local archive="$1"
  printf 'https://github.com/%s/releases/download/%s/%s' "$repository" "$release_tag" "$archive"
}

version="${release_tag#v}"
x86_macos="squeezy-x86_64-apple-darwin.tar.gz"
arm_macos="squeezy-aarch64-apple-darwin.tar.gz"
x86_linux="squeezy-x86_64-unknown-linux-musl.tar.gz"

formula_dir="$tap_dir/Formula"
formula_file="$formula_dir/squeezy.rb"
mkdir -p "$formula_dir"

cat > "$formula_file" <<FORMULA
class Squeezy < Formula
  desc "Cost-aware coding agent TUI with local semantic code navigation"
  homepage "https://github.com/${repository}"
  version "${version}"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "$(url_for "$arm_macos")"
      sha256 "$(sha256_for "$arm_macos")"
    else
      url "$(url_for "$x86_macos")"
      sha256 "$(sha256_for "$x86_macos")"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "$(url_for "$x86_linux")"
      sha256 "$(sha256_for "$x86_linux")"
    else
      odie "Squeezy only publishes x86_64 Linux Homebrew archives for now"
    end
  end

  def install
    bin.install "squeezy"
  end

  test do
    assert_match "squeezy: ok", shell_output("#{bin}/squeezy --health")
  end
end
FORMULA

echo "updated $formula_file"

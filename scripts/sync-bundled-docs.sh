#!/usr/bin/env bash
# Copy docs/external/*.md into crates/squeezy-skills/bundled-docs/external/
# so the squeezy-skills crate can be packaged for crates.io. Local builds
# read docs/external/ directly; the bundled copy is only consulted when
# build.rs runs from inside a published tarball (no workspace path).
#
# Run before `cargo publish -p squeezy-skills` (or `cargo package`).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
src="$repo_root/docs/external"
dst="$repo_root/crates/squeezy-skills/bundled-docs/external"

if [[ ! -d "$src" ]]; then
  echo "source docs not found: $src" >&2
  exit 1
fi

rm -rf "$dst"
mkdir -p "$dst"
cp "$src"/*.md "$dst"/

echo "synced $(ls "$dst" | wc -l | tr -d ' ') docs to crates/squeezy-skills/bundled-docs/external/"

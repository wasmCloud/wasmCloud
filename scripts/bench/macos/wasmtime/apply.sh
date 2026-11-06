#!/usr/bin/env bash
# Vendor wasmtime from the cargo registry, apply patches/*.patch, and point
# this workspace at the result with [patch.crates-io].
#
#   scripts/bench/macos/wasmtime/apply.sh
#
# DEST (default ../wasmtime-<version>-patched beside the repo) is where the
# patched crate goes; it must not exist yet. NO_CARGO_PATCH=1 skips editing
# Cargo.toml. Undo: delete DEST and the [patch.crates-io] wasmtime line.
set -euo pipefail

# shellcheck disable=SC1091
command -v cargo >/dev/null || . "$HOME/.cargo/env"

here="$(cd "$(dirname "$0")" && pwd)"
root="$(git -C "$here" rev-parse --show-toplevel)"
version=48.0.8 # the patches are made against this release
dest="${DEST:-$(dirname "$root")/wasmtime-${version}-patched}"

locked=$(awk '/^name = "wasmtime"$/ {getline; gsub(/[" ]/, "", $0); sub(/version=/, "", $0); print; exit}' "$root/Cargo.lock")
if [ "$locked" != "$version" ]; then
  echo "Cargo.lock has wasmtime $locked; these patches are for $version. Rebase them first." >&2
  exit 1
fi
if [ -e "$dest" ]; then
  echo "$dest already exists; remove it or set DEST." >&2
  exit 1
fi

(cd "$root" && cargo fetch -q)
src=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/"wasmtime-$version" 2>/dev/null | head -1)
[ -n "$src" ] || { echo "wasmtime-$version is not in the cargo registry" >&2; exit 1; }

cp -R "$src" "$dest"
git -C "$dest" init -q
git -C "$dest" add -A
git -C "$dest" -c user.name=vendor -c user.email=vendor@localhost commit -q -m "wasmtime $version from crates.io"
git -C "$dest" -c user.name=vendor -c user.email=vendor@localhost am -q "$here"/patches/*.patch
git -C "$dest" log --oneline

if [ -z "${NO_CARGO_PATCH:-}" ] && ! grep -q '^wasmtime = { path = ' "$root/Cargo.toml"; then
  rel=$(python3 -I -c 'import os,sys; print(os.path.relpath(sys.argv[1], sys.argv[2]))' "$dest" "$root")
  printf '\n# Local wasmtime with scripts/bench/macos/wasmtime/patches applied.\n[patch.crates-io]\nwasmtime = { path = "%s" }\n' "$rel" >> "$root/Cargo.toml"
  echo "added [patch.crates-io] wasmtime = { path = \"$rel\" } to Cargo.toml"
fi
echo "patched wasmtime at $dest; enable with WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT=262144"

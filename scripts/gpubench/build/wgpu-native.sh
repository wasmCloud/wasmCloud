#!/usr/bin/env bash
# Build wgpu-native (the webgpu.h step 2 links) on exactly the wgpu crates
# wash-runtime resolves, so steps 2 and 3 run one WebGPU implementation. The
# release binaries can lag a patch behind this repo's Cargo.lock, so this
# rebuilds from the pinned tag with the wgpu crates moved to the host's
# versions, and refuses to finish if they still differ. wgpu-native-patches/
# adds the async glue ggml needs (futures, WaitAny, WGSL feature queries)
# without changing how wgpu-core does the work.
#
#   build/wgpu-native.sh       prints the install prefix (lib/ + include/)

set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"

src="$src_dir/wgpu-native"
fetch_git "$WGPU_NATIVE_REPO" "$WGPU_NATIVE_COMMIT" "$src" --submodules
apply_patches "$src" "$gpubench_dir/wgpu-native-patches"

want="$(host_wgpu_versions "$repo_root/Cargo.lock")"
[ -n "$want" ] || die "no wgpu crates in $repo_root/Cargo.lock"
key="$( { printf "%s" "$want"; cat "$gpubench_dir/cmake/wgpu-native/webgpu.h" "$gpubench_dir"/wgpu-native-patches/*.patch; } | sha256_of /dev/stdin | cut -c1-12)"
prefix="$build_root/wgpu-native-$WGPU_NATIVE_TAG-$key"
if [ -f "$prefix/lib/libwgpu_native.a" ]; then
  echo "$prefix"
  exit 0
fi

# Moving one crate can drag another along, so repeat until nothing moves.
for _ in 1 2 3; do
  while read -r name version; do
    have="$(host_wgpu_versions "$src/Cargo.lock" | awk -v n="$name" '$1 == n { print $2 }')"
    [ "$have" = "$version" ] && continue
    log "wgpu-native: $name $have -> $version"
    (cd "$src" && cargo update -q -p "$name@$have" --precise "$version")
  done <<<"$want"
done
got="$(host_wgpu_versions "$src/Cargo.lock")"
[ "$got" = "$want" ] || die "wgpu-native resolves
$got
but wash-runtime resolves
$want"

(cd "$src" && cargo build -q --release --locked) >&2

# Dawn's C++ wrapper targets a newer webgpu.h than wgpu-native ships. The
# compat header adds what's missing; that is only sound while every enum the
# two headers share has the same value, so check that here.
fetch_git "$WASI_WEBGPU_HEADERS_REPO" "$WASI_WEBGPU_HEADERS_COMMIT" "$src_dir/wasi-webgpu-headers" --submodules
enums() { grep -oE '^ *WGPU[A-Za-z0-9_]+ = 0x[0-9A-Fa-f]+' "$1" | tr -d ' ' | sort -u; }
skew="$(join -t= <(enums "$src_dir/wasi-webgpu-headers/webgpu/webgpu.h") <(enums "$src/ffi/webgpu-headers/webgpu.h") |
  awk -F= '$2 != $3')"
[ -z "$skew" ] || die "webgpu.h enum values differ between Dawn's target and wgpu-native:
$skew"

mkdir -p "$prefix.part/lib" "$prefix.part/include/webgpu"
cp "$src/target/release/libwgpu_native.a" "$prefix.part/lib/"
cp "$src/ffi/webgpu-headers/webgpu.h" "$prefix.part/include/webgpu/wgpu-native-webgpu.h"
cp "$gpubench_dir/cmake/wgpu-native/webgpu.h" "$src/ffi/wgpu.h" "$prefix.part/include/webgpu/"
printf '%s\n' "$want" >"$prefix.part/wgpu-versions.txt"
rm -rf "$prefix"
mv "$prefix.part" "$prefix"
echo "$prefix"

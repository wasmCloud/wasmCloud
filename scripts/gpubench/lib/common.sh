# Shared helpers for scripts/gpubench. Source, don't execute.
#
# Everything downloaded or built lives under $GPUBENCH_CACHE, outside the
# checkout, so a CI checkout's clean step doesn't throw away an hour of builds
# or the model files.

# shellcheck disable=SC2034  # variables consumed by the scripts that source this
gpubench_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "$gpubench_dir/../.." && pwd)"

# shellcheck source=../pins.env
. "$gpubench_dir/pins.env"

GPUBENCH_CACHE="${GPUBENCH_CACHE:-$HOME/.cache/wasmcloud-gpubench}"
src_dir="$GPUBENCH_CACHE/src"
tools_dir="$GPUBENCH_CACHE/tools"
build_root="$GPUBENCH_CACHE/build"
model_dir="${GPUBENCH_MODEL_DIR:-$GPUBENCH_CACHE/models}"
mkdir -p "$src_dir" "$tools_dir" "$build_root" "$model_dir"

log() { printf '==> %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# The box label, which keys the rows: `<kind>-<class>` in CI (e.g. cuda-l4;
# .github/scripts/gpubench-boxes.mjs), else the detected kind. `vulkan-sw` is
# a Linux host with no GPU, where wgpu runs on Mesa's lavapipe: it exercises
# the WebGPU steps end to end and its numbers mean nothing.
gpubench_box() {
  if [ -n "${GPUBENCH_BOX:-}" ]; then
    echo "$GPUBENCH_BOX"
    return
  fi
  case "$(uname -s)" in
    Darwin) echo metal ;;
    Linux)
      if command -v nvidia-smi >/dev/null 2>&1; then echo cuda; else echo vulkan-sw; fi
      ;;
    *) die "unsupported OS $(uname -s)" ;;
  esac
}

# The box kind, which picks the backend: `metal` (macOS, Apple GPU), `cuda`
# (Linux, NVIDIA GPU), or `vulkan` for vulkan-sw.
gpubench_kind() {
  local box
  box="$(gpubench_box)"
  echo "${box%%-*}"
}

ncpu() { getconf _NPROCESSORS_ONLN; }

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# fetch_file URL SHA256 DEST: download once, verify every time.
fetch_file() {
  local url="$1" want="$2" dest="$3"
  if [ ! -f "$dest" ]; then
    log "downloading $url"
    curl -fL --retry 3 -o "$dest.part" "$url"
    mv "$dest.part" "$dest"
  fi
  local got
  got="$(sha256_of "$dest")"
  [ "$got" = "$want" ] || die "$dest: sha256 $got, expected $want"
}

# fetch_git REPO COMMIT DIR [--submodules]: a shallow checkout of exactly COMMIT.
fetch_git() {
  local repo="$1" commit="$2" dir="$3" submodules="${4:-}"
  if [ -d "$dir/.git" ] && [ "$(git -C "$dir" rev-parse HEAD)" = "$commit" ]; then
    return
  fi
  log "fetching $repo @ $commit"
  rm -rf "$dir"
  git init -q "$dir"
  git -C "$dir" remote add origin "$repo"
  git -C "$dir" fetch -q --depth 1 origin "$commit"
  git -C "$dir" -c advice.detachedHead=false checkout -q FETCH_HEAD
  if [ "$submodules" = --submodules ]; then
    git -C "$dir" submodule update -q --init --recursive --depth 1
  fi
}

wasi_sdk_path() {
  local os arch sha
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) os=macos arch=arm64 sha="$WASI_SDK_SHA256_ARM64_MACOS" ;;
    Linux-x86_64) os=linux arch=x86_64 sha="$WASI_SDK_SHA256_X86_64_LINUX" ;;
    *) die "no wasi-sdk pin for $(uname -s)-$(uname -m)" ;;
  esac
  local name="wasi-sdk-${WASI_SDK_VERSION}.0-${arch}-${os}"
  local dir="$tools_dir/$name"
  if [ ! -x "$dir/bin/clang" ]; then
    fetch_file "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${WASI_SDK_VERSION}/${name}.tar.gz" \
      "$sha" "$tools_dir/${name}.tar.gz"
    tar -xzf "$tools_dir/${name}.tar.gz" -C "$tools_dir"
    # Gatekeeper quarantines the unpacked binaries on macOS.
    [ "$os" = macos ] && xattr -dr com.apple.quarantine "$dir" 2>/dev/null || true
  fi
  echo "$dir"
}

dawn_headers_path() {
  local dir="$tools_dir/dawn-headers-$DAWN_HEADERS_COMMIT"
  if [ ! -d "$dir" ]; then
    local tgz="$tools_dir/dawn-headers-$DAWN_HEADERS_COMMIT.tar.gz"
    fetch_file "https://github.com/google/dawn/releases/download/$DAWN_HEADERS_RELEASE/dawn-headers-$DAWN_HEADERS_COMMIT.tar.gz" \
      "$DAWN_HEADERS_SHA256" "$tgz"
    mkdir -p "$dir.part"
    tar -xzf "$tgz" -C "$dir.part"
    mv "$dir.part" "$dir"
  fi
  echo "$dir"
}

# The wgpu crate versions wash-runtime resolves, as `name version` lines.
host_wgpu_versions() {
  awk '
    /^\[\[package\]\]/ { name = "" }
    /^name = / { gsub(/"/, "", $3); name = $3 }
    /^version = / && (name == "wgpu-core" || name == "wgpu-hal" || name == "wgpu-types" || name == "naga") {
      gsub(/"/, "", $3); print name, $3
    }
  ' "$1" | sort -u
}

# Apply every PATCHDIR/*.patch to the checkout at SRC, skipping ones already
# applied. A patch that neither applies nor is applied means the pin moved.
apply_patches() {
  local src="$1" dir="$2" patch
  for patch in "$dir"/*.patch; do
    if git -C "$src" apply --check "$patch" 2>/dev/null; then
      log "applying $(basename "$patch")"
      git -C "$src" apply "$patch"
    else
      git -C "$src" apply --check --reverse "$patch" 2>/dev/null ||
        die "$(basename "$patch") neither applies nor is applied to $src"
    fi
  done
}

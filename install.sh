#!/bin/bash

# Install script for wash - The Wasm Shell
# Usage: curl -fsSL https://raw.githubusercontent.com/wasmcloud/wasmCloud/main/install.sh | bash
# Options: -v to enable signature verification (requires GitHub CLI)
#
# Environment variables:
# - GITHUB_TOKEN: GitHub personal access token (optional, for higher API rate limits)
# - INSTALL_DIR: Directory to install wash binary (default: ~/.wash/bin)
# - WASH_VERSION: Specific version to install (default: latest). Can also be set via --version flag.

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Constants
REPO="wasmcloud/wasmCloud"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.wash/bin}"
TMP_DIR="/tmp/wash-install-$$"
VERIFY_SIGNATURE=false
NO_MODIFY_PATH=false
VERSION="${WASH_VERSION:-}"

# Helper functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1" >&2
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1" >&2
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1" >&2
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1" >&2
}

cleanup() {
    if [ -d "$TMP_DIR" ]; then
        rm -rf "$TMP_DIR"
    fi
}

trap cleanup EXIT

# Detect platform
detect_platform() {
    local os arch
    
    case "$(uname -s)" in
        Linux*)  os="unknown-linux-musl" ;;
        Darwin*) os="apple-darwin" ;;
        *)       log_error "Unsupported operating system: $(uname -s)"; exit 1 ;;
    esac
    
    case "$(uname -m)" in
        x86_64)  arch="x86_64" ;;
        arm64|aarch64) arch="aarch64" ;;
        *)       log_error "Unsupported architecture: $(uname -m)"; exit 1 ;;
    esac
    
    echo "${arch}-${os}"
}

# Get latest release information from GitHub API
get_latest_release() {
    local api_url="https://api.github.com/repos/${REPO}/releases/latest"
    local curl_args=("-s")

    if [ -n "${GITHUB_TOKEN:-}" ]; then
        curl_args+=("-H" "Authorization: token ${GITHUB_TOKEN:-}")
        log_info "Using GitHub token for API access"
    fi

    log_info "Fetching latest release information..."

    local response
    if ! response=$(curl "${curl_args[@]}" "$api_url" 2>/dev/null); then
        log_error "Failed to fetch release information from GitHub API"
        log_error "Please check your internet connection and try again"
        exit 1
    fi

    # Check for API errors (404, etc.)
    if echo "$response" | grep -q '"message".*"Not Found"'; then
        log_error "Repository ${REPO} not found or has no releases"
        log_error "Please verify the repository exists and has published releases"
        exit 1
    fi

    # Extract tag name using basic JSON parsing
    local tag_name
    if ! tag_name=$(echo "$response" | grep '"tag_name"' | head -n 1 | cut -d '"' -f 4); then
        log_error "Failed to parse release information from API response"
        log_error "API response: ${response}"
        exit 1
    fi

    if [ -z "$tag_name" ]; then
        log_error "No releases found for repository ${REPO}"
        log_error "Please verify the repository has published releases"
        exit 1
    fi

    echo "$tag_name"
}

# Get release information for a specific version
get_release_by_version() {
    local version="$1"
    local curl_args=("-s")

    if [ -n "${GITHUB_TOKEN:-}" ]; then
        curl_args+=("-H" "Authorization: token ${GITHUB_TOKEN:-}")
    fi

    # Build a list of candidate tags to try in order:
    # 1. The version as provided (e.g. v2.0.1 or wash-v2.0.0-rc.8)
    # 2. With 'wash-v' prefix, for pre-2.0 releases that used that convention
    local candidates=("$version")
    if [[ ! "$version" =~ ^wash-v ]]; then
        local bare="${version#v}"
        candidates+=("wash-v${bare}")
    fi

    local response tag_name
    for candidate in "${candidates[@]}"; do
        local api_url="https://api.github.com/repos/${REPO}/releases/tags/${candidate}"

        log_info "Fetching release information for version ${candidate}..."

        if ! response=$(curl "${curl_args[@]}" "$api_url" 2>/dev/null); then
            log_error "Failed to fetch release information from GitHub API"
            log_error "Please check your internet connection and try again"
            exit 1
        fi

        if echo "$response" | grep -q '"message".*"Not Found"'; then
            continue
        fi

        if tag_name=$(echo "$response" | grep '"tag_name"' | head -n 1 | cut -d '"' -f 4) && [ -n "$tag_name" ]; then
            echo "$tag_name"
            return 0
        fi
    done

    log_error "Version ${version} not found"
    log_error "Please verify the version exists. You can check available versions at:"
    log_error "https://github.com/${REPO}/releases"
    exit 1
}

# Get asset ID for the specified platform
get_asset_id_for_platform() {
    local platform="$1"
    local version="$2"
    local expected_name="wash-${platform}"

    local api_url
    if [ -n "$version" ]; then
        api_url="https://api.github.com/repos/${REPO}/releases/tags/${version}"
    else
        api_url="https://api.github.com/repos/${REPO}/releases/latest"
    fi

    local curl_args=("-s")

    if [ -n "${GITHUB_TOKEN:-}" ]; then
        curl_args+=("-H" "Authorization: token ${GITHUB_TOKEN:-}")
    fi

    local response
    if ! response=$(curl "${curl_args[@]}" "$api_url" 2>/dev/null); then
        log_error "Failed to fetch release information for asset lookup" >&2
        return 1
    fi
    
    # Try to use jq if available for reliable JSON parsing
    if command -v jq >/dev/null 2>&1; then
        local asset_id
        asset_id=$(echo "$response" | jq -r ".assets[] | select(.name == \"${expected_name}\") | .id")
        echo "$asset_id"
        return 0
    fi
    
    # Fallback to basic text processing
    # This is a simplified approach that looks for the pattern:
    # "name": "wash-platform"
    # and then finds the "id": number before it
    local asset_id
    asset_id=$(echo "$response" | \
        grep -B 20 "\"name\": *\"${expected_name}\"" | \
        grep '"id":' | \
        tail -n 1 | \
        sed 's/.*"id": *\([0-9]*\).*/\1/')
    
    echo "$asset_id"
}

# Download and install wash binary
install_wash() {
    local platform="$1"
    local version="$2"
    
    log_info "Detected platform: ${platform}"
    log_info "Latest version: ${version}"
    
    # Get the asset ID for our platform
    log_info "Finding asset for platform..."
    local asset_id
    asset_id=$(get_asset_id_for_platform "$platform" "$version")
    
    if [ -z "$asset_id" ]; then
        log_error "No matching binary found for platform ${platform}"
        log_error "Available assets:"
        
        # Use same pattern as elsewhere for optional GitHub token
        local list_curl_args=("-s")
        if [ -n "${GITHUB_TOKEN:-}" ]; then
            list_curl_args+=(" -H" "Authorization: token ${GITHUB_TOKEN:-}")
        fi
        
        curl "${list_curl_args[@]}" \
            "https://api.github.com/repos/${REPO}/releases/latest" | \
            grep '"name"' | sed 's/.*"name": *"\([^"]*\)".*/  - \1/' >&2
        exit 1
    fi
    
    local download_url="https://api.github.com/repos/${REPO}/releases/assets/${asset_id}"
    log_info "Download URL: ${download_url}"
    
    # Create temporary directory
    mkdir -p "$TMP_DIR"
    
    # Download binary using GitHub API
    log_info "Downloading wash binary..."
    local curl_args=("-fL" "-o" "${TMP_DIR}/wash" "-H" "Accept: application/octet-stream")
    
    if [ -n "${GITHUB_TOKEN:-}" ]; then
        curl_args+=("-H" "Authorization: token ${GITHUB_TOKEN:-}")
    fi
    
    if ! curl "${curl_args[@]}" "$download_url"; then
        log_error "Failed to download wash binary from ${download_url}"
        exit 1
    fi
    
    # Make binary executable
    chmod +x "${TMP_DIR}/wash"

    # Verify signature if requested
    if ! verify_artifact_signature "${TMP_DIR}/wash" "$version"; then
        log_error "Signature verification failed! Aborting installation."
        exit 1
    fi

    # Create install directory if it doesn't exist
    mkdir -p "$INSTALL_DIR"

    # Move binary to install directory
    if ! mv "${TMP_DIR}/wash" "${INSTALL_DIR}/wash"; then
        log_error "Failed to install wash to ${INSTALL_DIR}"
        exit 1
    fi
    
    if [ "$VERIFY_SIGNATURE" = "true" ]; then
        log_success "wash ${version} installed and verified successfully to ${INSTALL_DIR}/wash"
    else
        log_success "wash ${version} installed successfully to ${INSTALL_DIR}/wash"
    fi
    
    # Test installation
    if "${INSTALL_DIR}/wash" --help >/dev/null 2>&1; then
        log_success "Verified installation"
    else
        log_warn "Could not verify installation. Try running: ${INSTALL_DIR}/wash --help"
    fi
    
    # Configure PATH
    configure_path

    # Show next steps
    echo >&2
    log_info "Next steps:"
    echo "  1. Run 'wash --help' to see available commands" >&2
    echo "  2. Run 'wash new' to create your first WebAssembly component" >&2
}

# Add INSTALL_DIR to the user's shell profile
configure_path() {
    # Check if INSTALL_DIR is already in PATH
    case ":$PATH:" in
        *":${INSTALL_DIR}:"*)
            log_info "${INSTALL_DIR} is already in PATH"
            return 0
            ;;
    esac

    # Skip profile modification if --no-modify-path was passed
    if [ "$NO_MODIFY_PATH" = "true" ]; then
        log_info "Skipping PATH modification (--no-modify-path)"
        log_info "Manually add ${INSTALL_DIR} to your PATH"
        return 0
    fi

    # Skip profile modification in CI or non-interactive shells
    if [ "${CI:-}" = "true" ] || [ ! -t 2 ]; then
        log_info "Non-interactive environment detected, skipping PATH modification"
        log_info "Add ${INSTALL_DIR} to your PATH to use wash"
        return 0
    fi

    local shell_name
    shell_name=$(basename "${SHELL:-}")

    case "$shell_name" in
        zsh)
            _patch_profile "${HOME}/.zshrc"
            ;;
        bash)
            # Prefer .bashrc; fall back to .bash_profile
            if [ -f "${HOME}/.bashrc" ]; then
                _patch_profile "${HOME}/.bashrc"
            else
                _patch_profile "${HOME}/.bash_profile"
            fi
            ;;
        fish)
            _patch_fish_profile
            ;;
        *)
            log_warn "Unknown shell '${shell_name}' — manually add ${INSTALL_DIR} to your PATH"
            ;;
    esac
}

# Append an export line to a POSIX shell profile, skipping if already present
_patch_profile() {
    local profile_file="$1"
    local line="export PATH=\"${INSTALL_DIR}:\$PATH\""

    if [ -f "$profile_file" ] && grep -qF "${INSTALL_DIR}" "$profile_file"; then
        log_info "${INSTALL_DIR} already referenced in ${profile_file}"
        return 0
    fi

    echo "" >> "$profile_file"
    echo "# Added by wash installer" >> "$profile_file"
    echo "$line" >> "$profile_file"
    log_success "Added ${INSTALL_DIR} to PATH in ${profile_file}"
    log_info "Restart your shell or run: source ${profile_file}"
}

# Append a fish-style PATH entry, skipping if already present
_patch_fish_profile() {
    local config_file="${HOME}/.config/fish/config.fish"

    if [ -f "$config_file" ] && grep -qF "${INSTALL_DIR}" "$config_file"; then
        log_info "${INSTALL_DIR} already referenced in ${config_file}"
        return 0
    fi

    mkdir -p "$(dirname "$config_file")"
    echo "" >> "$config_file"
    echo "# Added by wash installer" >> "$config_file"
    echo "fish_add_path ${INSTALL_DIR}" >> "$config_file"
    log_success "Added ${INSTALL_DIR} to PATH in ${config_file}"
    log_info "Restart your shell or run: source ${config_file}"
}

# Parse command line arguments
parse_args() {
    while [[ $# -gt 0 ]]; do
        case $1 in
            -v|--verify)
                VERIFY_SIGNATURE=true
                shift
                ;;
            --no-modify-path)
                NO_MODIFY_PATH=true
                shift
                ;;
            --version)
                if [ -z "$2" ] || [[ "$2" == -* ]]; then
                    log_error "--version requires a version argument"
                    show_help
                    exit 1
                fi
                VERSION="$2"
                shift 2
                ;;
            -h|--help)
                show_help
                exit 0
                ;;
            *)
                log_error "Unknown argument: $1"
                show_help
                exit 1
                ;;
        esac
    done
}

# Show help
show_help() {
    cat << EOF
Install script for wash - The Wasm Shell

Usage: $0 [OPTIONS]

Options:
  -v, --verify         Enable signature verification (requires GitHub CLI)
  --version VERSION    Install a specific version (e.g., v2.0.1, or wash-v2.0.0-rc.8 for pre-2.0 releases)
  --no-modify-path     Don't modify shell profile to add wash to PATH
  -h, --help           Show this help message

Environment variables:
  GITHUB_TOKEN    GitHub personal access token (optional, for higher API rate limits)
  INSTALL_DIR     Directory to install wash binary (default: ~/.wash/bin)

Examples:
  # Standard installation (latest version)
  curl -fsSL https://raw.githubusercontent.com/wasmcloud/wasmCloud/main/install.sh | bash

  # Install with signature verification
  curl -fsSL https://raw.githubusercontent.com/wasmcloud/wasmCloud/main/install.sh | bash -s -- -v

  # Install a specific version
  curl -fsSL https://raw.githubusercontent.com/wasmcloud/wasmCloud/main/install.sh | bash -s -- --version v2.0.1
EOF
}

# Check if signature verification is supported and dependencies are available
check_verification_support() {
    if [ "$VERIFY_SIGNATURE" != "true" ]; then
        return 0
    fi

    log_info "Signature verification requested"

    # Check if gh CLI is installed
    if ! command -v gh >/dev/null 2>&1; then
        log_error "Signature verification requires GitHub CLI (gh) but it's not installed"
        log_error "Install it from: https://cli.github.com/"
        exit 1
    fi

    # Check if gh CLI is authenticated
    if ! gh auth status >/dev/null 2>&1; then
        log_warn "GitHub CLI is not authenticated, which may limit verification capabilities"
        log_warn "Consider running: gh auth login"
    fi

    log_info "GitHub CLI dependency check passed"
}

# Verify artifact signature using GitHub attestations
verify_artifact_signature() {
    local artifact_path="$1"
    local version="$2"

    if [ "$VERIFY_SIGNATURE" != "true" ]; then
        return 0
    fi

    log_info "Verifying artifact attestations..."

    # Verify build provenance attestation
    if ! gh attestation verify "$artifact_path" \
        --repo "$REPO" \
        --predicate-type https://slsa.dev/provenance/v1; then
        log_error "Build provenance attestation verification failed!"
        return 1
    fi

    log_success "Artifact attestations verified successfully!"
}

# Main execution
main() {
    # Parse command line arguments
    parse_args "$@"

    log_info "Installing wash - The Wasm Shell"
    log_info "Install directory: ${INSTALL_DIR}"
    echo >&2

    # Check dependencies
    if ! command -v curl >/dev/null 2>&1; then
        log_error "curl is required but not installed"
        exit 1
    fi
    log_info "curl dependency check passed"

    # Check verification support if requested
    check_verification_support
    
    # Optional: Check for GitHub token (helps with API rate limits)
    if [ -n "${GITHUB_TOKEN:-}" ]; then
        log_info "Using GitHub token for API requests"
    else
        log_info "No GitHub token provided - using anonymous API requests (may hit rate limits)"
    fi
    
    # Detect platform
    local platform
    log_info "Detecting platform..."
    platform=$(detect_platform)
    log_info "Platform detected: ${platform}"
    
    # Get release version
    local version
    if [ -n "$VERSION" ]; then
        log_info "Fetching release information for version ${VERSION}..."
        version=$(get_release_by_version "$VERSION")
    else
        log_info "Fetching latest release information..."
        version=$(get_latest_release)
    fi
    log_info "Version: ${version}"
    
    # Install wash
    install_wash "$platform" "$version"
}

# Run main function
main "$@"
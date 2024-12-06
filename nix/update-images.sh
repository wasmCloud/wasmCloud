#!/usr/bin/env bash

set -xe

update() {
    local dir
    dir="$(mktemp -d)"

    local amd64
    amd64=$(skopeo inspect --format '{{ .Digest }}' --override-os linux --override-arch amd64 "docker://${1}:${2}")
    skopeo \
        --override-os linux \
        --override-arch amd64 \
        --insecure-policy \
        copy \
        --src-tls-verify \
        "docker://${1}@${amd64}" \
        "docker-archive://${dir}/${3}-amd64.tar:${1}:${2}" \
        >&2

    local arm64
    arm64=$(skopeo inspect --format '{{ .Digest }}' --override-os linux --override-arch arm64 "docker://${1}:${2}")
    skopeo \
        --override-os linux \
        --override-arch arm64 \
        --insecure-policy \
        copy \
        --src-tls-verify \
        "docker://${1}@${arm64}" \
        "docker-archive://${dir}/${3}-arm64.tar:${1}:${2}" \
        >&2

    echo "  ${3}-amd64.arch = \"amd64\";"
    echo "  ${3}-amd64.finalImageName = \"${1}\";"
    echo "  ${3}-amd64.finalImageTag = \"${2}\";"
    echo "  ${3}-amd64.imageDigest = \"${amd64}\";"
    echo "  ${3}-amd64.imageName = \"${1}\";"
    echo "  ${3}-amd64.sha256 = \"$(nix hash file "${dir}/${3}-amd64.tar")\";"
    echo "  ${3}-arm64.arch = \"arm64\";"
    echo "  ${3}-arm64.finalImageName = \"${1}\";"
    echo "  ${3}-arm64.finalImageTag = \"${2}\";"
    echo "  ${3}-arm64.imageDigest = \"${arm64}\";"
    echo "  ${3}-arm64.imageName = \"${1}\";"
    echo "  ${3}-arm64.sha256 = \"$(nix hash file "${dir}/${3}-arm64.tar")\";"

    rm -rf "${dir}"
}

cat >"$(git rev-parse --show-toplevel)/nix/images/default.nix" <<EOF
{
$(update "debian" "12-slim" "debian")
$(update "cgr.dev/chainguard/wolfi-base" "latest" "wolfi")
}
EOF

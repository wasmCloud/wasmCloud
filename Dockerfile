FROM cgr.dev/chainguard/wolfi-base:latest AS base

FROM base AS base-amd64
ARG BIN_AMD64
ARG BIN=$BIN_AMD64

FROM base AS base-arm64
ARG BIN_ARM64
ARG BIN=$BIN_ARM64

FROM base-$TARGETARCH

ARG BIN_NAME=wasmcloud

# Copy application binary from disk
COPY ${BIN} /usr/local/bin/${BIN_NAME}

# Create a fixed symlink so ENTRYPOINT can use exec form without variable expansion.
# Docker's exec form [".."] doesn't expand variables, so we use a consistent path.
# Note: argv[0] will be "/usr/local/bin/app" instead of the actual binary name.
RUN ln -sf /usr/local/bin/${BIN_NAME} /usr/local/bin/app

ENTRYPOINT ["/usr/local/bin/app"]

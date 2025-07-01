FROM cgr.dev/chainguard/wolfi-base:latest AS base

FROM base AS base-amd64
ARG BIN_AMD64
ARG BIN=$BIN_AMD64

FROM base AS base-arm64
ARG BIN_ARM64
ARG BIN=$BIN_ARM64

FROM base-$TARGETARCH

# Copy application binary from disk
COPY ${BIN} /usr/local/bin/wasmcloud

# Run the application
ENTRYPOINT ["/usr/local/bin/wasmcloud"]

FROM debian:bullseye-slim as base

RUN DEBIAN_FRONTEND=noninteractive apt-get update && apt-get install -y ca-certificates

FROM base AS base-amd64
ARG BIN_AMD64
ARG BIN=$BIN_AMD64

FROM base AS base-arm64
ARG BIN_ARM64
ARG BIN=$BIN_ARM64

FROM base-$TARGETARCH

ARG USERNAME=wash
ARG USER_UID=1000
ARG USER_GID=$USER_UID

RUN addgroup --gid $USER_GID $USERNAME \
    && adduser --disabled-login -u $USER_UID --ingroup $USERNAME $USERNAME

# Copy application binary from disk
COPY --chown=$USERNAME --chmod=755 ${BIN} /usr/local/bin/wash

USER $USERNAME

# Run the application
ENTRYPOINT ["/usr/local/bin/wash"]

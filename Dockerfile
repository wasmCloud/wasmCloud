# syntax=docker/dockerfile:1-labs

FROM cgr.dev/chainguard/rust:latest-dev AS builder
WORKDIR /src
ENV RUST_BACKTRACE=1

# tools
USER root
RUN apk --no-cache add protoc protobuf protobuf-dev
USER nonroot

# copy source code
COPY --chown=nonroot:nonroot . .

# Optional comma-separated cargo feature list (e.g. "wasip3,wasi-tls").
# Empty by default so the standard image stays on WASI Preview 2.
ARG CARGO_FEATURES=""

# build static binary
RUN cargo build --release --bin wash ${CARGO_FEATURES:+--features ${CARGO_FEATURES}}

# Release image
FROM cgr.dev/chainguard/wolfi-base
RUN apk add --no-cache git
COPY --from=builder /src/target/release/wash /usr/local/bin/wash
ENTRYPOINT ["/usr/local/bin/wash"]

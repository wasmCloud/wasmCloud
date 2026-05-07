# syntax=docker/dockerfile:1-labs

FROM lukemathwalker/cargo-chef:latest-rust-1.91.0-alpine3.22 AS chef
USER root
WORKDIR /src

FROM chef AS planner
COPY --exclude=rust-toolchain.toml . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /src/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json
COPY --exclude=rust-toolchain.toml . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --target x86_64-unknown-linux-musl -p wash-precompile --bin wash-precompile && \
    cp target/x86_64-unknown-linux-musl/release/wash-precompile /tmp/wash-precompile

# Release image — minimal base. wash-precompile is a one-shot worker so we
# need only the binary; no shell, no package manager.
FROM cgr.dev/chainguard/wolfi-base
COPY --from=builder /tmp/wash-precompile /usr/local/bin/wash-precompile
ENTRYPOINT ["/usr/local/bin/wash-precompile"]

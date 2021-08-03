FROM rust:slim-buster as builder
WORKDIR /build
RUN apt-get update && apt-get install -y \
    ca-certificates \
    clang \
    libclang-dev \
    libssl-dev \
    llvm-dev \
    pkg-config \
    ;
COPY Cargo.toml .
COPY Cargo.lock .
COPY ./src ./src
COPY ./crates ./crates
COPY ./samples ./samples
ENV RUSTFLAGS=-Ctarget-feature=-crt-static
RUN cargo build --release

FROM debian:buster-slim as final
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl-dev && \
    rm -rf /var/lib/apt/lists/* \
    ;
COPY --from=builder /build/target/release/wasmcloud /usr/local/bin/wasmcloud
RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid "10001" \
    "user" \
    ;
USER user:user

ENTRYPOINT ["/usr/local/bin/wasmcloud"]
CMD ["-V"]

# Sometimes you want to be able to debug the cluster connectivity,
# or the http/redis capability providers. Build and upload this target with:
#
#   docker build --target debug -t wasmcloud/wasmcloud:0.18.2-debug .
#   docker push wasmcloud/wasmcloud:0.18.2-debug
#
FROM final as debug
USER root
RUN apt-get update && apt-get install -y \
    curl \
    redis && \
    curl -sSL https://github.com/nats-io/natscli/releases/download/0.0.25/nats-0.0.25-amd64.deb -o nats-0.0.25-amd64.deb && \
    dpkg -i nats-0.0.25-amd64.deb && \
    rm -rf nats-0.0.25-amd64.deb /var/lib/apt/lists/* \
    ;
USER user:user

# Make docker build use the `final` target by default.
FROM final

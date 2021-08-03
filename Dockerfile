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

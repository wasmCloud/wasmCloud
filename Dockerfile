FROM rust:1.49 as builder

WORKDIR /build

COPY Cargo.toml .
COPY Cargo.lock .
COPY ./src ./src

RUN apt update -y && apt install clang libssl-dev -y
RUN cargo build --release

FROM rust:1.49-slim-buster

COPY --from=builder /build/target/release/wash /usr/local/bin

ENTRYPOINT ["/usr/local/bin/wash"]

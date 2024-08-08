# First stage: Build the binary
FROM rust:1-bullseye AS builder

WORKDIR /app

# Copy the necessary parts of the workspace since .dockerignore isn't being respected
COPY Cargo.toml Cargo.toml
COPY Cargo.lock Cargo.lock
COPY ./src ./src
COPY ./crates ./crates
COPY ./tests ./tests

# Build the binary
WORKDIR crates/secrets-nats-kv
RUN cargo build --release

# Second stage: Create a minimal container with the binary
FROM debian:bullseye-slim

WORKDIR /app

# Copy the binary from the first stage
COPY --from=builder /app/target/release/secrets-nats-kv .

# Set the entrypoint
ENTRYPOINT ["./secrets-nats-kv"]
CMD ["run"]

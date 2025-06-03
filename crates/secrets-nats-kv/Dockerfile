# First stage: Build the binary
FROM rust:1-bullseye@sha256:821c41ed8d70347b32c2a3e4779fa0f05354773adf627aa388d0904494e0d21f AS builder

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
FROM debian:bullseye-slim@sha256:60a596681410bd31a48e5975806a24cd78328f3fd6b9ee5bc64dca6d46a51f29

WORKDIR /app

# Install CA certificates
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*

# Copy the binary from the first stage
COPY --from=builder /app/target/release/secrets-nats-kv .

# Set the entrypoint
ENTRYPOINT ["./secrets-nats-kv"]
CMD ["run"]

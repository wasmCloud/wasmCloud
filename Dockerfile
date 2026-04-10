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

# build static binary
RUN cargo build --release --bin wash

# Release image
FROM cgr.dev/chainguard/wolfi-base
RUN apk add --no-cache git
COPY --from=builder /src/target/release/wash /usr/local/bin/wash
ENTRYPOINT ["/usr/local/bin/wash"]

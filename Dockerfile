# syntax=docker/dockerfile:1-labs

FROM cgr.dev/chainguard/rust:latest-dev AS builder
WORKDIR /src
ENV RUST_BACKTRACE=1

# tools
USER root
RUN attempt=1; delay=2; \
    until apk update && apk add --no-cache protoc protobuf protobuf-dev; do \
        if [ "$attempt" -ge 5 ]; then \
            echo "APK installation failed after $attempt attempts" >&2; \
            exit 1; \
        fi; \
        echo "APK installation attempt $attempt failed; retrying in ${delay}s" >&2; \
        sleep "$delay"; \
        attempt=$((attempt + 1)); \
        delay=$((delay * 2)); \
    done
USER nonroot

# copy source code
COPY --chown=nonroot:nonroot . .

# Optional comma-separated cargo feature list for opt-in extras (e.g.
# "wasi-tls", "wasi-webgpu"). WASI Preview 3 is already compiled into the
# default wash build, so it needs no feature flag here.
ARG CARGO_FEATURES=""

# build static binary
RUN cargo build --release --bin wash ${CARGO_FEATURES:+--features ${CARGO_FEATURES}}

# Release image
FROM cgr.dev/chainguard/wolfi-base
RUN attempt=1; delay=2; \
    until apk update && apk add --no-cache git; do \
        if [ "$attempt" -ge 5 ]; then \
            echo "APK installation failed after $attempt attempts" >&2; \
            exit 1; \
        fi; \
        echo "APK installation attempt $attempt failed; retrying in ${delay}s" >&2; \
        sleep "$delay"; \
        attempt=$((attempt + 1)); \
        delay=$((delay * 2)); \
    done
COPY --from=builder /src/target/release/wash /usr/local/bin/wash

# Smoke test the binary against THIS stage's libc. The builder and the runtime
# base are independently-rolling `:latest` Chainguard images, so they can sit on
# different glibc majors for a window (e.g. rust:latest-dev on 2.44 while
# wolfi-base is still on 2.43, which Wolfi ships as separate, mutually
# conflicting `glibc-2.43`/`glibc-2.44` packages — apk cannot reconcile them
# here). Without this the image builds green and only fails much later, as an
# unreadable `libm.so.6: version GLIBC_x.y not found` CrashLoopBackOff in the
# operator e2e cluster. Fail here instead, where the error points at the cause.
RUN ["/usr/local/bin/wash", "--version"]

ENTRYPOINT ["/usr/local/bin/wash"]

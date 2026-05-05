# wash-precompile

Standalone worker with a frozen two-flag CLI:

```
wash-precompile --image <oci-ref> --output <scheme-url>
```

Pulls a wasm component from an OCI registry, precompiles it via `wasmtime::Engine::precompile_component`, and writes the resulting `.cwasm` bytes to the URL given by `--output` (`nats://` for production, `file://` for tests and local dev).

The operator-side `PrecompileReconciler` constructs Jobs that invoke this binary.

## Build the binary

```bash
cargo build -p wash-precompile --release
```

The release binary lands at `target/release/wash-precompile`.

## Build the container image

The Dockerfile builds a static musl binary with `cargo-chef` and packages it into a `chainguard/wolfi-base` image. Build context is the workspace root:

```bash
docker build -f crates/wash-precompile/Dockerfile -t wash-precompile:dev .
```

Override the tag for pushing to a registry:

```bash
docker build -f crates/wash-precompile/Dockerfile \
    -t ghcr.io/your-org/wash-precompile:v0.1.0 .
```

Smoke-test the image:

```bash
docker run --rm wash-precompile:dev --help
```

Should print clap's usage screen with `--image` and `--output` and exit 0.

## Run locally against a `file://` output

```bash
cargo run -p wash-precompile -- \
    --image ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0 \
    --output file:///tmp/out.cwasm
```

For `nats://`, set `NATS_URL`. For pulling from a private registry, set `DOCKER_CONFIG` pointing at a directory containing a `config.json` (the same format `kubectl create secret docker-registry` produces).

## Tests

```bash
# Unit tests
cargo test -p wash-precompile

# Integration tests (requires Docker for testcontainers NATS)
cargo test -p wash-precompile -- --ignored
```

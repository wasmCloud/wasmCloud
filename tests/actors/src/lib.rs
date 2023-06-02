pub const RUST_ECHO_MODULE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/actor-rust-echo-module.wasm"));

pub const RUST_HTTP_LOG_RNG_COMPAT: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/actor-rust-http-log-rng-compat.wasm"
));

pub const RUST_HTTP_LOG_RNG_COMPONENT: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/actor-rust-http-log-rng-component.wasm"
));

pub const RUST_HTTP_LOG_RNG_MODULE: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/actor-rust-http-log-rng-module.wasm"
));

pub fn encode_component(module: &[u8], wasi: bool) -> anyhow::Result<Vec<u8>> {
    use anyhow::Context;

    let encoder = wit_component::ComponentEncoder::default()
        .validate(true)
        .module(module)
        .context("failed to set core component module")?;
    let encoder = if wasi {
        encoder
            .adapter(
                "wasi_snapshot_preview1",
                include_bytes!(concat!(
                    env!("OUT_DIR"),
                    "/wasi-preview1-component-adapter.wasm"
                )),
            )
            .context("failed to add WASI adapter")?
    } else {
        encoder
    };
    encoder.encode().context("failed to encode a component")
}

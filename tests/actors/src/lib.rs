pub const RUST_ECHO_MODULE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/actor-rust-echo-module.wasm"));

pub const RUST_FOOBAR_COMPONENT: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/actor-rust-foobar-component.wasm"
));

pub const RUST_FOOBAR_GUEST_COMPONENT: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/actor-rust-foobar-guest-component.wasm"
));

pub const RUST_FOOBAR_HOST_COMPONENT: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/actor-rust-foobar-host-component.wasm"
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
                include_bytes!(concat!(env!("OUT_DIR"), "/wasi-snapshot-preview1.wasm")),
            )
            .context("failed to add WASI adapter")?
    } else {
        encoder
    };
    encoder.encode().context("failed to encode a component")
}

use anyhow::Context;
use wit_component::ComponentEncoder;

pub const RUST_ECHO_MODULE: &[u8] =
    include_bytes!(env!("CARGO_CDYLIB_FILE_ACTOR_ECHO_MODULE"));

pub const RUST_FOOBAR_COMPONENT: &[u8] =
    include_bytes!(env!("CARGO_CDYLIB_FILE_ACTOR_FOOBAR_COMPONENT"));

pub const RUST_FOOBAR_GUEST_COMPONENT: &[u8] =
    include_bytes!(env!("CARGO_CDYLIB_FILE_ACTOR_FOOBAR_GUEST_COMPONENT"));

pub const RUST_FOOBAR_HOST_COMPONENT: &[u8] =
    include_bytes!(env!("CARGO_CDYLIB_FILE_ACTOR_FOOBAR_HOST_COMPONENT"));

pub const RUST_HTTP_LOG_RNG_COMPONENT: &[u8] =
    include_bytes!(env!("CARGO_CDYLIB_FILE_ACTOR_HTTP_LOG_RNG_COMPONENT"));

pub const RUST_HTTP_LOG_RNG_MODULE: &[u8] =
    include_bytes!(env!("CARGO_CDYLIB_FILE_ACTOR_HTTP_LOG_RNG_MODULE"));

pub fn encode_component(module: &[u8], wasi: bool) -> anyhow::Result<Vec<u8>> {
    let encoder = ComponentEncoder::default()
        .validate(true)
        .module(module)
        .context("failed to set core component module")?;
    let encoder = if wasi {
        encoder
            .adapter(
                "wasi_snapshot_preview1",
                include_bytes!(env!("CARGO_CDYLIB_FILE_WASI_SNAPSHOT_PREVIEW1")),
            )
            .context("failed to add WASI adapter")?
    } else {
        encoder
    };
    encoder.encode().context("failed to encode a component")
}

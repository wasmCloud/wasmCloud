pub const ISSUER: &str = env!("ISSUER");

pub const RUST_BUILTINS_COMPONENT_REACTOR: &str =
    concat!(env!("OUT_DIR"), "/rust-builtins-component-reactor.wasm");
pub const RUST_BUILTINS_COMPONENT_REACTOR_SIGNED: &str = concat!(
    env!("OUT_DIR"),
    "/rust-builtins-component-reactor.signed.wasm"
);

pub const RUST_BUILTINS_COMPONENT_REACTOR_PREVIEW2: &str = concat!(
    env!("OUT_DIR"),
    "/rust-builtins-component-reactor-preview2.wasm"
);
pub const RUST_BUILTINS_COMPONENT_REACTOR_PREVIEW2_SIGNED: &str = concat!(
    env!("OUT_DIR"),
    "/rust-builtins-component-reactor-preview2.signed.wasm"
);

pub const RUST_BUILTINS_MODULE_REACTOR: &str =
    concat!(env!("OUT_DIR"), "/rust-builtins-module-reactor.wasm");
pub const RUST_BUILTINS_MODULE_REACTOR_SIGNED: &str =
    concat!(env!("OUT_DIR"), "/rust-builtins-module-reactor.signed.wasm");

pub const RUST_FOOBAR_COMPONENT_COMMAND: &str =
    concat!(env!("OUT_DIR"), "/rust-foobar-component-command.wasm");
pub const RUST_FOOBAR_COMPONENT_COMMAND_SIGNED: &str = concat!(
    env!("OUT_DIR"),
    "/rust-foobar-component-command.signed.wasm"
);

pub const RUST_FOOBAR_COMPONENT_COMMAND_PREVIEW2: &str = concat!(
    env!("OUT_DIR"),
    "/rust-foobar-component-command-preview2.wasm"
);
pub const RUST_FOOBAR_COMPONENT_COMMAND_PREVIEW2_SIGNED: &str = concat!(
    env!("OUT_DIR"),
    "/rust-foobar-component-command-preview2.signed.wasm"
);

pub const RUST_LOGGING_MODULE_COMMAND: &str =
    concat!(env!("OUT_DIR"), "/rust-logging-module-command.wasm");
pub const RUST_LOGGING_MODULE_COMMAND_SIGNED: &str =
    concat!(env!("OUT_DIR"), "/rust-logging-module-command.signed.wasm");

pub const RUST_KV_HTTP_SMITHY: &str = concat!(env!("OUT_DIR"), "/rust-kv-http-smithy.wasm");
pub const RUST_KV_HTTP_SMITHY_SIGNED: &str =
    concat!(env!("OUT_DIR"), "/rust-kv-http-smithy.signed.wasm");

pub const RUST_BLOBSTORE_HTTP_SMITHY: &str =
    concat!(env!("OUT_DIR"), "/rust-blobstore-http-smithy.wasm");
pub const RUST_BLOBSTORE_HTTP_SMITHY_SIGNED: &str =
    concat!(env!("OUT_DIR"), "/rust-blobstore-http-smithy.signed.wasm");

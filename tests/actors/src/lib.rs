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

pub const RUST_KV_HTTP_SMITHY: &str = concat!(env!("OUT_DIR"), "/rust-kv-http-smithy.wasm");
pub const RUST_KV_HTTP_SMITHY_SIGNED: &str =
    concat!(env!("OUT_DIR"), "/rust-kv-http-smithy.signed.wasm");

pub const RUST_BLOBSTORE_HTTP_SMITHY: &str =
    concat!(env!("OUT_DIR"), "/rust-blobstore-http-smithy.wasm");
pub const RUST_BLOBSTORE_HTTP_SMITHY_SIGNED: &str =
    concat!(env!("OUT_DIR"), "/rust-blobstore-http-smithy.signed.wasm");

pub const RUST_LATTICE_CONTROL_HTTP_SMITHY: &str =
    concat!(env!("OUT_DIR"), "/rust-lattice-control-http-smithy.wasm");
pub const RUST_LATTICE_CONTROL_HTTP_SMITHY_SIGNED: &str = concat!(
    env!("OUT_DIR"),
    "/rust-lattice-control-http-smithy.signed.wasm"
);

pub const RUST_MESSAGING_SENDER_HTTP_SMITHY: &str =
    concat!(env!("OUT_DIR"), "/rust-messaging-sender-http-smithy.wasm");
pub const RUST_MESSAGING_SENDER_HTTP_SMITHY_SIGNED: &str = concat!(
    env!("OUT_DIR"),
    "/rust-messaging-sender-http-smithy.signed.wasm"
);

pub const RUST_MESSAGING_RECEIVER_SMITHY: &str =
    concat!(env!("OUT_DIR"), "/rust-messaging-receiver-smithy.wasm");
pub const RUST_MESSAGING_RECEIVER_SMITHY_SIGNED: &str = concat!(
    env!("OUT_DIR"),
    "/rust-messaging-receiver-smithy.signed.wasm"
);

pub const RUST_WRPC_PINGER_COMPONENT: &str = concat!(
    env!("OUT_DIR"),
    "/rust-wrpc-pinger-component-preview2.signed.wasm"
);
pub const RUST_WRPC_PONGER_COMPONENT: &str = concat!(
    env!("OUT_DIR"),
    "/rust-wrpc-ponger-component-preview2.signed.wasm"
);

//! Wasmcloud Weld runtime library
//!
//! This crate provides code generation and runtime support for wasmcloud rpc messages
//! used by [wasmcloud](https://wasmcloud.dev) actors and capability providers.
//!

mod actor_wasm;
mod common;
pub use common::{
    client, context, deserialize, serialize, Message, MessageDispatch, RpcError, Transport,
};
mod timestamp;
pub use timestamp::Timestamp;
pub mod provider;
mod wasmbus_core;
pub mod core {
    // re-export core lib as "core"
    pub use crate::wasmbus_core::*;
}
mod wasmbus_model;
pub mod model {
    // re-export core lib as "core"
    pub use crate::wasmbus_model::*;
}

/// Version number of this api
#[doc(hidden)]
pub const WASMBUS_RPC_VERSION: u32 = 0;

/// import module for webassembly linking
#[doc(hidden)]
pub const WASMBUS_RPC_IMPORT_NAME: &str = "wapc";

/// This crate's published version
pub const WELD_CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub type CallResult = std::result::Result<Vec<u8>, Box<dyn std::error::Error + Sync + Send>>;
pub type HandlerResult<T> = std::result::Result<T, Box<dyn std::error::Error + Sync + Send>>;
pub type TomlMap = toml::value::Map<String, toml::value::Value>;

pub mod actor {

    pub mod prelude {
        pub use crate::common::{client, context, Message, MessageDispatch, RpcError};

        // re-export async_trait
        pub use async_trait::async_trait;
        pub use wasmbus_macros::Actor;

        #[cfg(feature = "BigInteger")]
        pub use num_bigint::BigInt as BigInteger;

        #[cfg(feature = "BigDecimal")]
        pub use bigdecimal::BigDecimal;

        cfg_if::cfg_if! {

            if #[cfg(target_arch = "wasm32")] {
                pub use crate::actor_wasm::{console_log, WasmHost};
            } else {
                // this code is non-functional, since actors only run in wasm32,
                // but it reduces compiler errors if you are building a cargo multi-project workspace for non-wasm32
                #[derive(Clone, Debug, Default)]
                pub struct WasmHost {}

                #[async_trait]
                impl crate::Transport for WasmHost {
                    async fn send(&self, _: &context::Context<'_>, _: &client::SendConfig,
                                _: Message<'_>, ) -> std::result::Result<Message<'_>, RpcError> {
                       unimplemented!();
                    }
                }

                pub fn console_log(_s: &str) {}
            }
        }
    }
}

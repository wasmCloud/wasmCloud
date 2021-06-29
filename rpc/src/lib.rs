//! Wasmcloud Weld runtime library
//!
//! This crate provides code generation and runtime support for wasmcloud rpc messages
//! used by [wasmcloud](https://wasmcloud.dev) actors and capability providers.
//!

mod common;
pub use common::{
    client, context, deserialize, serialize, Message, MessageDispatch, RpcError, Transport,
};
pub mod core; // export auto-generated items from smithy model
mod timestamp;
pub use timestamp::Timestamp;

mod actor_wasm;

/// Version number of this api
#[doc(hidden)]
pub const WELD_RPC_VERSION: u32 = 0; // api version 0 is binary compatible with wapc

/// This crate's published version
pub const WELD_CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub type CallResult = std::result::Result<Vec<u8>, Box<dyn std::error::Error + Sync + Send>>;
pub type HandlerResult<T> = std::result::Result<T, Box<dyn std::error::Error + Sync + Send>>;
pub type TomlMap = toml::value::Map<String, toml::value::Value>;

pub mod actor {

    pub mod prelude {
        pub use crate::common::{client, context, Message, MessageDispatch, RpcError};

        #[cfg(target_arch = "wasm32")]
        pub use crate::actor_wasm::{console_log, WasmHost};

        //pub use crate::core::{Actor, ActorReceiver, ActorSender};
        // re-export async_trait
        pub use async_trait::async_trait;
        pub use wasmcloud_weld_macros::Actor;
        //pub use crate::Timestamp;
    }
}

pub mod provider {

    pub mod prelude {
        pub use crate::{client, context, Message, MessageDispatch, RpcError};
        //pub use crate::Timestamp;
        pub use async_trait::async_trait;
        pub use wasmcloud_weld_macros::Provider;

        #[cfg(feature = "BigInteger")]
        pub use num_bigint::BigInt as BigInteger;

        #[cfg(feature = "BigDecimal")]
        pub use bigdecimal::BigDecimal;
    }
}

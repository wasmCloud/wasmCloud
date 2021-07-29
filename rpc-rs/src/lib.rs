//! Wasmcloud Weld runtime library
//!
//! This crate provides code generation and runtime support for wasmcloud rpc messages
//! used by [wasmcloud](https://wasmcloud.dev) actors and capability providers.
//!
//#![feature(toowned_clone_into)]

use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value as JsonValue;

mod timestamp;
pub use timestamp::Timestamp;

mod actor_wasm;
mod common;
pub use common::{
    client, context, deserialize, serialize, Message, MessageDispatch, RpcError, Transport,
};
pub mod channel_log;
pub mod provider;
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

mod wasmbus_core;
pub mod core {
    // re-export core lib as "core"
    pub use crate::wasmbus_core::*;

    cfg_if::cfg_if! {
        if #[cfg(not(target_arch = "wasm32"))] {

            // allow testing provider outside host
            const TEST_HARNESS: &str = "_TEST_";

            /// how often we will ping nats server for keep-alive
            const NATS_PING_INTERVAL_SEC: u16 = 15;

            /// number of unsuccessful pings before connection is deemed disconnected
            const NATS_PING_FAIL_COUNT: u16 = 8;

            // TODO: is this milliseconds? - units not documented
            /// time between connection retries
            const NATS_RECONNECT_INTERVAL: u64 = 15;

            impl HostData {
                /// returns whether the provider is running under test
                pub fn is_test(&self) -> bool {
                    self.host_id == TEST_HARNESS
                }

                /// obtain NatsClientOptions pre-populated with connection data from the host.
                pub fn nats_options(&self) -> ratsio::NatsClientOptions {
                    ratsio::NatsClientOptions {
                        ping_interval: NATS_PING_INTERVAL_SEC,
                        ping_max_out: NATS_PING_FAIL_COUNT,
                        reconnect_timeout: NATS_RECONNECT_INTERVAL,
                        // if connect fails, keep trying, forever
                        ensure_connect: true,
                        // need to test whether this works
                        subscribe_on_reconnect: true,
                        cluster_uris: if self.lattice_rpc_url.is_empty() {
                            Vec::new()
                        } else {
                            vec![self.lattice_rpc_url.clone()]
                        }
                        .into(),
                        ..Default::default()
                    }
                }
            }
        }
    }

    /// url scheme for wasmbus protocol messages
    pub const URL_SCHEME: &str = "wasmbus";

    impl std::fmt::Display for WasmCloudEntity {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.url())
        }
    }

    impl WasmCloudEntity {
        /// constructor for actor entity
        pub fn new_actor(id: &str) -> WasmCloudEntity {
            WasmCloudEntity {
                public_key: id.to_string(),
                contract_id: "".into(),
                link_name: "".into(),
            }
        }

        /// constructor for capability provider entity
        pub fn new_provider(id: &str, contract_id: &str, link_name: &str) -> WasmCloudEntity {
            WasmCloudEntity {
                public_key: id.into(),
                contract_id: contract_id.into(),
                link_name: link_name.into(),
            }
        }

        /// Returns URL of the entity
        pub fn url(&self) -> String {
            if self.public_key.to_uppercase().starts_with('M') {
                format!("{}://{}", crate::core::URL_SCHEME, self.public_key)
            } else {
                format!(
                    "{}://{}/{}/{}",
                    URL_SCHEME,
                    self.contract_id
                        .replace(":", "/")
                        .replace(" ", "_")
                        .to_lowercase(),
                    self.link_name.replace(" ", "_").to_lowercase(),
                    self.public_key
                )
            }
        }

        /// Returns the unique (public) key of the entity
        pub fn key(&self) -> String {
            self.public_key.to_string()
        }
    }
}

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

/// convert json args to msgpack
pub fn json_to_args<T>(v: JsonValue) -> Result<Vec<u8>, RpcError>
where
    T: Serialize,
    T: DeserializeOwned,
{
    serialize(
        &serde_json::from_value::<T>(v)
            .map_err(|e| RpcError::Deser(format!("invalid params: {}.", e)))?,
    )
    /*
    let val: T = serde_json::from_value(v)
        .map_err(|e| RpcError::Deser(format!("invalid params: {}.", e)))?;
    let blob = serialize(&val)?;
    Ok(blob)
         */
}

/// convert message response to json
pub fn response_to_json<T>(msg: &[u8]) -> Result<JsonValue, RpcError>
where
    T: Serialize,
    T: DeserializeOwned,
{
    serde_json::to_value(deserialize::<T>(msg)?)
        .map_err(|e| RpcError::Ser(format!("response serialization : {}.", e)))

    /*
    let val: T = deserialize(msg)?;
    let j = serde_json::to_value(val)
        .map_err(|e| RpcError::Ser(format!("response serialization : {}.", e)))?;
    Ok(j)
     */
}

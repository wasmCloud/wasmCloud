//! Wasmcloud Weld runtime library
//!
//! This crate provides code generation and runtime support for wasmcloud rpc messages
//! used by [wasmcloud](https://wasmcloud.dev) actors and capability providers.
//!

mod timestamp;
// re-export Timestamp
pub use timestamp::Timestamp;

mod actor_wasm;
pub mod channel_log;
pub mod common;
pub mod provider;
pub(crate) mod provider_main;
mod wasmbus_model;
pub mod model {
    // re-export model lib as "model"
    pub use crate::wasmbus_model::*;
}
pub mod cbor;
pub mod error;

#[deprecated(
    since = "0.7.0-alpha.2",
    note = "use wasmbus_rpc::common::deserialize instead of wasmbus_rpc::deseerialize"
)]
pub use common::deserialize;
#[deprecated(
    since = "0.7.0-alpha.2",
    note = "use wasmbus_rpc::common::serialize instead of wasmbus_rpc::serialize"
)]
pub use common::serialize;
#[deprecated(
    since = "0.7.0-alpha.2",
    note = "use wasmbus_rpc::common::Context instead of wasmbus_rpc::Context"
)]
pub use common::Context;
#[deprecated(
    since = "0.7.0-alpha.2",
    note = "use wasmbus_rpc::common::Message instead of wasmbus_rpc::Message"
)]
pub use common::Message;
#[deprecated(
    since = "0.7.0-alpha.2",
    note = "use wasmbus_rpc::common::SendOpts instead of wasmbus_rpc::SendOpts"
)]
pub use common::SendOpts;
#[deprecated(
    since = "0.7.0-alpha.2",
    note = "use wasmbus_rpc::common::Transport instead of wasmbus_rpc::Transport"
)]
pub use common::Transport;
#[deprecated(
    since = "0.7.0-alpha.2",
    note = "use wasmbus_rpc::error::RpcError instead of wasmbus_rpc::RpcError"
)]
pub use error::RpcError;
#[deprecated(
    since = "0.7.0-alpha.2",
    note = "use wasmbus_rpc::error::RpcResult instead of wasmbus_rpc::RpcResult"
)]
pub use error::RpcResult;

// re-export nats-aflowt
#[cfg(not(target_arch = "wasm32"))]
pub use nats_aflowt as anats;

/// This will be removed in a later version - use cbor instead to avoid dependence on minicbor crate
/// @deprecated
pub use minicbor;

#[cfg(not(target_arch = "wasm32"))]
pub mod rpc_client;

/// import module for webassembly linking
#[doc(hidden)]
pub const WASMBUS_RPC_IMPORT_NAME: &str = "wasmbus";

/// Version number of this api
#[doc(hidden)]
pub const WASMBUS_RPC_VERSION: u32 = 0;

/// This crate's published version
pub const WELD_CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub type CallResult = std::result::Result<Vec<u8>, Box<dyn std::error::Error + Sync + Send>>;
pub type HandlerResult<T> = std::result::Result<T, Box<dyn std::error::Error + Sync + Send>>;
pub type TomlMap = toml::value::Map<String, toml::value::Value>;

mod wasmbus_core;
pub mod core {
    // re-export core lib as "core"
    use crate::error::{RpcError, RpcResult};
    pub use crate::wasmbus_core::*;
    use std::convert::TryFrom;

    cfg_if::cfg_if! {
        if #[cfg(not(target_arch = "wasm32"))] {

            // allow testing provider outside host
            const TEST_HARNESS: &str = "_TEST_";
            // fallback nats address if host doesn't pass one to provider
            const DEFAULT_NATS_ADDR: &str = "nats://127.0.0.1:4222";

            impl HostData {
                /// returns whether the provider is running under test
                pub fn is_test(&self) -> bool {
                    self.host_id == TEST_HARNESS
                }

                /// Connect to nats using options provided by host
                pub async fn nats_connect(&self) -> RpcResult<crate::anats::Connection> {
                    use std::str::FromStr as _;
                    let nats_addr = if !self.lattice_rpc_url.is_empty() {
                        self.lattice_rpc_url.as_str()
                    } else {
                        DEFAULT_NATS_ADDR
                    };
                    let nats_server = nats_aflowt::ServerAddress::from_str(nats_addr).map_err(|e| {
                        RpcError::InvalidParameter(format!("Invalid nats server url '{}': {}", nats_addr, e))
                    })?;

                    // Connect to nats
                    let nc = nats_aflowt::Options::default()
                        .max_reconnects(None)
                        .connect(vec![nats_server])
                        .await
                        .map_err(|e| {
                            RpcError::ProviderInit(format!("nats connection to {} failed: {}", nats_addr, e))
                        })?;
                    Ok(nc)
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

    impl LinkDefinition {
        pub fn actor_entity(&self) -> WasmCloudEntity {
            WasmCloudEntity {
                public_key: self.actor_id.clone(),
                contract_id: String::default(),
                link_name: String::default(),
            }
        }

        pub fn provider_entity(&self) -> WasmCloudEntity {
            WasmCloudEntity {
                public_key: self.provider_id.clone(),
                contract_id: self.contract_id.clone(),
                link_name: self.link_name.clone(),
            }
        }
    }

    impl WasmCloudEntity {
        /// constructor for actor entity
        pub fn new_actor<T: ToString>(public_key: T) -> RpcResult<WasmCloudEntity> {
            let public_key = public_key.to_string();
            if public_key.is_empty() {
                return Err(RpcError::InvalidParameter(
                    "public_key may not be empty".to_string(),
                ));
            }
            Ok(WasmCloudEntity {
                public_key,
                contract_id: String::new(),
                link_name: String::new(),
            })
        }

        /*
        /// create provider entity from link definition
        pub fn from_link(link: &LinkDefinition) -> Self {
            WasmCloudEntity {
                public_key: link.provider_id.clone(),
                contract_id: link.contract_id.clone(),
                link_name: link.link_name.clone(),
            }
        }
         */

        /// constructor for capability provider entity
        /// all parameters are required
        pub fn new_provider<T1: ToString, T2: ToString>(
            contract_id: T1,
            link_name: T2,
        ) -> RpcResult<WasmCloudEntity> {
            let contract_id = contract_id.to_string();
            if contract_id.is_empty() {
                return Err(RpcError::InvalidParameter(
                    "contract_id may not be empty".to_string(),
                ));
            }
            let link_name = link_name.to_string();
            if link_name.is_empty() {
                return Err(RpcError::InvalidParameter(
                    "link_name may not be empty".to_string(),
                ));
            }
            Ok(WasmCloudEntity {
                public_key: "".to_string(),
                contract_id,
                link_name,
            })
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
                        .replace(':', "/")
                        .replace(' ', "_")
                        .to_lowercase(),
                    self.link_name.replace(' ', "_").to_lowercase(),
                    self.public_key
                )
            }
        }

        /// Returns the unique (public) key of the entity
        pub fn public_key(&self) -> String {
            self.public_key.to_string()
        }

        /// returns true if this entity refers to an actor
        pub fn is_actor(&self) -> bool {
            self.link_name.is_empty() || self.contract_id.is_empty()
        }

        /// returns true if this entity refers to a provider
        pub fn is_provider(&self) -> bool {
            !self.is_actor()
        }
    }

    impl TryFrom<&str> for WasmCloudEntity {
        type Error = RpcError;

        /// converts string into actor entity
        fn try_from(target: &str) -> Result<WasmCloudEntity, Self::Error> {
            WasmCloudEntity::new_actor(target.to_string())
        }
    }

    impl TryFrom<String> for WasmCloudEntity {
        type Error = RpcError;

        /// converts string into actor entity
        fn try_from(target: String) -> Result<WasmCloudEntity, Self::Error> {
            WasmCloudEntity::new_actor(target)
        }
    }
}

pub mod actor {

    pub mod prelude {
        pub use crate::{
            common::{Context, Message, MessageDispatch, SendOpts, Transport},
            core::{Actor, ActorReceiver},
            error::{RpcError, RpcResult},
        };

        // re-export async_trait
        pub use async_trait::async_trait;
        // derive macros
        pub use wasmbus_macros::{Actor, ActorHealthResponder as HealthResponder};

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
                impl crate::common::Transport for WasmHost {
                    async fn send(&self, _ctx: &Context,
                                _msg: Message<'_>, _opts: Option<SendOpts> ) -> RpcResult<Vec<u8>> {
                       unimplemented!();
                    }
                    fn set_timeout(&self, _interval: std::time::Duration) {
                       unimplemented!();
                    }
                }

                pub fn console_log(_s: &str) {}
            }
        }
    }
}

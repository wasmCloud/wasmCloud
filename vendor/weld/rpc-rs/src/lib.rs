//! wasmcloud-rpc runtime library
//!
//! This crate provides code generation and runtime support for wasmcloud rpc messages
//! used by [wasmcloud](https://wasmcloud.dev) actors and capability providers.
//!

mod timestamp;
// re-export Timestamp
pub use timestamp::Timestamp;
// re-export wascap crate
#[cfg(not(target_arch = "wasm32"))]
pub use wascap;

// re-export async-nats. work-around for
// https://github.com/rust-lang/rust/issues/44663 and https://rust-lang.github.io/rfcs/1977-public-private-dependencies.html
// longer term: if public-private is not implemented,
// split out rpc-client to separate lib, and make interfaces build locally (as wit-bindgen does)
#[cfg(not(target_arch = "wasm32"))]
pub use async_nats;

#[cfg(all(not(target_arch = "wasm32"), feature = "otel"))]
#[macro_use]
pub mod otel;

mod actor_wasm;
pub mod cbor;
pub mod common;
pub(crate) mod document;
pub mod error;
pub mod provider;
pub(crate) mod provider_main;
mod wasmbus_model;

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

pub type CallResult = Result<Vec<u8>, Box<dyn std::error::Error + Sync + Send>>;
pub type HandlerResult<T> = Result<T, Box<dyn std::error::Error + Sync + Send>>;
pub type TomlMap = toml::value::Map<String, toml::value::Value>;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod chunkify;
mod wasmbus_core;

#[macro_use]

pub mod model {
    // re-export model lib as "model"
    pub use crate::wasmbus_model::*;
}

pub mod core {
    // re-export core lib as "core"
    use crate::error::{RpcError, RpcResult};
    pub use crate::wasmbus_core::*;
    use std::convert::TryFrom;

    cfg_if::cfg_if! {
        if #[cfg(not(target_arch = "wasm32"))] {

            // allow testing provider outside host
            const TEST_HARNESS: &str = "_TEST_";

            impl HostData {
                /// returns whether the provider is running under test
                pub fn is_test(&self) -> bool {
                    self.host_id == TEST_HARNESS
                }

                /// Connect to nats using options provided by host
                pub async fn nats_connect(&self) -> RpcResult<crate::async_nats::Client> {
                    use std::str::FromStr as _;
                    let nats_addr = if !self.lattice_rpc_url.is_empty() {
                        self.lattice_rpc_url.as_str()
                    } else {
                        crate::provider::DEFAULT_NATS_ADDR
                    };
                    let nats_server = crate::async_nats::ServerAddr::from_str(nats_addr).map_err(|e| {
                        RpcError::InvalidParameter(format!("Invalid nats server url '{}': {}", nats_addr, e))
                    })?;

                    // Connect to nats
                    let nc = crate::async_nats::ConnectOptions::default()
                        .connect(nats_server)
                        .await
                        .map_err(|e| {
                            RpcError::ProviderInit(format!("nats connection to {nats_addr} failed: {e}"))
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
                format!("{}://{}", URL_SCHEME, self.public_key)
            } else {
                format!(
                    "{}://{}/{}/{}",
                    URL_SCHEME,
                    self.contract_id.replace(':', "/").replace(' ', "_").to_lowercase(),
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
                pub use crate::actor_wasm::{console_log, host_call, WasmHost};
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

#[cfg(test)]
mod test {
    use anyhow::anyhow;

    fn ret_rpc_err(val: u8) -> Result<u8, crate::error::RpcError> {
        let x = match val {
            0 => Ok(0),
            10 | 11 => Err(crate::error::RpcError::Other(format!("rpc:{val}"))),
            _ => Ok(255),
        }?;
        Ok(x)
    }

    fn ret_any(val: u8) -> anyhow::Result<u8> {
        let x = match val {
            0 => Ok(0),
            20 | 21 => Err(anyhow!("any:{}", val)),
            _ => Ok(255),
        }?;
        Ok(x)
    }

    fn either(val: u8) -> anyhow::Result<u8> {
        let x = match val {
            0 => 0,
            10 | 11 => ret_rpc_err(val)?,
            20 | 21 => ret_any(val)?,
            _ => 255,
        };
        Ok(x)
    }

    #[test]
    fn values() {
        use crate::error::RpcError;

        let v0 = ret_rpc_err(0);
        assert_eq!(v0.ok().unwrap(), 0);

        let v10 = either(10);
        assert!(v10.is_err());
        assert_eq!(v10.as_ref().err().unwrap().to_string().as_str(), "rpc:10");
        if let Err(e) = &v10 {
            if let Some(rpc_err) = e.downcast_ref::<RpcError>() {
                eprintln!("10 is rpc error (ok)");
                match rpc_err {
                    RpcError::Other(s) => {
                        eprintln!("RpcError::Other({s})");
                    }
                    RpcError::Nats(s) => {
                        eprintln!("RpcError::Nats({s})");
                    }
                    _ => {
                        eprintln!("RpcError::unknown {rpc_err}");
                    }
                }
            } else {
                eprintln!("10 is not rpc error. value={e}");
            }
        }

        let v20 = either(20);
        assert!(v20.is_err());
        assert_eq!(v20.as_ref().err().unwrap().to_string().as_str(), "any:20");
        if let Err(e) = &v20 {
            if let Some(rpc_err) = e.downcast_ref::<RpcError>() {
                eprintln!("20 is rpc error (ok)");
                match rpc_err {
                    RpcError::Other(s) => {
                        eprintln!("RpcError::Other({s})");
                    }
                    RpcError::Nats(s) => {
                        eprintln!("RpcError::Nats({s})");
                    }
                    _ => {
                        eprintln!("RpcError::unknown {rpc_err}");
                    }
                }
            } else {
                eprintln!("20 is not rpc error. value={e}");
            }
        }
    }
}

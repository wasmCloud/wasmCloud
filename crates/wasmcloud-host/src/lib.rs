//! # wasmCloud Host
//!
//! [wasmCloud](https://wasmcloud.com) is a secure, distributed actor platform with an autonomous mesh network built
//! for bridging disparate and far-flung infrastructure.
//!
//! By default a wasmCloud host will start in offline mode and only allow "local" scheduling of actors
//! and capabilities. If you choose to opt-in to the lattice, you can use NATS as a message broker to
//! provide the infrastructure for wasmCloud's self-forming, self-healing network. If you then want
//! even more power, you can choose to override the capability provider used for managing the shared
//! state within a lattice.
//!
//! Local development on your workstation is easy and simple by default, and you should only
//! incur additional complexity as you move toward resilient, distributed production
//! environments.
//!
//! To start a runtime, simply add actors and capabilities to the host. For more information,
//! take a look at the documentation and tutorials at [wasmcloud.dev](https://wasmcloud.dev).
//!
//! # Example
//! The following example creates a new wasmCloud host in the default standalone (no lattice) mode. It
//! then loads an actor that simply echoes back incoming HTTP requests as outbound HTTP responses.
//! The HTTP server capability provider is loaded so that the actor can receive web requests.
//! Note that the link definition (configuration of the link between the actor and the
//! capability provider) can be defined _in any order_. The host runtime automatically
//! establishes links as soon as all related parties are up and running inside a host or
//! a lattice.
//!
//! ```
//! use wasmcloud_host::{HostBuilder, Actor, NativeCapability};
//! use std::collections::HashMap;
//! use std::error::Error;
//! use std::time::Duration;
//! use actix_rt::time::delay_for;
//! extern crate reqwest;
//!
//! const WEB_PORT: u32 = 8080;
//!
//! #[actix_rt::main]
//! async fn main() -> Result<(), Box<dyn Error + Sync +Send>> {
//!     let h = HostBuilder::new().build();
//!     h.start().await?;
//!     let echo = Actor::from_file("../../tests/modules/echo.wasm")?;
//!     let actor_id = echo.public_key();
//!     h.start_actor(echo).await?;
//!
//!     // Read a cross-platform provider archive file
//!     let arc = par_from_file("../../tests/modules/httpserver.par.gz")?;
//!     let websrv = NativeCapability::from_archive(&arc, None)?;
//!     let websrv_id = websrv.id();
//!
//!     let mut webvalues: HashMap<String, String> = HashMap::new();
//!     webvalues.insert("PORT".to_string(), format!("{}", WEB_PORT));
//!     // Establish a link between the actor and a capability provider
//!     h.set_link(
//!         &actor_id,
//!         "wasmcloud:httpserver",
//!         None,
//!         websrv_id,
//!         webvalues,
//!     )
//!     .await?;
//!     // Start the web server provider (which auto-establishes the link)
//!     h.start_native_capability(websrv).await?;
//!     // Let the web server start
//!     delay_for(Duration::from_millis(500)).await;
//!     let url = format!("http://localhost:{}/demo?test=kthxbye", WEB_PORT);
//!
//!     let resp = reqwest::get(&url).await?;
//!     assert!(resp.status().is_success());
//!     let v: serde_json::Value = serde_json::from_slice(&resp.bytes().await?)?;
//!     assert_eq!("test=kthxbye", v["query_string"].as_str().unwrap());
//!
//!     Ok(())
//! }
//!
//! # fn par_from_file(file: &str) -> Result<provider_archive::ProviderArchive, Box<dyn Error + Sync + Send>> {
//! #   use std::io::Read;
//! #   let mut f = std::fs::File::open(file)?;
//! #   let mut buf = Vec::new();
//! #   f.read_to_end(&mut buf)?;
//! #   provider_archive::ProviderArchive::try_load(&buf)
//! # }
//!
//! ```
//!

mod actors;
mod auth;
mod capability;
mod control_interface;
mod dispatch;
mod errors;
mod generated;
mod hlreg;
mod host;
mod host_controller;
mod manifest;
mod messagebus;
mod middleware;
mod oci;

#[macro_use]
extern crate log;

pub use crate::control_interface::events::{ControlEvent, EventHeader, PublishedEvent};
pub use actors::WasmCloudActor;
pub use auth::{Authorizer, CloneAuthorizer};
pub use capability::native::NativeCapability;
pub use dispatch::{Invocation, InvocationResponse, WasmCloudEntity};
pub use host::{Host, HostBuilder};
pub use manifest::HostManifest;

/// Result type used for function calls within this library
pub type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error + Send + Sync>>;
/// Type alias used to disambiguate between wasmCloud actors and Actix actors
pub type Actor = WasmCloudActor;

#[doc(hidden)]
pub const SYSTEM_ACTOR: &str = "system";

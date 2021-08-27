#![doc(html_favicon_url = "https://wasmcloud.com/favicon.ico")]
#![doc(html_logo_url = "https://wasmcloud.com/images/screenshots/Wasmcloud.Icon_Green_704x492.png")]

//! # wasmCloud Host
//!
//! [wasmCloud](https://wasmcloud.dev) is a platform for writing portable business logic that can run anywhere
//! from the edge to the cloud, that boasts a secure-by-default, boilerplate-free developer experience with
//! rapid feedback loop.
//!
//! The wasmCloud team believes that we can not only change the way developers build software for the better,
//! but make it easier to secure, deploy, maintain, observe, and upgrade that software as well--all while reducing
//! the amount of boilerplate we have to copy and paste.
//!
//! wasmCloud is designed around the following core tenets:
//! * Productivity - Developer and Operations
//! * Enterprise-grade Security
//! * Cost Savings
//! * Portability
//! * Performance
//!
//! You should not have to change your design, architecture, or your programming environment as you move from concept to production.
//! wasmCloud aims to bring joy to distributed systems development without sacrificing enterprise-grade features.
//!
//! # Actors
//!
//! wasmCloud's [actors](https://wasmcloud.dev/reference/host-runtime/actors/) are designed in the spirit of the [actor model](https://en.wikipedia.org/wiki/Actor_model), though some of
//! the implementation details may differ from what people might expect from certain actor runtimes. A wasmCloud actor is a
//! single-threaded, portable unit of compute and deployment.
//!
//! Our actors also contain cryptographically signed JSON Web Tokens (JWT) that assert via claims the list of capabilities
//! to which any given actor has been granted access. For more information, check out our [security](https://wasmcloud.dev/reference/host-runtime/security/) documentation.
//!
//! # Capabilities
//!
//! Actors, by virtue of being freestanding (non-[WASI](https://wasi.dev/)) WebAssembly modules, cannot interact with the operating system nor can they
//! perform I/O of any kind. As such, if an actor wants to do anything other than perform pure calculations, it must do so
//! by virtue of a [capability provider](https://wasmcloud.dev/reference/host-runtime/capabilities/), a dynamic plugin loaded by the wasmCloud host runtime that is made available for
//! secure dispatch to and from an actor.
//!
//! # Using the Host API
//!
//! This crate provides the [`primary API`](struct@Host) for interacting with the host runtime. If you are purely interested
//! in using a "stock" binary to run your actor workloads and communicate with capability providers using standard
//! features, then you should use the [wasmCloud](https://wasmcloud.dev/overview/installation/) binary available for installation.
//!
//! If, on the other hand, you are interested in providing a custom host runtime of your own that utilizes the wasmCloud
//! host API as a platform, then this crate is what you'll need.
//!
//! To start a runtime, simply build a host and then add actors, capabilities, and link definitions to it.
//! For more information, take a look at the documentation and tutorials at [wasmcloud.dev](https://wasmcloud.dev).
//!
//! # Host API Example
//!
//! The following example creates a new wasmCloud host in the default standalone (no lattice/single-player) mode. It
//! then loads an actor that echoes back incoming HTTP requests as a JSON object in the body of the outbound HTTP response.
//!
//! The HTTP server capability provider is loaded so that the actor can receive web requests. A [link definition](https://wasmcloud.dev/reference/host-runtime/links/) is required
//! between the HTTP server capability provider and the actor in order to verify actor privileges and supply configuration values
//! (such as the port on which to listen). This link definition can be established _before or after_ the actor and capability
//! provider have been started, as link definitions are first-class data cached throughout a [lattice](https://wasmcloud.dev/reference/lattice/).
//!
//! ```
//! use wasmcloud_host::{HostBuilder, Actor, NativeCapability};
//! use std::collections::HashMap;
//! use std::error::Error;
//! use std::time::Duration;
//! use actix_rt::time::sleep;
//! use reqwest;
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
//!     
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
//!     sleep(Duration::from_millis(500)).await;
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

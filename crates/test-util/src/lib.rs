//! This crate contains utilities for testing [wasmCloud][wasmCloud], a universal application platform powered by [WebAssembly][wasm].
//!
//! You can use this crate to start the wasmCloud host, test both wasmCloud [components][docs-components] and [capability providers][docs-provider]s, and more.
//!
//! ## Quickstart
//!
//! When building integration tests, you can start the wasmCloud host without `wash` and directly from Rust code to test it:
//!
//! ```rust,ignore
//! use wasmcloud_test_util::{assert_config_put, assert_scale_component, WasmCloudTestHost};
//! use wasmcloud_test_util::control_interface::ClientBuilder;
//!
//! # async fn quickstart() -> anyhow::Result<()> {
//!
//! // Build a wasmCloud host (assuming you have a local NATS server running)
//! let nats_url = "nats://localhost:4222";
//! let lattice = "default";
//! let host = WasmCloudTestHost::start(nats_url, lattice).await?;
//!
//! // Once you have a host (AKA a single-member wasmCloud lattice), you'll want a NATS client
//! // which you can use to control the host and the lattice:
//! let nats_client = async_nats::connect(nats_url).await?;
//! let ctl_client = ClientBuilder::new(nats_client)
//!     .lattice(host.lattice_name().to_string())
//!     .build();
//!
//! // Now that you have a control client, you can use the `assert_*` functions to perform actions on your host:
//! assert_config_put(
//!     &ctl_client,
//!     "test-config",
//!     [("EXAMPLE_KEY".to_string(), "EXAMPLE_VALUE".to_string())],
//! )
//! .await?;
//!
//! assert_scale_component(
//!     &ctl_client,
//!     &host.host_key(),
//!     "ghcr.io/wasmcloud/components/http-jsonify-rust:0.1.1",
//!     "example-component",
//!     None,
//!     1,
//!     Vec::new(),
//! )
//! .await?;
//!
//! // ...
//!
//! Ok(())
//! # }
//! ```
//!
//! You can find examples of this crate in use in the [wasmCloud repository `tests` folder](https://github.com/wasmCloud/wasmCloud/tree/main/tests).
//!
//! [wasmCloud]: https://wasmcloud.com
//! [wasm]: https://webassembly.org/
//! [docs-providers]: https://wasmcloud.com/docs/concepts/providers
//! [docs-components]: https://wasmcloud.com/docs/concepts/components
//!

pub mod component;
pub mod host;
pub mod lattice;
pub mod provider;

/// Re-export of control interface fo ruse
pub use wasmcloud_control_interface as control_interface;

pub use crate::component::assert_scale_component;
pub use crate::host::WasmCloudTestHost;
pub use crate::host::{assert_delete_label, assert_put_label};
pub use crate::lattice::config::assert_config_put;
pub use crate::provider::assert_start_provider;

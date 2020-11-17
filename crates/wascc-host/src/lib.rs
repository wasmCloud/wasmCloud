mod actors;
mod auth;
mod capability;
mod control_plane;
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

pub use capability::native::NativeCapability;
pub use control_plane::events::{ControlEvent, EventHeader, PublishedEvent};
pub use control_plane::{ControlInterface, ControlPlaneProvider};
pub use dispatch::{BusDispatcher, Invocation, InvocationResponse, WasccEntity};
pub use host::{Host, HostBuilder};
pub use manifest::HostManifest;
pub use messagebus::LatticeProvider;

pub type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error + Send + Sync>>;
pub type Actor = actors::WasccActor;

pub const SYSTEM_ACTOR: &str = "system";

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

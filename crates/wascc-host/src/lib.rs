mod auth;
mod capability;
mod dispatch;
mod errors;
mod generated;
mod host;
mod host_controller;
mod messagebus;
mod oci;
mod actor_host;

#[macro_use]
extern crate log;

pub use host::{Host, HostBuilder};
pub use messagebus::MessageBusProvider;

pub type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error>>;

pub const SYSTEM_ACTOR: &str = "system";

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

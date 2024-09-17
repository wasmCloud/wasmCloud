pub use testcontainers::{core::Mount, runners::AsyncRunner, ContainerAsync, ImageExt};

pub mod azurite;
pub use azurite::*;

pub mod localstack;
pub use localstack::*;

pub mod nats_server;
pub use nats_server::*;

pub mod squid_proxy;
pub use squid_proxy::*;

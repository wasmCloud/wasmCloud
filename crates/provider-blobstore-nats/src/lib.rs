use crate::config::{NatsConnectionConfig, StorageConfig};
/// [`wasmcloud_provider_blobstore_nats`] crate is a NATS JetStream implementation of the wasmCloud's "wrpc:blobstore/blobstore@0.2.0" interface.
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// [`NatsBlobstore`] holds the handle to opened NATS Object Stores, and their container (bucket) storage configuration.
#[derive(Clone)]
pub(crate) struct NatsBlobstore {
    pub(crate) jetstream: async_nats::jetstream::context::Context,
    pub(crate) storage_config: StorageConfig,
}

/// [`NatsBlobstoreProvider`] holds the default NATS connection configuration and individual consumer
/// components' established NATS JetStream connections.
#[derive(Default, Clone)]
pub struct NatsBlobstoreProvider {
    /// Map of component_id -> link_name -> NATS Object Store JetStream Context (supports multiple links per component)
    consumer_components: Arc<RwLock<HashMap<String, HashMap<String, NatsBlobstore>>>>,
    default_config: NatsConnectionConfig,
}

mod blobstore;
/// Provider modules
mod config;
mod provider;

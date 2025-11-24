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
#[derive(Clone)]
pub struct NatsBlobstoreProvider {
    /// Map of component_id -> link_name -> NATS Object Store JetStream Context (supports multiple links per component)
    consumer_components: Arc<RwLock<HashMap<String, HashMap<String, NatsBlobstore>>>>,
    default_config: Arc<RwLock<NatsConnectionConfig>>,
    /// Shutdown signal sender
    quit_tx: Arc<tokio::sync::broadcast::Sender<()>>,
}

impl NatsBlobstoreProvider {
    pub fn new(quit_tx: tokio::sync::broadcast::Sender<()>) -> Self {
        Self {
            consumer_components: Arc::default(),
            default_config: Arc::default(),
            quit_tx: Arc::new(quit_tx),
        }
    }
}

mod blobstore;
/// Provider modules
mod config;
mod provider;
mod bindings {
    wit_bindgen_wrpc::generate!({
        world: "interfaces",
        with: {
            "wrpc:blobstore/blobstore@0.2.0": generate,
            "wrpc:blobstore/types@0.2.0": wrpc_interface_blobstore::bindings::wrpc::blobstore::types,
            "wasi:blobstore/types@0.2.0-draft": generate,
            "wasi:io/error@0.2.1": generate,
            "wasi:io/poll@0.2.1": generate,
            "wasi:io/streams@0.2.1": generate
        }
    });

    pub mod ext {
        wit_bindgen_wrpc::generate!({
            world: "extensions",
            with: {
                "wrpc:extension/types@0.0.1": wasmcloud_provider_sdk::types,
                "wrpc:extension/manageable@0.0.1": generate,
                "wrpc:extension/configurable@0.0.1": generate,
            }
        });
    }
}

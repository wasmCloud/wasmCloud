#![allow(clippy::type_complexity)]

use core::future::Future;
use core::pin::Pin;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _, Result};
use azure_storage::CloudLocation;
use azure_storage_blobs::prelude::*;
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt as _};
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, instrument};
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, propagate_trace_for_ctx, run_provider,
    serve_provider_exports,
    types::{BindRequest, BindResponse, HealthCheckResponse},
    Context,
};
use wrpc_interface_blobstore::bindings::{
    exports::wrpc::blobstore::blobstore::Handler,
    serve,
    wrpc::blobstore::types::{ContainerMetadata, ObjectId, ObjectMetadata},
};

use config::StorageConfig;

mod config;

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

use bindings::ext::exports::wrpc::extension::{
    configurable::{self, InterfaceConfig},
    manageable,
};

/// Blobstore Azblob provider
///
/// This struct will be the target of generated implementations (via wit-provider-bindgen)
/// for the blobstore provider WIT contract
#[derive(Clone)]
pub struct BlobstoreAzblobProvider {
    /// Per-config storage for Azure connection clients
    config: Arc<RwLock<HashMap<String, BlobServiceClient>>>,
    /// Shutdown signal sender
    quit_tx: Arc<tokio::sync::broadcast::Sender<()>>,
}

impl BlobstoreAzblobProvider {
    pub fn new(quit_tx: tokio::sync::broadcast::Sender<()>) -> Self {
        Self {
            config: Arc::default(),
            quit_tx: Arc::new(quit_tx),
        }
    }

    pub fn name() -> &'static str {
        "blobstore-azure-provider"
    }
}

pub async fn run() -> anyhow::Result<()> {
    BlobstoreAzblobProvider::run().await
}

impl manageable::Handler<Option<Context>> for BlobstoreAzblobProvider {
    async fn bind(
        &self,
        _cx: Option<Context>,
        _req: BindRequest,
    ) -> anyhow::Result<Result<BindResponse, String>> {
        Ok(Ok(BindResponse {
            identity_token: None,
            provider_xkey: Some(get_connection().provider_xkey.public_key().into()),
        }))
    }

    async fn health_request(
        &self,
        _cx: Option<Context>,
    ) -> anyhow::Result<Result<HealthCheckResponse, String>> {
        Ok(Ok(HealthCheckResponse {
            healthy: true,
            message: Some("OK".to_string()),
        }))
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self, _cx: Option<Context>) -> anyhow::Result<Result<(), String>> {
        self.config.write().await.drain();
        // Signal shutdown
        let _ = self.quit_tx.send(());
        Ok(Ok(()))
    }
}

impl configurable::Handler<Option<Context>> for BlobstoreAzblobProvider {
    #[instrument(level = "debug", skip_all)]
    async fn update_base_config(
        &self,
        _cx: Option<Context>,
        config: wasmcloud_provider_sdk::types::BaseConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let flamegraph_path = config
            .config
            .iter()
            .find(|(k, _)| k == "FLAMEGRAPH_PATH")
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var("PROVIDER_BLOBSTORE_AZURE_FLAMEGRAPH_PATH").ok());
        initialize_observability!(Self::name(), flamegraph_path, config.config);

        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn update_interface_export_config(
        &self,
        _cx: Option<Context>,
        source_id: String,
        _link_name: String,
        link_config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config_map: HashMap<String, String> = link_config.config.into_iter().collect();
        let config = match StorageConfig::from_config_map(&config_map) {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, %source_id, "failed to read storage config");
                return Err(e);
            }
        };

        let builder = match config_map.get("CLOUD_LOCATION") {
            Some(custom_location) => ClientBuilder::with_location(
                CloudLocation::Custom {
                    account: config.storage_account.clone(),
                    uri: custom_location.to_string(),
                },
                config.access_key(),
            ),
            None => ClientBuilder::new(config.storage_account.clone(), config.access_key()),
        };
        let client = builder.blob_service_client();

        let mut update_map = self.config.write().await;
        update_map.insert(source_id, client);
        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(target_id))]
    async fn update_interface_import_config(
        &self,
        _cx: Option<Context>,
        _target_id: String,
        _link_name: String,
        _config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(target_id))]
    async fn delete_interface_import_config(
        &self,
        _cx: Option<Context>,
        _target_id: String,
        _link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }

    #[instrument(level = "info", skip_all, fields(source_id))]
    async fn delete_interface_export_config(
        &self,
        _cx: Option<Context>,
        source_id: String,
        _link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        self.config.write().await.remove(&source_id);
        Ok(Ok(()))
    }
}

impl BlobstoreAzblobProvider {
    pub async fn run() -> anyhow::Result<()> {
        let (shutdown, quit_tx) = run_provider(Self::name(), None)
            .await
            .context("failed to run provider")?;
        let provider = Self::new(quit_tx);
        let connection = get_connection();
        let (main_client, ext_client) = connection.get_wrpc_clients_for_serving().await?;
        serve_provider_exports(
            &main_client,
            &ext_client,
            provider,
            shutdown,
            serve,
            bindings::ext::serve,
        )
        .await
        .context("failed to serve provider exports")
    }

    async fn get_config(&self, context: Option<&Context>) -> anyhow::Result<BlobServiceClient> {
        if let Some(source_id) = context.and_then(|Context { component, .. }| component.as_ref()) {
            self.config
                .read()
                .await
                .get(source_id)
                .with_context(|| format!("failed to lookup {source_id} configuration"))
                .cloned()
        } else {
            bail!(
                "failed to lookup source of invocation, could not construct Azure blobstore client"
            )
        }
    }
}

impl Handler<Option<Context>> for BlobstoreAzblobProvider {
    #[instrument(level = "trace", skip(self))]
    async fn clear_container(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let client = client.container_client(&name);
            let mut blob_stream = client.list_blobs().into_stream();
            while let Some(blob_entry) = blob_stream.next().await {
                let blob_entry =
                    blob_entry.with_context(|| format!("failed to list blobs in '{name}'"))?;
                for blob in blob_entry.blobs.blobs() {
                    client
                        .blob_client(&blob.name)
                        .delete()
                        .await
                        .with_context(|| {
                            format!("failed to delete blob '{}' in '{name}'", blob.name)
                        })?;
                }
            }
            Ok(())
        }
        .await
        .map_err(|err: anyhow::Error| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn container_exists(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<bool, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            client
                .container_client(name)
                .exists()
                .await
                .context("failed to check container existence")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn create_container(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            client
                .container_client(name)
                .create()
                .await
                .context("failed to create container")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn delete_container(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            client
                .container_client(name)
                .delete()
                .await
                .context("failed to delete container")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn get_container_info(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<ContainerMetadata, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let properties = client
                .container_client(name)
                .get_properties()
                .await
                .context("failed to get container properties")?;

            let created_at = properties
                .date
                .unix_timestamp()
                .try_into()
                .context("failed to convert created_at date to u64")?;

            // NOTE: The `created_at` format is currently undefined
            // https://github.com/WebAssembly/wasi-blobstore/issues/7
            anyhow::Ok(ContainerMetadata { created_at })
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn list_container_objects(
        &self,
        cx: Option<Context>,
        name: String,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> anyhow::Result<
        Result<
            (
                Pin<Box<dyn Stream<Item = Vec<String>> + Send>>,
                Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
            ),
            String,
        >,
    > {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let mut names = client.container_client(name).list_blobs().into_stream();
            let (tx, rx) = mpsc::channel(16);
            anyhow::Ok((
                Box::pin(ReceiverStream::new(rx)) as Pin<Box<dyn Stream<Item = _> + Send>>,
                Box::pin(async move {
                    let mut offset = offset.unwrap_or_default().try_into().unwrap_or(usize::MAX);
                    let mut limit = limit
                        .and_then(|limit| limit.try_into().ok())
                        .unwrap_or(usize::MAX);
                    while let Some(res) = names.next().await {
                        let res = res
                            .context("failed to receive response")
                            .map_err(|err| format!("{err:#}"))?;
                        let mut chunk = vec![];
                        for name in res.blobs.blobs().map(|Blob { name, .. }| name) {
                            if limit == 0 {
                                break;
                            }
                            if offset > 0 {
                                offset -= 1;
                                continue;
                            }
                            chunk.push(name.clone());
                            limit -= 1;
                        }
                        if !chunk.is_empty() && tx.send(chunk).await.is_err() {
                            return Err("stream receiver closed".to_string());
                        }
                    }
                    Ok(())
                }) as Pin<Box<dyn Future<Output = _> + Send>>,
            ))
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn copy_object(
        &self,
        cx: Option<Context>,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let copy_source = client
                .container_client(src.container)
                .blob_client(src.object)
                .url()
                .context("failed to get source object for copy")?;

            client
                .container_client(dest.container)
                .blob_client(dest.object)
                .copy(copy_source)
                .await
                .map(|_| ())
                .context("failed to copy source object")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn delete_object(
        &self,
        cx: Option<Context>,
        id: ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            client
                .container_client(id.container)
                .blob_client(id.object)
                .delete()
                .await
                .map(|_| ())
                .context("failed to delete object")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn delete_objects(
        &self,
        cx: Option<Context>,
        container: String,
        objects: Vec<String>,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let deletes = objects.iter().map(|object| async {
                client
                    .container_client(container.clone())
                    .blob_client(object.clone())
                    .delete()
                    .await
            });
            futures::future::join_all(deletes)
                .await
                .into_iter()
                .collect::<Result<Vec<_>, azure_storage::Error>>()
                .map(|_| ())
                .context("failed to delete objects")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn get_container_data(
        &self,
        cx: Option<Context>,
        id: ObjectId,
        start: u64,
        end: u64,
    ) -> anyhow::Result<
        Result<
            (
                Pin<Box<dyn Stream<Item = Bytes> + Send>>,
                Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
            ),
            String,
        >,
    > {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let mut stream = client
                .container_client(id.container)
                .blob_client(id.object)
                .get()
                .range(start..end)
                .into_stream();

            let (tx, rx) = mpsc::channel(16);
            anyhow::Ok((
                Box::pin(ReceiverStream::new(rx)) as Pin<Box<dyn Stream<Item = _> + Send>>,
                Box::pin(async move {
                    async move {
                        while let Some(res) = stream.next().await {
                            let res = res.context("failed to receive blob")?;
                            let buf = res
                                .data
                                .collect()
                                .await
                                .context("failed to receive bytes")?;
                            tx.send(buf).await.context("stream receiver closed")?;
                        }
                        anyhow::Ok(())
                    }
                    .await
                    .map_err(|err| format!("{err:#}"))
                }) as Pin<Box<dyn Future<Output = _> + Send>>,
            ))
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn get_object_info(
        &self,
        cx: Option<Context>,
        id: ObjectId,
    ) -> anyhow::Result<Result<ObjectMetadata, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let info = client
                .container_client(id.container)
                .blob_client(id.object)
                .get_properties()
                .await
                .map_err(|e| anyhow::anyhow!(e))?;

            // NOTE: The `created_at` format is currently undefined
            // https://github.com/WebAssembly/wasi-blobstore/issues/7
            let created_at = info
                .blob
                .properties
                .creation_time
                .unix_timestamp()
                .try_into()
                .context("failed to convert created_at date to u64")?;
            anyhow::Ok(ObjectMetadata {
                created_at,
                size: info.blob.properties.content_length,
            })
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn has_object(
        &self,
        cx: Option<Context>,
        id: ObjectId,
    ) -> anyhow::Result<Result<bool, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            client
                .container_client(id.container)
                .blob_client(id.object)
                .exists()
                .await
                .map_err(|e| anyhow::anyhow!(e))
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn move_object(
        &self,
        cx: Option<Context>,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;

            let source_client = client
                .container_client(src.container)
                .blob_client(src.object);

            // Copy and then delete the source object
            let copy_source = source_client
                .url()
                .context("failed to get source object for copy")?;

            client
                .container_client(dest.container)
                .blob_client(dest.object)
                .copy(copy_source)
                .await
                .map(|_| ())
                .context("failed to copy source object to move")?;

            source_client
                .delete()
                .await
                .map(|_| ())
                .context("failed to delete source object")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self, data))]
    async fn write_container_data(
        &self,
        cx: Option<Context>,
        id: ObjectId,
        data: Pin<Box<dyn Stream<Item = Bytes> + Send>>,
    ) -> anyhow::Result<Result<Pin<Box<dyn Future<Output = Result<(), String>> + Send>>, String>>
    {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let client = self
                .get_config(cx.as_ref())
                .await
                .context("failed to retrieve azure blobstore client")?;
            let client = client.container_client(id.container).blob_client(id.object);
            anyhow::Ok(Box::pin(async move {
                // TODO: Stream data
                let data: BytesMut = data.collect().await;
                client
                    .put_block_blob(data)
                    .await
                    .map(|_| ())
                    .context("failed to write container data")
                    .map_err(|err| format!("{err:#}"))?;
                Ok(())
            }) as Pin<Box<dyn Future<Output = _> + Send>>)
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }
}

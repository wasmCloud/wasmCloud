//! blobstore-fs capability provider
//!
//!

use core::future::Future;
use core::pin::pin;

use core::sync::atomic::{AtomicUsize, Ordering};
use std::collections::HashMap;
use std::io::SeekFrom;
use std::os::unix::prelude::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{anyhow, bail, Context as _};
use async_nats::HeaderMap;
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use path_clean::PathClean;
use tokio::fs::{self, create_dir_all, File};
use tokio::io::AsyncSeekExt as _;
use tokio::sync::RwLock;
use tokio::{select, spawn};
use tokio_stream::wrappers::ReadDirStream;
use tokio_util::io::ReaderStream;
use tracing::{debug, error, info, instrument, warn};
use wasmcloud_provider_sdk::provider::invocation_context;
use wasmcloud_provider_sdk::{
    get_connection, Context, LinkConfig, ProviderHandler, ProviderOperationResult,
};
use wrpc_transport::{AcceptedInvocation, Transmitter};

#[allow(unused)]
const CAPABILITY_ID: &str = "wasmcloud:blobstore";
#[allow(unused)]
const FIRST_SEQ_NBR: u64 = 0;

pub type ChunkOffsetKey = (String, usize);

#[derive(Default, Debug, Clone)]
struct FsProviderConfig {
    root: Arc<PathBuf>,
}

/// fs capability provider implementation
#[derive(Default, Clone)]
pub struct FsProvider {
    config: Arc<RwLock<HashMap<String, FsProviderConfig>>>,
}

/// Resolve a path with two components (base & root),
/// ensuring that the path is below the given root.
fn resolve_subpath(root: &Path, path: impl AsRef<Path>) -> Result<PathBuf, std::io::Error> {
    let joined = root.join(&path);
    let joined = joined.clean();

    // Check components of either path
    let mut joined_abs_iter = joined.components();
    for root_part in root.components() {
        let joined_part = joined_abs_iter.next();

        // If the joined path is shorter or doesn't match
        // for the duration of the root, path is suspect
        if joined_part.is_none() || joined_part != Some(root_part) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "Invalid path [{}], is not contained by root path [{}]",
                    path.as_ref().display(),
                    root.display(),
                ),
            ));
        }
    }

    // At this point, the root iterator has ben exhausted
    // and the remaining components are the paths beneath the root
    Ok(joined)
}

impl FsProvider {
    async fn get_root(&self, headers: Option<&HeaderMap>) -> anyhow::Result<Arc<PathBuf>> {
        if let Some(ref source_id) = headers
            .map(invocation_context)
            .and_then(|Context { actor, .. }| actor)
        {
            self.config
                .read()
                .await
                .get(source_id)
                .with_context(|| format!("failed to lookup {source_id} configuration"))
                .map(|FsProviderConfig { root }| Arc::clone(root))
        } else {
            // TODO: Support a default here
            bail!("failed to lookup invocation source ID")
        }
    }

    async fn get_container(
        &self,
        headers: Option<&HeaderMap>,
        container: impl AsRef<Path>,
    ) -> anyhow::Result<PathBuf> {
        let root = self
            .get_root(headers)
            .await
            .context("failed to get container root")?;
        resolve_subpath(&root, container).context("failed to resolve subpath")
    }

    async fn get_object(
        &self,
        headers: Option<&HeaderMap>,
        wrpc_interface_blobstore::ObjectId { container, object }: wrpc_interface_blobstore::ObjectId,
    ) -> anyhow::Result<PathBuf> {
        let container = self
            .get_container(headers, container)
            .await
            .context("failed to get container")?;
        resolve_subpath(&container, object).context("failed to resolve subpath")
    }

    #[instrument(level = "trace", skip_all)]
    pub async fn serve(&self, commands: impl Future<Output = ()>) -> anyhow::Result<()> {
        let connection = get_connection();
        let wrpc = connection.get_wrpc_client(connection.provider_key());
        let mut commands = pin!(commands);
        'outer: loop {
            use wrpc_interface_blobstore::Blobstore as _;
            let clear_container_invocations = wrpc.serve_clear_container().await.context(
                "failed to serve `wrpc:blobstore/blobstore.clear-container` invocations",
            )?;
            let mut clear_container_invocations = pin!(clear_container_invocations);

            let container_exists_invocations = wrpc.serve_container_exists().await.context(
                "failed to serve `wrpc:blobstore/blobstore.container-exists` invocations",
            )?;
            let mut container_exists_invocations = pin!(container_exists_invocations);

            let create_container_invocations = wrpc.serve_create_container().await.context(
                "failed to serve `wrpc:blobstore/blobstore.create-container` invocations",
            )?;
            let mut create_container_invocations = pin!(create_container_invocations);

            let delete_container_invocations = wrpc.serve_delete_container().await.context(
                "failed to serve `wrpc:blobstore/blobstore.delete-container` invocations",
            )?;
            let mut delete_container_invocations = pin!(delete_container_invocations);

            let get_container_info_invocations = wrpc.serve_get_container_info().await.context(
                "failed to serve `wrpc:blobstore/blobstore.get-container-info` invocations",
            )?;
            let mut get_container_info_invocations = pin!(get_container_info_invocations);

            let list_container_objects_invocations =
                wrpc.serve_list_container_objects().await.context(
                    "failed to serve `wrpc:blobstore/blobstore.list-container-objects` invocations",
                )?;
            let mut list_container_objects_invocations = pin!(list_container_objects_invocations);

            let copy_object_invocations = wrpc
                .serve_copy_object()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.copy-object` invocations")?;
            let mut copy_object_invocations = pin!(copy_object_invocations);

            let delete_object_invocations = wrpc
                .serve_delete_object()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.delete-object` invocations")?;
            let mut delete_object_invocations = pin!(delete_object_invocations);

            let delete_objects_invocations = wrpc
                .serve_delete_objects()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.delete-objects` invocations")?;
            let mut delete_objects_invocations = pin!(delete_objects_invocations);

            let get_container_data_invocations = wrpc.serve_get_container_data().await.context(
                "failed to serve `wrpc:blobstore/blobstore.get-container-data` invocations",
            )?;
            let mut get_container_data_invocations = pin!(get_container_data_invocations);

            let get_object_info_invocations = wrpc.serve_get_object_info().await.context(
                "failed to serve `wrpc:blobstore/blobstore.get-object-info` invocations",
            )?;
            let mut get_object_info_invocations = pin!(get_object_info_invocations);

            let has_object_invocations = wrpc
                .serve_has_object()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.has-object` invocations")?;
            let mut has_object_invocations = pin!(has_object_invocations);

            let move_object_invocations = wrpc
                .serve_move_object()
                .await
                .context("failed to serve `wrpc:blobstore/blobstore.move-object` invocations")?;
            let mut move_object_invocations = pin!(move_object_invocations);

            let write_container_data_invocations =
                wrpc.serve_write_container_data().await.context(
                    "failed to serve `wrpc:blobstore/blobstore.write-container-data` invocations",
                )?;
            let mut write_container_data_invocations = pin!(write_container_data_invocations);

            loop {
                select! {
                    invocation = clear_container_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_clear_container(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.clear-container` invocation")
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.clear-container` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            }
                        }
                    },
                    invocation = container_exists_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_container_exists(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.container-exists` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.container-exists` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = create_container_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_create_container(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.container-exists` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.container-exists` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = delete_container_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_delete_container(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.delete-container` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.delete-container` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = get_container_info_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_get_container_info(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.get-container-info` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.get-container-info` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = list_container_objects_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_list_container_objects(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.list-container-objects` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.list-container-objects` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = copy_object_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_copy_object(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.copy-object` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.copy-object` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = delete_object_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_delete_object(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.delete-object` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.delete-object` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = delete_objects_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_delete_objects(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.delete-objects` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.delete-objects` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = get_container_data_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_get_container_data(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.get-container-data` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.get-container-data` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = get_object_info_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_get_object_info(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.get-object-info` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.get-object-info` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = has_object_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_has_object(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.has-object` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.has-object` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = move_object_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_move_object(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.move-object` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.move-object` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    invocation = write_container_data_invocations.next() => {
                        match invocation {
                            Some(Ok(invocation)) => {
                                let provider = self.clone();
                                spawn(async move { provider.serve_write_container_data(invocation).await });
                            },
                            Some(Err(err)) => {
                                error!(?err, "failed to accept `wrpc:blobstore/blobstore.write-container-data` invocation") ;
                            },
                            None => {
                                warn!("`wrpc:blobstore/blobstore.write-container-data` stream unexpectedly finished, resubscribe");
                                continue 'outer
                            },
                        }
                    },
                    _ = &mut commands => {
                        debug!("shutdown command received");
                        return Ok(())
                    }
                }
            }
        }
    }
}

impl FsProvider {
    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_clear_container<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<HeaderMap>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let path = self.get_container(context.as_ref(), container).await?;
                    debug!("read directory at `{}`", path.display());
                    let dir = fs::read_dir(path).await.context("failed to read path")?;
                    ReadDirStream::new(dir)
                        .map(|entry| entry.context("failed to lookup directory entry"))
                        .try_for_each_concurrent(None, |entry| async move {
                            let ty = entry
                                .file_type()
                                .await
                                .context("failed to lookup directory entry type")?;
                            let path = entry.path();
                            if ty.is_dir() {
                                fs::remove_dir_all(&path).await.with_context(|| {
                                    format!("failed to remove directory at `{}`", path.display())
                                })?
                            } else {
                                fs::remove_file(&path).await.with_context(|| {
                                    format!("failed to remove file at `{}`", path.display())
                                })?
                            }
                            Ok(())
                        })
                        .await
                        .context("failed to remove directory contents")
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_container_exists<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<HeaderMap>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let path = self.get_container(context.as_ref(), container).await?;
                    fs::try_exists(path)
                        .await
                        .context("failed to check if path exists")
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_create_container<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<HeaderMap>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let path = self.get_container(context.as_ref(), container).await?;
                    fs::create_dir_all(path)
                        .await
                        .context("failed to create path")
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_delete_container<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<HeaderMap>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let path = self.get_container(context.as_ref(), container).await?;
                    fs::remove_dir_all(path)
                        .await
                        .context("failed to remove path")
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_get_container_info<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: container,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<HeaderMap>, String, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let path = self.get_container(context.as_ref(), container).await?;
                    let md = fs::metadata(path)
                        .await
                        .context("failed to lookup directory metadata")?;
                    let created_at = md.created().context("failed to lookup creation date")?;
                    let created_at = created_at
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .context("creation time before Unix epoch")?;
                    // NOTE: The `created_at` format is currently undefined
                    // https://github.com/WebAssembly/wasi-blobstore/issues/7
                    anyhow::Ok(wrpc_interface_blobstore::ContainerMetadata {
                        created_at: created_at.as_secs(),
                    })
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[allow(clippy::type_complexity)]
    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_list_container_objects<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (container, limit, offset),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<HeaderMap>, (String, Option<u64>, Option<u64>), Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let path = self.get_container(context.as_ref(), container).await?;
                    let dir = fs::read_dir(path).await.context("failed to read path")?;
                    let names = ReadDirStream::new(dir)
                        .skip(offset.unwrap_or_default().try_into().unwrap_or(usize::MAX))
                        .take(limit.unwrap_or_default().try_into().unwrap_or(usize::MAX))
                        .then(|entry| async move {
                            let entry = entry.context("failed to lookup directory entry")?;
                            let name = entry.file_name().to_string_lossy().to_string();
                            // TODO: Remove the need for this wrapping
                            Ok(vec![Some(wrpc_transport::Value::String(name))])
                        });
                    anyhow::Ok(wrpc_transport::Value::Stream(Box::pin(names)))
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_copy_object<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (src, dest),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<HeaderMap>,
            (
                wrpc_interface_blobstore::ObjectId,
                wrpc_interface_blobstore::ObjectId,
            ),
            Tx,
        >,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let root = self
                        .get_root(context.as_ref())
                        .await
                        .context("failed to get root")?;
                    let src_container = resolve_subpath(&root, src.container)
                        .context("failed to resolve source container path")?;
                    let src = resolve_subpath(&src_container, src.object)
                        .context("failed to resolve source object path")?;

                    let dest_container = resolve_subpath(&root, dest.container)
                        .context("failed to resolve destination container path")?;
                    let dest = resolve_subpath(&dest_container, dest.object)
                        .context("failed to resolve destination object path")?;
                    debug!("copy `{}` to `{}`", src.display(), dest.display());
                    fs::copy(src, dest).await.context("failed to copy")
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_delete_object<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: id,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<HeaderMap>, wrpc_interface_blobstore::ObjectId, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let path = self.get_object(context.as_ref(), id).await?;
                    debug!("remove file at `{}`", path.display());
                    match fs::remove_file(&path).await {
                        Ok(()) => Ok(()),
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(err) => Err(anyhow!(err)
                            .context(format!("failed to remove file at `{}`", path.display()))),
                    }
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_delete_objects<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (container, objects),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<HeaderMap>, (String, Vec<String>), Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let container = self.get_container(context.as_ref(), container).await?;
                    for name in objects {
                        let path = resolve_subpath(&container, name)
                            .context("failed to resolve object path")?;
                        debug!("remove file at `{}`", path.display());
                        match fs::remove_file(&path).await {
                            Ok(()) => Ok(()),
                            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                            Err(err) => Err(anyhow!(err)
                                .context(format!("failed to remove file at `{}`", path.display()))),
                        }?;
                    }
                    anyhow::Ok(())
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_get_container_data<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (id, start, end),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<HeaderMap>,
            (wrpc_interface_blobstore::ObjectId, u64, u64),
            Tx,
        >,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let limit = end
                        .checked_sub(start)
                        .context("`end` must be greater than `start`")?;
                    let limit = Arc::new(AtomicUsize::new(limit.try_into().unwrap_or(usize::MAX)));
                    let path = self.get_object(context.as_ref(), id).await?;
                    let mut object = File::open(path).await.context("failed to open file")?;
                    object
                        .seek(SeekFrom::Start(start))
                        .await
                        .context("failed to seek from start")?;
                    let data = ReaderStream::new(object)
                        .take_while({
                            let limit = Arc::clone(&limit);
                            move |_| {
                                let limit = Arc::clone(&limit);
                                async move { limit.load(Ordering::Relaxed) > 0 }
                            }
                        })
                        .map(move |buf| {
                            // TODO: Remove the need for this wrapping
                            let mut buf = buf.context("failed to read file")?;
                            let n = limit.load(Ordering::Relaxed);
                            if buf.len() > n {
                                buf.truncate(n);
                                limit.store(0, Ordering::Relaxed);
                            } else {
                                limit.fetch_sub(buf.len(), Ordering::Relaxed);
                            }
                            Ok(vec![Some(wrpc_transport::Value::List(
                                buf.into_iter().map(wrpc_transport::Value::U8).collect(),
                            ))])
                        });
                    anyhow::Ok(wrpc_transport::Value::Stream(Box::pin(data)))
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_get_object_info<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: id,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<HeaderMap>, wrpc_interface_blobstore::ObjectId, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let path = self.get_object(context.as_ref(), id).await?;
                    let md = fs::metadata(path)
                        .await
                        .context("failed to lookup file metadata")?;
                    let created_at = md.created().context("failed to lookup creation date")?;
                    let created_at = created_at
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .context("creation time before Unix epoch")?;
                    // NOTE: The `created_at` format is currently undefined
                    // https://github.com/WebAssembly/wasi-blobstore/issues/7
                    anyhow::Ok(wrpc_interface_blobstore::ObjectMetadata {
                        created_at: created_at.as_secs(),
                        size: md.size(),
                    })
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_has_object<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: id,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<HeaderMap>, wrpc_interface_blobstore::ObjectId, Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let path = self.get_object(context.as_ref(), id).await?;
                    fs::try_exists(path)
                        .await
                        .context("failed to check if path exists")
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "trace", skip(self, result_subject, transmitter))]
    async fn serve_move_object<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (src, dest),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<HeaderMap>,
            (
                wrpc_interface_blobstore::ObjectId,
                wrpc_interface_blobstore::ObjectId,
            ),
            Tx,
        >,
    ) {
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let root = self
                        .get_root(context.as_ref())
                        .await
                        .context("failed to get root")?;
                    let src_container = resolve_subpath(&root, src.container)
                        .context("failed to resolve source container path")?;
                    let src = resolve_subpath(&src_container, src.object)
                        .context("failed to resolve source object path")?;

                    let dest_container = resolve_subpath(&root, dest.container)
                        .context("failed to resolve destination container path")?;
                    let dest = resolve_subpath(&dest_container, dest.object)
                        .context("failed to resolve destination object path")?;
                    fs::copy(&src, dest).await.context("failed to copy")?;
                    fs::remove_file(src)
                        .await
                        .context("failed to remove source")
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(
        level = "trace",
        skip(self, result_subject, error_subject, transmitter, data)
    )]
    async fn serve_write_container_data<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (id, data),
            result_subject,
            error_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<HeaderMap>,
            (
                wrpc_interface_blobstore::ObjectId,
                impl Stream<Item = anyhow::Result<Bytes>> + Send,
            ),
            Tx,
        >,
    ) {
        // TODO: Consider streaming to FS
        let data: BytesMut = match data.try_collect().await {
            Ok(data) => data,
            Err(err) => {
                error!(?err, "failed to receive value");
                if let Err(err) = transmitter
                    .transmit_static(error_subject, err.to_string())
                    .await
                {
                    error!(?err, "failed to transmit error")
                }
                return;
            }
        };
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                async {
                    let path = self.get_object(context.as_ref(), id).await?;
                    fs::write(path, data).await.context("failed to write file")
                }
                .await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }
}

#[async_trait]
impl ProviderHandler for FsProvider {
    /// The fs provider has one configuration parameter, the root of the file system
    async fn receive_link_config_as_target(
        &self,
        link_config: impl LinkConfig,
    ) -> ProviderOperationResult<()> {
        let source_id = link_config.get_source_id();
        let config_values = link_config.get_config();
        for (k, v) in config_values.iter() {
            info!("link definition configuration [{k}] set to [{v}]");
        }

        // Determine the root path value
        let root_val: PathBuf = match config_values.iter().find(|(key, _)| **key == "ROOT") {
            None => "/tmp".into(),
            Some((_, value)) => value.into(),
        };

        // Build configuration for FS Provider to use later
        let config = FsProviderConfig {
            root: Arc::new(root_val.clean()),
        };

        info!("Saved FsProviderConfig: {:#?}", config);
        info!(
            "File System Blob Store Container Root: '{:?}'",
            &config.root
        );

        // Save the configuration for the actor
        self.config
            .write()
            .await
            .insert(source_id.into(), config.clone());

        // Resolve the subpath from the root to the actor ID, carefully
        let actor_dir = match resolve_subpath(&config.root, source_id) {
            Ok(path) => path,
            Err(e) => {
                error!("Failed to resolve subpath to actor directory: {e}");
                return Err(anyhow!(e)
                    .context("failed to resolve subpath to actor dir")
                    .into());
            }
        };

        // Create directory for the individual actor
        if let Err(e) = create_dir_all(actor_dir.as_path()).await {
            error!("Could not create actor directory: {:?}", e);
            return Err(anyhow!(e)
                .context("failed to create actor directory")
                .into());
        }

        Ok(())
    }

    async fn delete_link(&self, source_id: &str) -> ProviderOperationResult<()> {
        self.config.write().await.remove(source_id);
        Ok(())
    }

    async fn shutdown(&self) -> ProviderOperationResult<()> {
        self.config.write().await.drain();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_subpath;

    use std::path::PathBuf;

    /// Ensure that only safe subpaths are resolved
    #[tokio::test]
    async fn resolve_safe_samepath() {
        assert!(resolve_subpath(&PathBuf::from("./"), "./././").is_ok());
    }

    /// Ensure that ancestor paths are not allowed to be resolved as subpaths
    #[tokio::test]
    async fn resolve_fail_ancestor() {
        let res = resolve_subpath(&PathBuf::from("./"), "../").unwrap_err();
        assert_eq!(res.kind(), std::io::ErrorKind::PermissionDenied);
    }
}

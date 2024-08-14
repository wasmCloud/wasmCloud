#![allow(clippy::type_complexity)]

//! blobstore-fs capability provider
//!
//!

use core::pin::Pin;
use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context as _};
use bindings::wrpc::blobstore::types::{ObjectId, ObjectMetadata};
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use path_clean::PathClean;
use tokio::fs::{self, create_dir_all, File};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
use tokio::spawn;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::{ReadDirStream, ReceiverStream};
use tokio_util::io::ReaderStream;
use tracing::{debug, error, info, instrument, trace, warn};
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, propagate_trace_for_ctx, run_provider,
    serve_provider_exports, Context, LinkConfig, LinkDeleteInfo, Provider,
};

use crate::bindings::wrpc::blobstore::types::ContainerMetadata;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wasi:blobstore/types@0.2.0-draft": generate,
            "wasi:io/error@0.2.0": generate,
            "wasi:io/poll@0.2.0": generate,
            "wasi:io/streams@0.2.0": generate,
            "wrpc:blobstore/blobstore@0.1.0": generate,
            "wrpc:blobstore/types@0.1.0": generate,
        }
    });
}

#[derive(Default, Debug, Clone)]
struct FsProviderConfig {
    root: Arc<PathBuf>,
}

/// fs capability provider implementation
#[derive(Default, Clone)]
pub struct FsProvider {
    config: Arc<RwLock<HashMap<String, FsProviderConfig>>>,
}

pub async fn run() -> anyhow::Result<()> {
    FsProvider::run().await
}

impl FsProvider {
    pub async fn run() -> anyhow::Result<()> {
        initialize_observability!(
            "blobstore-fs-provider",
            std::env::var_os("PROVIDER_BLOBSTORE_FS_FLAMEGRAPH_PATH")
        );

        let provider = Self::default();
        let shutdown = run_provider(provider.clone(), "blobstore-fs-provider")
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        serve_provider_exports(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
            shutdown,
            bindings::serve,
        )
        .await
        .context("failed to serve provider exports")
    }
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
    async fn get_root(&self, context: Option<Context>) -> anyhow::Result<Arc<PathBuf>> {
        if let Some(ref source_id) = context.and_then(|Context { component, .. }| component) {
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
        context: Option<Context>,
        container: impl AsRef<Path>,
    ) -> anyhow::Result<PathBuf> {
        let root = self
            .get_root(context)
            .await
            .context("failed to get container root")?;
        resolve_subpath(&root, container).context("failed to resolve subpath")
    }

    async fn get_object(
        &self,
        context: Option<Context>,
        ObjectId { container, object }: ObjectId,
    ) -> anyhow::Result<PathBuf> {
        let container = self
            .get_container(context, container)
            .await
            .context("failed to get container")?;
        resolve_subpath(&container, object).context("failed to resolve subpath")
    }
}

impl bindings::exports::wrpc::blobstore::blobstore::Handler<Option<Context>> for FsProvider {
    #[instrument(level = "trace", skip(self))]
    async fn clear_container(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let path = self.get_container(cx, name).await?;
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
                        })?;
                    } else {
                        fs::remove_file(&path).await.with_context(|| {
                            format!("failed to remove file at `{}`", path.display())
                        })?;
                    }
                    Ok(())
                })
                .await
                .context("failed to remove directory contents")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }

    #[instrument(level = "trace", skip(self))]
    async fn container_exists(
        &self,
        cx: Option<Context>,
        name: String,
    ) -> anyhow::Result<Result<bool, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let path = self.get_container(cx, name).await?;
            fs::try_exists(path)
                .await
                .context("failed to check if path exists")
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
            let path = self.get_container(cx, name).await?;
            fs::create_dir_all(path)
                .await
                .context("failed to create path")
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
            let path = self.get_container(cx, name).await?;
            fs::remove_dir_all(path)
                .await
                .context("failed to remove path")
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
            let path = self.get_container(cx, name).await?;
            let md = fs::metadata(&path)
                .await
                .context("failed to lookup directory metadata")?;

            let created_at = match md.created() {
                Ok(created_time) => created_time
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .context("creation time before Unix epoch")?,
                Err(e) => {
                    // NOTE: Some platforms don't have support for creation time, so we default to the unix epoch
                    debug!(
                        error = ?e,
                        ?path,
                        "failed to get creation time for container, defaulting to 0"
                    );
                    Duration::from_secs(0)
                }
            };
            // NOTE: The `created_at` format is currently undefined
            // https://github.com/WebAssembly/wasi-blobstore/issues/7
            anyhow::Ok(ContainerMetadata {
                created_at: created_at.as_secs(),
            })
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
    ) -> anyhow::Result<Result<Pin<Box<dyn Stream<Item = Vec<String>> + Send>>, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let path = self.get_container(cx, name).await?;
            let offset = offset.unwrap_or_default().try_into().unwrap_or(usize::MAX);
            let limit = limit.unwrap_or(u64::MAX).try_into().unwrap_or(usize::MAX);
            debug!(path = ?path.display(), offset, limit, "read directory");
            let dir = fs::read_dir(path).await.context("failed to read path")?;
            let mut names = ReadDirStream::new(dir)
                .skip(offset)
                .take(limit)
                .map(move |entry| {
                    let entry = entry.context("failed to lookup directory entry")?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    trace!(name, "list file name");
                    anyhow::Ok(name)
                });
            let (tx, rx) = mpsc::channel(16);
            spawn(async move {
                while let Some(name) = names.next().await {
                    match name {
                        Ok(name) => {
                            if tx.send(vec![name]).await.is_err() {
                                warn!("stream receiver closed");
                                return;
                            }
                        }
                        Err(err) => {
                            error!(?err, "failed to list file names");
                            return;
                        }
                    }
                }
            });
            anyhow::Ok(Box::pin(ReceiverStream::new(rx)) as Pin<Box<dyn Stream<Item = _> + Send>>)
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
            let root = self.get_root(cx).await.context("failed to get root")?;
            let src_container = resolve_subpath(&root, src.container)
                .context("failed to resolve source container path")?;
            let src = resolve_subpath(&src_container, src.object)
                .context("failed to resolve source object path")?;

            let dest_container = resolve_subpath(&root, dest.container)
                .context("failed to resolve destination container path")?;
            let dest = resolve_subpath(&dest_container, dest.object)
                .context("failed to resolve destination object path")?;
            debug!("copy `{}` to `{}`", src.display(), dest.display());
            fs::copy(src, dest).await.context("failed to copy")?;
            anyhow::Ok(())
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
            let path = self.get_object(cx, id).await?;
            debug!("remove file at `{}`", path.display());
            match fs::remove_file(&path).await {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(err) => {
                    Err(anyhow!(err)
                        .context(format!("failed to remove file at `{}`", path.display())))
                }
            }
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
            let container = self.get_container(cx, container).await?;
            for name in objects {
                let path =
                    resolve_subpath(&container, name).context("failed to resolve object path")?;
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
    ) -> anyhow::Result<Result<Pin<Box<dyn Stream<Item = Bytes> + Send>>, String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            let limit = end
                .checked_sub(start)
                .context("`end` must be greater than `start`")?;
            let path = self.get_object(cx, id).await?;
            debug!(path = ?path.display(), "open file");
            let mut object = File::open(&path)
                .await
                .with_context(|| format!("failed to open object file [{}]", path.display()))?;
            if start > 0 {
                debug!("seek file");
                object
                    .seek(SeekFrom::Start(start))
                    .await
                    .context("failed to seek from start")?;
            }
            let mut data = ReaderStream::new(object.take(limit));
            let (tx, rx) = mpsc::channel(16);
            spawn(async move {
                while let Some(buf) = data.next().await {
                    match buf {
                        Ok(buf) => {
                            debug!(?buf, "sending chunk");
                            if tx.send(buf).await.is_err() {
                                warn!("stream receiver closed");
                                return;
                            }
                        }
                        Err(err) => {
                            error!(?err, "failed to read file");
                            return;
                        }
                    }
                }
                debug!("finished reading file");
            });
            anyhow::Ok(Box::pin(ReceiverStream::new(rx)) as Pin<Box<dyn Stream<Item = _> + Send>>)
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
            let path = self.get_object(cx, id).await?;
            let md = fs::metadata(&path)
                .await
                .context("failed to lookup file metadata")?;

            let created_at = match md.created() {
                Ok(created_time) => created_time
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .context("creation time before Unix epoch")?,
                Err(e) => {
                    // NOTE: Some platforms don't have support for creation time, so we default to the unix epoch
                    debug!(
                        error = ?e,
                        ?path,
                        "failed to get creation time for object, defaulting to 0"
                    );
                    Duration::from_secs(0)
                }
            };
            // NOTE: The `created_at` format is currently undefined
            // https://github.com/WebAssembly/wasi-blobstore/issues/7
            #[cfg(unix)]
            let size = std::os::unix::fs::MetadataExt::size(&md);
            #[cfg(windows)]
            let size = std::os::windows::fs::MetadataExt::file_size(&md);
            anyhow::Ok(ObjectMetadata {
                created_at: created_at.as_secs(),
                size,
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
            let path = self.get_object(cx, id).await?;
            fs::try_exists(path)
                .await
                .context("failed to check if path exists")
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
            let root = self.get_root(cx).await.context("failed to get root")?;
            let src_container = resolve_subpath(&root, src.container)
                .context("failed to resolve source container path")?;
            let src = resolve_subpath(&src_container, src.object)
                .context("failed to resolve source object path")?;

            let dest_container = resolve_subpath(&root, dest.container)
                .context("failed to resolve destination container path")?;
            let dest = resolve_subpath(&dest_container, dest.object)
                .context("failed to resolve destination object path")?;
            debug!("copy `{}` to `{}`", src.display(), dest.display());
            fs::copy(&src, dest).await.context("failed to copy")?;
            debug!("remove `{}`", src.display());
            fs::remove_file(src)
                .await
                .context("failed to remove source")
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
    ) -> anyhow::Result<Result<(), String>> {
        Ok(async {
            propagate_trace_for_ctx!(cx);
            // TODO: Consider streaming
            let data: BytesMut = data.collect().await;
            let path = self.get_object(cx, id).await?;
            fs::write(path, data).await.context("failed to write file")
        }
        .await
        .map_err(|err| format!("{err:#}")))
    }
}

impl Provider for FsProvider {
    /// The fs provider has one configuration parameter, the root of the file system
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            source_id, config, ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        for (k, v) in config {
            info!("link definition configuration [{k}] set to [{v}]");
        }

        // Determine the root path value
        let root_val: PathBuf = match config.iter().find(|(key, _)| key.to_uppercase() == "ROOT") {
            None => {
                // If no root is specified, use the tempdir and create a specific directory for this component
                let root = std::env::temp_dir();
                // Resolve the subpath from the root to the component ID, carefully
                match resolve_subpath(&root, source_id) {
                    Ok(path) => path,
                    Err(e) => {
                        error!("Failed to resolve subpath to component directory: {e}");
                        return Err(
                            anyhow!(e).context("failed to resolve subpath to component dir")
                        );
                    }
                }
            }
            // If a root is manually specified, use that path exactly
            Some((_, value)) => value.into(),
        };

        // Ensure the root path exists
        if let Err(e) = create_dir_all(&root_val).await {
            error!("Could not create component directory: {:?}", e);
            return Err(anyhow!(e).context("failed to create component directory"));
        }

        // Build configuration for FS Provider to use later
        let config = FsProviderConfig {
            root: Arc::new(root_val.clean()),
        };

        info!("Saved FsProviderConfig: {:#?}", config);
        info!(
            "File System Blob Store Container Root: '{:?}'",
            &config.root
        );

        // Save the configuration for the component
        self.config
            .write()
            .await
            .insert(source_id.into(), config.clone());

        Ok(())
    }

    #[instrument(level = "info", skip_all, fields(source_id = info.get_source_id()))]
    async fn delete_link_as_target(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_source_id();
        self.config.write().await.remove(component_id);
        Ok(())
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.config.write().await.drain();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

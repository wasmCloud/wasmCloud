//! blobstore-fs capability provider
//!
//!

use std::time::SystemTime;
use std::{
    collections::HashMap,
    io::{Error as IoError, ErrorKind as IoErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
};

use path_clean::PathClean;
use tokio::fs::{
    create_dir_all, metadata, read, read_dir, remove_dir_all, remove_file, File, OpenOptions,
};
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tracing::{error, info};

use wasmcloud_provider_wit_bindgen::deps::{
    async_trait::async_trait,
    serde::Deserialize,
    wasmcloud_provider_sdk::core::LinkDefinition,
    wasmcloud_provider_sdk::error::{ProviderInvocationError, ProviderInvocationResult},
    wasmcloud_provider_sdk::Context,
};

mod fs_utils;
use fs_utils::all_dirs;

wasmcloud_provider_wit_bindgen::generate!({
    impl_struct: FsProvider,
    contract: "wasmcloud:blobstore",
    wit_bindgen_cfg: "provider-blobstore"
});

#[allow(unused)]
const CAPABILITY_ID: &str = "wasmcloud:blobstore";
#[allow(unused)]
const FIRST_SEQ_NBR: u64 = 0;

pub type ChunkOffsetKey = (String, usize);

#[derive(Default, Debug, Clone, Deserialize)]
#[serde(crate = "wasmcloud_provider_wit_bindgen::deps::serde")]
struct FsProviderConfig {
    ld: LinkDefinition,
    root: PathBuf,
}

/// fs capability provider implementation
#[derive(Clone)]
pub struct FsProvider {
    config: Arc<RwLock<HashMap<String, FsProviderConfig>>>,
    upload_chunks: Arc<RwLock<HashMap<String, u64>>>, // keep track of the next offset for chunks to be uploaded
    download_chunks: Arc<RwLock<HashMap<ChunkOffsetKey, Chunk>>>,
}

impl FsProvider {
    /// Resolve a path with two components (base & root),
    /// ensuring that the path is below the given root.
    async fn resolve_subpath<P: AsRef<Path>>(
        &self,
        root: &Path,
        path: P,
    ) -> Result<PathBuf, IoError> {
        let joined = root.join(&path);
        let joined = joined.clean();

        // Check components of either path
        let mut joined_abs_iter = joined.components();
        for root_part in root.components() {
            let joined_part = joined_abs_iter.next();

            // If the joined path is shorter or doesn't match
            // for the duration of the root, path is suspect
            if joined_part.is_none() || joined_part != Some(root_part) {
                return Err(IoError::new(
                    IoErrorKind::PermissionDenied,
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
}

impl Default for FsProvider {
    fn default() -> Self {
        FsProvider {
            config: Arc::new(RwLock::new(HashMap::new())),
            upload_chunks: Arc::new(RwLock::new(HashMap::new())),
            download_chunks: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl FsProvider {
    /// Get actor id string based on context value
    async fn get_actor_id(&self, ctx: &Context) -> ProviderInvocationResult<String> {
        let actor_id = match &ctx.actor {
            Some(id) => id.clone(),
            None => {
                return Err(ProviderInvocationError::Provider(String::from(
                    "No actor id found",
                )));
            }
        };
        Ok(actor_id)
    }

    async fn get_ld(&self, ctx: &Context) -> ProviderInvocationResult<LinkDefinition> {
        let actor_id = self.get_actor_id(ctx).await?;
        let conf_map = self.config.read().await;
        let conf = conf_map.get(&actor_id);
        let ld = match conf {
            Some(config) => config.ld.clone(),
            None => {
                return Err(ProviderInvocationError::Provider(String::from(
                    "No link definition found",
                )));
            }
        };
        Ok(ld)
    }

    async fn get_root(&self, ctx: &Context) -> ProviderInvocationResult<PathBuf> {
        let actor_id = self.get_actor_id(ctx).await?;
        let conf_map = self.config.read().await;
        let mut root = match conf_map.get(&actor_id) {
            Some(config) => config.root.clone(),
            None => {
                return Err(ProviderInvocationError::Provider(String::from(
                    "No root configuration found",
                )));
            }
        };
        root.push(actor_id.clone());
        Ok(root)
    }

    /// Stores a file chunk in right order.
    async fn store_chunk(
        &self,
        ctx: &Context,
        chunk: &Chunk,
        stream_id: &Option<String>,
    ) -> ProviderInvocationResult<()> {
        let root = self.get_root(ctx).await?;

        let container_dir = self.resolve_subpath(&root, &chunk.container_id).await?;
        let binary_file = self
            .resolve_subpath(&container_dir, &chunk.object_id)
            .await?;

        // create an empty file if it's the first chunk
        if chunk.offset == 0 {
            let resp = File::create(&binary_file);
            if resp.await.is_err() {
                let error_string = format!("Could not create file: {:?}", binary_file);
                error!("{:?}", &error_string);
                return Err(ProviderInvocationError::Provider(error_string));
            }
            if let Some(s_id) = stream_id {
                let mut upload_chunks = self.upload_chunks.write().await;
                let next_offset: u64 = 0;
                upload_chunks.insert(s_id.clone(), next_offset);
            } else if !chunk.is_last {
                return Err(ProviderInvocationError::Provider(
                    "Chunked storage is missing stream id".to_string(),
                ));
            }
        }

        // for continuing chunk storage, check that the chunk's offset matches the expected next one
        // which it should as theput_object calls are generated by an actor.
        if let Some(s_id) = stream_id {
            let mut upload_chunks = self.upload_chunks.write().await;
            let expected_offset = upload_chunks.get(s_id).unwrap();
            if *expected_offset != chunk.offset {
                return Err(ProviderInvocationError::Provider(format!(
                    "Chunk offset {} not the same as the expected offset: {}",
                    chunk.offset, *expected_offset
                )));
            }

            // Update the next expected offset
            let next_offset = if chunk.is_last {
                0u64
            } else {
                chunk.offset + chunk.bytes.len() as u64
            };
            upload_chunks.insert(s_id.clone(), next_offset);
        }

        let chunk_obj_subpath = Path::new(&chunk.container_id).join(&chunk.object_id);
        let chunk_obj_path = self.resolve_subpath(&root, &chunk_obj_subpath).await?;

        let mut file = OpenOptions::new()
            .create(false)
            .append(true)
            .open(chunk_obj_path)
            .await?;
        info!(
            "Receiving file chunk offset {} for {}/{}, size {}",
            chunk.offset,
            chunk.container_id,
            chunk.object_id,
            chunk.bytes.len()
        );

        let count = file.write(chunk.bytes.as_ref()).await?;
        if count != chunk.bytes.len() {
            let msg = format!(
                "Failed to fully write chunk: {} of {} bytes",
                count,
                chunk.bytes.len()
            );
            error!("{}", &msg);
            return Err(msg.into());
        }

        Ok(())
    }

    /// Sends bytes to actor in a single rpc message.
    /// If successful, returns number of bytes sent (same as chunk.content_length)
    #[allow(unused)]
    async fn send_chunk(&self, ctx: Context, chunk: Chunk) -> ProviderInvocationResult<u64> {
        info!(
            "Send chunk: container = {:?}, object = {:?}",
            chunk.container_id, chunk.object_id
        );

        let ld = self.get_ld(&ctx).await?;
        let receiver = InvocationHandler::new(&ld);

        let container_id = chunk.container_id.clone();
        let object_id = chunk.object_id.clone();
        let actor_id = &ld.actor_id;
        let chunk_len_bytes: u64 = chunk
            .bytes
            .len()
            .try_into()
            .map_err(|e| ProviderInvocationError::Provider(format!("failed to do: {e}")))?;

        let cr = receiver.receive_chunk(chunk).await
            .map_err(|e| ProviderInvocationError::Provider(format!(
                "sending chunk error: Container({container_id}) Object({object_id}) to Actor({actor_id}): {e:?}",
                )))?;

        Ok(if cr.cancel_download {
            0
        } else {
            chunk_len_bytes
        })
    }
}

#[async_trait]
impl WasmcloudCapabilityProvider for FsProvider {
    /// The fs provider has one configuration parameter, the root of the file system
    async fn put_link(&self, ld: &LinkDefinition) -> bool {
        for val in ld.values.iter() {
            info!("ld conf {:?}", val);
        }

        // Determine the root path value
        let root_val: PathBuf = match ld.values.iter().find(|(key, _)| key == "ROOT") {
            None => "/tmp".into(),
            Some((_, value)) => value.into(),
        };

        // Build configuration for FS Provider to use later
        let config = FsProviderConfig {
            ld: ld.clone(),
            root: root_val.clean(),
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
            .insert(ld.actor_id.clone(), config.clone());

        // Resolve the subpath from the root to the actor ID, carefully
        let actor_dir = match self.resolve_subpath(&config.root, &ld.actor_id).await {
            Ok(path) => path,
            Err(e) => {
                error!("Failed to resolve subpath to actor directory: {e}");
                return false;
            }
        };

        // Create directory for the individual actor
        match create_dir_all(actor_dir.as_path()).await {
            Ok(()) => true,
            Err(e) => {
                error!("Could not create actor directory: {:?}", e);
                false
            }
        }
    }

    async fn delete_link(&self, actor_id: &str) {
        self.config.write().await.remove(actor_id);
    }

    async fn shutdown(&self) {
        self.config.write().await.drain();
    }
}

/// Handle Factorial methods
#[async_trait]
impl WasmcloudBlobstoreBlobstore for FsProvider {
    /// Returns whether the container exists
    #[allow(unused)]
    async fn container_exists(
        &self,
        ctx: Context,
        container_id: ContainerId,
    ) -> ProviderInvocationResult<bool> {
        info!("Called container_exists({:?})", container_id);

        let root = self.get_root(&ctx).await?;
        let chunk_dir = self.resolve_subpath(&root, &container_id).await?;

        match read_dir(&chunk_dir).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Creates a container by name, returning success if it worked
    /// Note that container names may not be globally unique - just unique within the
    /// "namespace" of the connecting actor and linkdef
    async fn create_container(
        &self,
        ctx: Context,
        container_id: ContainerId,
    ) -> ProviderInvocationResult<()> {
        let root = self.get_root(&ctx).await?;
        let chunk_dir = self.resolve_subpath(&root, &container_id).await?;

        info!("create dir: {:?}", chunk_dir);

        match create_dir_all(chunk_dir).await {
            Ok(()) => Ok(()),
            Err(e) => Err(ProviderInvocationError::Provider(format!(
                "Could not create container: {:?}",
                e
            ))),
        }
    }

    /// Retrieves information about the container.
    /// Returns error if the container id is invalid or not found.
    #[allow(unused)]
    async fn get_container_info(
        &self,
        ctx: Context,
        container_id: ContainerId,
    ) -> ProviderInvocationResult<ContainerMetadata> {
        let root = self.get_root(&ctx).await?;
        let dir_path = self.resolve_subpath(&root, &container_id).await?;
        let dir_info = metadata(dir_path).await?;

        let modified = match dir_info.modified()?.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(s) => Timestamp {
                sec: s.as_secs(),
                nsec: 0u32,
            },
            Err(e) => return Err(ProviderInvocationError::Provider(format!("{:?}", e))),
        };

        Ok(ContainerMetadata {
            container_id: container_id.clone(),
            created_at: Some(modified),
        })
    }

    /// Returns list of container ids
    #[allow(unused)]
    async fn list_containers(
        &self,
        ctx: Context,
    ) -> ProviderInvocationResult<Vec<ContainerMetadata>> {
        let root = self.get_root(&ctx).await?;

        let containers = all_dirs(&root, &root, 0)
            .iter()
            .map(|c| ContainerMetadata {
                container_id: c.as_path().display().to_string(),
                created_at: None,
            })
            .collect();

        Ok(containers)
    }

    /// Empty and remove the container(s)
    /// The Vec<OperationResult> list contains one entry for each container
    /// that was not successfully removed, with the 'key' value representing the container name.
    /// If the Vec<OperationResult> list is empty, all container removals succeeded.
    #[allow(unused)]
    async fn remove_containers(
        &self,
        ctx: Context,
        arg: Vec<ContainerId>,
    ) -> ProviderInvocationResult<Vec<OperationResult>> {
        info!("Called remove_containers({:?})", arg);

        let root = self.get_root(&ctx).await?;

        let mut remove_errors = vec![];

        for cid in arg {
            let mut croot = root.clone();
            croot.push(&cid);

            if let Err(e) = remove_dir_all(&croot.as_path()).await {
                if read_dir(&croot.as_path()).await.is_ok() {
                    remove_errors.push(OperationResult {
                        error: Some(format!("{:?}", e.into_inner())),
                        key: cid.clone(),
                        success: true,
                    });
                }
            }
        }

        Ok(remove_errors)
    }

    /// Returns whether the object exists
    #[allow(unused)]
    async fn object_exists(
        &self,
        ctx: Context,
        container: ContainerObjectSelector,
    ) -> ProviderInvocationResult<bool> {
        info!("Called object_exists({:?})", container);

        let root = self.get_root(&ctx).await?;
        let file_subpath = Path::new(&container.container_id).join(&container.object_id);
        let file_path = self.resolve_subpath(&root, &file_subpath).await?;

        match File::open(file_path).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    /// Retrieves information about the object.
    /// Returns error if the object id is invalid or not found.
    #[allow(unused)]
    async fn get_object_info(
        &self,
        ctx: Context,
        container: ContainerObjectSelector,
    ) -> ProviderInvocationResult<ObjectMetadata> {
        info!("Called get_object_info({:?})", container);

        let root = self.get_root(&ctx).await?;
        let file_subpath = Path::new(&container.container_id).join(&container.object_id);
        let file_path = self.resolve_subpath(&root, &file_subpath).await?;

        let metadata = metadata(file_path).await?;

        let modified = match metadata.modified()?.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(s) => Timestamp {
                sec: s.as_secs(),
                nsec: 0u32,
            },
            Err(e) => return Err(ProviderInvocationError::Provider(format!("{:?}", e))),
        };

        Ok(ObjectMetadata {
            container_id: container.container_id.clone(),
            content_encoding: None,
            content_length: metadata.len(),
            content_type: None,
            last_modified: Some(modified),
            object_id: container.object_id.clone(),
        })
    }

    /// Lists the objects in the container.
    /// If the container exists and is empty, the returned `objects` list is empty.
    /// Parameters of the request may be used to limit the object names returned
    /// with an optional start value, end value, and maximum number of items.
    /// The provider may limit the number of items returned. If the list is truncated,
    /// the response contains a `continuation` token that may be submitted in
    /// a subsequent ListObjects request.
    ///
    /// Optional object metadata fields (i.e., `contentType` and `contentEncoding`) may not be
    /// filled in for ListObjects response. To get complete object metadata, use GetObjectInfo.
    /// Currently ignoring need for pagination
    #[allow(unused)]
    async fn list_objects(
        &self,
        ctx: Context,
        req: ListObjectsRequest,
    ) -> ProviderInvocationResult<ListObjectsResponse> {
        info!("Called list_objects({:?})", req);

        let root = self.get_root(&ctx).await?;
        let chunk_dir = self.resolve_subpath(&root, &req.container_id).await?;

        let mut objects = Vec::new();

        let mut entries = read_dir(&chunk_dir).await?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();

            if !path.is_dir() {
                let file_name = match entry.file_name().into_string() {
                    Ok(name) => name,
                    Err(_) => {
                        return Err(ProviderInvocationError::Provider(String::from(
                            "File name conversion failed",
                        )));
                    }
                };

                let modified = match entry
                    .metadata()
                    .await?
                    .modified()?
                    .duration_since(SystemTime::UNIX_EPOCH)
                {
                    Ok(s) => Timestamp {
                        sec: s.as_secs(),
                        nsec: 0u32,
                    },
                    Err(e) => return Err(ProviderInvocationError::Provider(format!("{:?}", e))),
                };

                objects.push(ObjectMetadata {
                    container_id: req.container_id.clone(),
                    content_encoding: None,
                    content_length: entry.metadata().await?.len(),
                    content_type: None,
                    last_modified: Some(modified),
                    object_id: file_name,
                });
            }
        }

        Ok(ListObjectsResponse {
            continuation: None,
            is_last: true,
            objects,
        })
    }

    /// Removes the objects. In the event any of the objects cannot be removed,
    /// the operation continues until all requested deletions have been attempted.
    /// The MultiRequest includes a list of errors, one for each deletion request
    /// that did not succeed. If the list is empty, all removals succeeded.
    #[allow(unused)]
    async fn remove_objects(
        &self,
        ctx: Context,
        arg: RemoveObjectsRequest,
    ) -> ProviderInvocationResult<Vec<OperationResult>> {
        info!("Invoked remove objects: {:?}", arg);
        let root = self.get_root(&ctx).await?;

        let mut errors = Vec::new();

        for object in &arg.objects {
            let object_subpath = Path::new(&arg.container_id).join(object);
            let object_path = self.resolve_subpath(&root, object_subpath).await?;

            if let Err(e) = remove_file(object_path.as_path()).await {
                errors.push(OperationResult {
                    error: Some(format!("{:?}", e)),
                    key: format!("{:?}", object_path),
                    success: false,
                })
            }
        }

        Ok(errors)
    }

    /// Requests to start upload of a file/blob to the Blobstore.
    /// It is recommended to keep chunks under 1MB to avoid exceeding nats default message size
    #[allow(unused)]
    async fn put_object(
        &self,
        ctx: Context,
        arg: PutObjectRequest,
    ) -> ProviderInvocationResult<PutObjectResponse> {
        info!(
            "Called put_object(): container={:?}, object={:?}",
            arg.chunk.container_id, arg.chunk.object_id
        );

        if arg.chunk.bytes.is_empty() {
            error!("put_object with zero bytes");
            return Err(ProviderInvocationError::Provider(
                "cannot put zero-length objects".to_string(),
            ));
        }

        let stream_id = if arg.chunk.is_last {
            None
        } else {
            Some(format!(
                "{}+{}+{}",
                self.get_actor_id(&ctx).await?,
                arg.chunk.container_id,
                arg.chunk.object_id
            ))
        };

        // store the chunks in order
        self.store_chunk(&ctx, &arg.chunk, &stream_id).await?;

        Ok(PutObjectResponse { stream_id })
    }

    /// Uploads a file chunk to a blobstore. This must be called AFTER PutObject
    /// It is recommended to keep chunks under 1MB to avoid exceeding nats default message size
    #[allow(unused)]
    async fn put_chunk(&self, ctx: Context, arg: PutChunkRequest) -> ProviderInvocationResult<()> {
        info!("Called put_chunk: {:?}", arg);

        // In the simplest case we can simply store the chunk (happy path)
        if !arg.cancel_and_remove {
            self.store_chunk(&ctx, &arg.chunk, &arg.stream_id).await?;
            return Ok(());
        }

        // Determine the path to the file
        let root = &self.get_root(&ctx).await?;
        let file_subpath = Path::new(&arg.chunk.container_id).join(&arg.chunk.object_id);
        let file_path = self.resolve_subpath(root, &file_subpath).await?;

        // Remove the file
        remove_file(file_path.as_path()).await.map_err(|e| {
            ProviderInvocationError::Provider(format!(
                "Could not cancel and remove file: {:?}",
                file_path
            ))
        })
    }

    /// Requests to retrieve an object. If the object is large, the provider
    /// may split the response into multiple parts
    /// It is recommended to keep chunks under 1MB to avoid exceeding nats default message size
    async fn get_object(
        &self,
        ctx: Context,
        req: GetObjectRequest,
    ) -> ProviderInvocationResult<GetObjectResponse> {
        info!("Called get_object: {:?}", req);

        // Determine path to object file
        let root = &self.get_root(&ctx).await?;
        let object_subpath = Path::new(&req.container_id).join(&req.object_id);
        let file_path = self.resolve_subpath(root, &object_subpath).await?;

        // Read the file in
        let file = read(file_path).await?;

        let start_offset = match req.range_start {
            Some(o) => o as usize,
            None => 0,
        };

        let end_offset = match req.range_end {
            Some(o) => std::cmp::min(o as usize + 1, file.len()),
            None => file.len(),
        };

        let mut _dcm = self.download_chunks.write().await;
        let _actor_id = self.get_actor_id(&ctx).await?;
        let slice = &file[start_offset..end_offset];

        info!(
            "Retriving chunk start offset: {}, end offset: {} (exclusive)",
            start_offset, end_offset
        );

        let chunk = Chunk {
            object_id: req.object_id.clone(),
            container_id: req.container_id.clone(),
            bytes: slice.to_vec(),
            offset: start_offset as u64,
            is_last: end_offset >= file.len(),
        };

        Ok(GetObjectResponse {
            content_encoding: None,
            content_length: chunk.bytes.len() as u64,
            content_type: None,
            error: None,
            initial_chunk: Some(chunk),
            success: true,
        })
    }

    fn contract_id() -> &'static str {
        "wasmcloud:blobstore"
    }
}

#[cfg(test)]
mod tests {
    use super::FsProvider;
    use std::io::ErrorKind as IoErrorKind;
    use std::path::PathBuf;

    /// Ensure that only safe subpaths are resolved
    #[tokio::test]
    async fn resolve_safe_samepath() {
        let provider = FsProvider::default();
        assert!(provider
            .resolve_subpath(&PathBuf::from("./"), "./././")
            .await
            .is_ok());
    }

    /// Ensure that ancestor paths are not allowed to be resolved as subpaths
    #[tokio::test]
    async fn resolve_fail_ancestor() {
        let provider = FsProvider::default();
        let res = provider
            .resolve_subpath(&PathBuf::from("./"), "../")
            .await
            .unwrap_err();
        assert_eq!(res.kind(), IoErrorKind::PermissionDenied);
    }
}

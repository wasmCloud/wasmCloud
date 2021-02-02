//! # S3 implementation of the wasmCloud blob store capability provider API
//!
//! Provides an implementation of the wasmcloud:blobstore contract for S3 and
//! S3-compatible (e.g. Minio) products.

#[macro_use]
extern crate wascc_codec as codec;
#[macro_use]
extern crate log;

use actor_blobstore::*;
use actor_core::CapabilityConfiguration;
use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use codec::core::{OP_BIND_ACTOR, OP_REMOVE_ACTOR};
use codec::{deserialize, serialize};
use rusoto_s3::S3Client;
use std::error::Error;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
mod s3;

#[cfg(not(feature = "static_plugin"))]
capability_provider!(S3Provider, S3Provider::new);

const CAPABILITY_ID: &str = "wasmcloud:blobstore";
const SYSTEM_ACTOR: &str = "system";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REVISION: u32 = 2; // Increment for each crates publish

#[derive(Debug, PartialEq)]
struct FileUpload {
    container: String,
    id: String,
    total_bytes: u64,
    expected_chunks: u64,
    chunks: Vec<FileChunk>,
}

impl FileUpload {
    pub fn is_complete(&self) -> bool {
        self.chunks.len() == self.expected_chunks as usize
    }
}

/// AWS S3 implementation of the `wascc:blobstore` specification
#[derive(Clone)]
pub struct S3Provider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    clients: Arc<RwLock<HashMap<String, Arc<S3Client>>>>,
    uploads: Arc<RwLock<HashMap<String, FileUpload>>>,
}

impl Default for S3Provider {
    fn default() -> Self {
        match env_logger::try_init() {
            Ok(_) => {}
            Err(_) => {}
        }

        S3Provider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            clients: Arc::new(RwLock::new(HashMap::new())),
            uploads: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl S3Provider {
    /// Creates a new S3 provider
    pub fn new() -> Self {
        Self::default()
    }

    fn configure(
        &self,
        config: CapabilityConfiguration,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        self.clients.write().unwrap().insert(
            config.module.clone(),
            Arc::new(s3::client_for_config(&config)?),
        );

        Ok(vec![])
    }
    fn deconfigure(&self, actor: &str) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        self.clients.write().unwrap().remove(actor);

        Ok(vec![])
    }

    fn create_container(
        &self,
        actor: &str,
        container: Container,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(s3::create_bucket(
            &self.clients.read().unwrap()[actor],
            &container.id,
        ))?;

        Ok(vec![])
    }

    fn remove_container(
        &self,
        actor: &str,
        container: Container,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(s3::remove_bucket(
            &self.clients.read().unwrap()[actor],
            &container.id,
        ))?;

        Ok(vec![])
    }

    fn upload_chunk(
        &self,
        actor: &str,
        chunk: FileChunk,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let key = upload_key(&chunk.container.id, &chunk.id, &actor);
        self.uploads
            .write()
            .unwrap()
            .entry(key.clone())
            .and_modify(|u| {
                u.chunks.push(chunk);
            });
        let complete = self.uploads.read().unwrap()[&key].is_complete();
        if complete {
            let mut rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(s3::complete_upload(
                &self.clients.read().unwrap()[actor],
                &self.uploads.read().unwrap()[&key],
            ))?;
            self.uploads.write().unwrap().remove(&key);
        }
        Ok(vec![])
    }

    fn start_upload(
        &self,
        actor: &str,
        chunk: FileChunk,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let key = upload_key(&chunk.container.id, &chunk.id, &actor);

        let upload = FileUpload {
            chunks: vec![],
            container: chunk.container.id,
            id: chunk.id.to_string(),
            total_bytes: chunk.total_bytes,
            expected_chunks: expected_chunks(chunk.total_bytes, chunk.chunk_size),
        };

        self.uploads.write().unwrap().insert(key, upload);

        Ok(vec![])
    }

    fn remove_object(
        &self,
        actor: &str,
        blob: Blob,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(s3::remove_object(
            &self.clients.read().unwrap()[actor],
            &blob.container.id,
            &blob.id,
        ))?;

        Ok(vec![])
    }

    fn get_object_info(
        &self,
        actor: &str,
        blob: Blob,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut rt = tokio::runtime::Runtime::new().unwrap();
        let info = rt.block_on(s3::head_object(
            &self.clients.read().unwrap()[actor],
            &blob.container.id,
            &blob.id,
        ));

        let blob = if let Ok(ob) = info {
            Blob {
                id: blob.id.to_string(),
                container: blob.container,
                byte_size: ob.content_length.unwrap() as u64,
            }
        } else {
            Blob {
                id: "none".to_string(),
                container: Container::new("none".to_string()),
                byte_size: 0,
            }
        };

        Ok(serialize(&blob)?)
    }

    fn list_objects(
        &self,
        actor: &str,
        container: Container,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut rt = tokio::runtime::Runtime::new().unwrap();
        let objects = rt.block_on(s3::list_objects(
            &self.clients.read().unwrap()[actor],
            &container.id,
        ))?;
        let blobs = if let Some(v) = objects {
            v.iter()
                .map(|ob| Blob {
                    id: ob.key.clone().unwrap(),
                    container: container.clone(),
                    byte_size: ob.size.unwrap() as u64,
                })
                .collect()
        } else {
            vec![]
        };
        let bloblist = BlobList { blobs };
        Ok(serialize(&bloblist)?)
    }

    fn start_download(
        &self,
        actor: &str,
        request: StreamRequest,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let actor = actor.to_string();

        let d = self.dispatcher.clone();
        let c = self.clients.read().unwrap()[&actor].clone();
        let container = request.container.id.to_string();
        let chunk_size = request.chunk_size;
        let id = request.id.to_string();
        let ctx = request.context.clone();

        let byte_size = {
            let mut rt = tokio::runtime::Runtime::new().unwrap();
            let info = rt.block_on(s3::head_object(&c, &container, &id)).unwrap();
            drop(rt);
            info.content_length.unwrap() as u64
        };

        std::thread::spawn(move || {
            let actor = actor.to_string();

            let chunk_count = expected_chunks(byte_size, chunk_size);
            let mut rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                for idx in 0..chunk_count {
                    dispatch_chunk(
                        idx,
                        d.clone(),
                        c.clone(),
                        container.to_string(),
                        id.to_string(),
                        chunk_size,
                        byte_size,
                        actor.clone(),
                        ctx.clone(),
                    )
                    .await;
                }
            });
        });

        Ok(vec![])
    }
}

async fn dispatch_chunk(
    idx: u64,
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    client: Arc<S3Client>,
    container: String,
    id: String,
    chunk_size: u64,
    byte_size: u64,
    actor: String,
    context: Option<String>,
) {
    // range header spec: https://www.w3.org/Protocols/rfc2616/rfc2616-sec14.html#sec14.35
    // tl;dr - ranges are _inclusive_, but start at 0.
    // idx 0, start 0, end chunk_size-1
    let start = idx * chunk_size;
    let mut end = start + chunk_size - 1;
    if end > byte_size {
        end = byte_size - 1;
    }

    let bytes = s3::get_blob_range(&client, &container, &id, start, end)
        .await
        .unwrap();

    let fc = FileChunk {
        sequence_no: idx + 1,
        container: Container::new(container),
        id,
        chunk_size,
        total_bytes: byte_size,
        chunk_bytes: bytes,
        context,
    };
    match dispatcher
        .read()
        .unwrap()
        .dispatch(&actor, OP_RECEIVE_CHUNK, &serialize(&fc).unwrap())
    {
        Ok(_) => {}
        Err(_) => error!("Failed to dispatch block to actor {}", actor),
    }
}

impl CapabilityProvider for S3Provider {
    // Invoked by the runtime host to give this provider plugin the ability to communicate
    // with actors
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn Dispatcher>,
    ) -> Result<(), Box<dyn Error + Sync + Send>> {
        trace!("Dispatcher received.");
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    // Invoked by host runtime to allow an actor to make use of the capability
    // All providers MUST handle the "configure" message, even if no work will be done
    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        trace!("Received host call from {}, operation - {}", actor, op);

        match op {
            OP_BIND_ACTOR if actor == SYSTEM_ACTOR => self.configure(deserialize(msg)?),
            OP_REMOVE_ACTOR if actor == SYSTEM_ACTOR => self.deconfigure(actor),
            OP_CREATE_CONTAINER => self.create_container(actor, deserialize(msg)?),
            OP_REMOVE_CONTAINER => self.remove_container(actor, deserialize(msg)?),
            OP_REMOVE_OBJECT => self.remove_object(actor, deserialize(msg)?),
            OP_LIST_OBJECTS => self.list_objects(actor, deserialize(msg)?),
            OP_UPLOAD_CHUNK => self.upload_chunk(actor, deserialize(msg)?),
            OP_START_DOWNLOAD => self.start_download(actor, deserialize(msg)?),
            OP_START_UPLOAD => self.start_upload(actor, deserialize(msg)?),
            OP_GET_OBJECT_INFO => self.get_object_info(actor, deserialize(msg)?),

            _ => Err("bad dispatch".into()),
        }
    }

    /// No cleanup needed
    fn stop(&self) {
        ()
    }
}

fn expected_chunks(total_bytes: u64, chunk_size: u64) -> u64 {
    let mut chunks = total_bytes / chunk_size;
    if total_bytes % chunk_size != 0 {
        chunks = chunks + 1
    }
    chunks
}

fn upload_key(container: &str, blob_id: &str, actor: &str) -> String {
    format!("{}-{}-{}", actor, container, blob_id)
}

#[cfg(test)]
mod test {
    use super::*;
    use crossbeam_utils::sync::WaitGroup;
    use std::collections::HashMap;

    // ***! These tests MUST be run in the presence of a minio server
    // The easiest option is just to run the default minio docker image as a
    // service. The following command will get you up and running for these tests.
    //
    // (Don't use this setup for production)
    // docker run -p 9000:9000 --name minio \
    //   --env MINIO_ACCESS_KEY="minioadmin" \
    //   --env MINIO_SECRET_KEY="minioadmin" bitnami/minio:latest

    #[test]
    fn test_create_and_remove_bucket() {
        let provider = S3Provider::new();
        provider.configure(gen_config("testar")).unwrap();
        let container = Container {
            id: "addremovebucket".to_string(),
        };
        let res = provider.handle_call(
            "testar",
            OP_CREATE_CONTAINER,
            &serialize(&container).unwrap(),
        );
        assert!(res.is_ok());
        let res2 = provider.handle_call(
            "testar",
            OP_REMOVE_CONTAINER,
            &serialize(container).unwrap(),
        );
        assert!(res2.is_ok());
    }

    #[test]
    fn test_upload_and_download() {
        let provider = S3Provider::new();
        provider.configure(gen_config("testupanddown")).unwrap();
        let wg = WaitGroup::new();
        let dispatcher = Box::new(TestDispatcher::new(wg.clone(), expected_chunks(10427, 100)));
        provider.configure_dispatch(dispatcher).unwrap();

        let container = Container {
            id: "updownbucket".to_string(),
        };
        let _res = provider.handle_call(
            "testupanddown",
            OP_CREATE_CONTAINER,
            &serialize(&container).unwrap(),
        );

        let mut data: Vec<u8> = Vec::new();
        for _ in 0..10427 {
            data.push(42);
        }

        let chunk_list: Vec<FileChunk> = data
            .chunks(100)
            .enumerate()
            .map(|(idx, v)| FileChunk {
                chunk_bytes: v.to_vec(),
                chunk_size: 100,
                container: Container::new("updownbucket".to_string()),
                id: "updowntestfile".to_string(),
                total_bytes: data.len() as u64,
                sequence_no: idx as u64 + 1,
                context: None,
            })
            .collect();

        let first_chunk = FileChunk {
            chunk_bytes: vec![],
            chunk_size: 100,
            container: Container::new("updownbucket".to_string()),
            id: "updowntestfile".to_string(),
            total_bytes: data.len() as u64,
            sequence_no: 0,
            context: None,
        };

        let _ = provider
            .handle_call(
                "testupanddown",
                OP_START_UPLOAD,
                &serialize(&first_chunk).unwrap(),
            )
            .unwrap();

        for chunk in chunk_list {
            let _ = provider
                .handle_call("testupanddown", OP_UPLOAD_CHUNK, &serialize(chunk).unwrap())
                .unwrap();
        }
        let req = StreamRequest {
            chunk_size: 100,
            container: Container::new("updownbucket".to_string()),
            id: "updowntestfile".to_string(),
            context: Some("test1".to_string()),
        };
        let _ = provider
            .handle_call(
                "testupanddown",
                OP_START_DOWNLOAD,
                &serialize(&req).unwrap(),
            )
            .unwrap();

        wg.wait();
        assert!(true);
    }

    #[test]
    fn test_upload() {
        let provider = S3Provider::new();
        provider.configure(gen_config("testupload")).unwrap();

        let container = Container {
            id: "uploadbucket".to_string(),
        };
        let _res = provider.handle_call(
            "testupload",
            OP_CREATE_CONTAINER,
            &serialize(&container).unwrap(),
        );

        let mut data: Vec<u8> = Vec::new();
        for _ in 0..10427 {
            data.push(42);
        }

        let chunk_list: Vec<FileChunk> = data
            .chunks(100)
            .enumerate()
            .map(|(idx, v)| FileChunk {
                chunk_bytes: v.to_vec(),
                chunk_size: 100,
                container: Container::new("uploadbucket".to_string()),
                id: "testfile".to_string(),
                total_bytes: data.len() as u64,
                sequence_no: idx as u64 + 1,
                context: None,
            })
            .collect();

        let first_chunk = FileChunk {
            chunk_bytes: vec![],
            chunk_size: 100,
            container: Container::new("uploadbucket".to_string()),
            id: "testfile".to_string(),
            total_bytes: data.len() as u64,
            sequence_no: 0,
            context: None,
        };

        let _ = provider.handle_call(
            "testupload",
            OP_START_UPLOAD,
            &serialize(&first_chunk).unwrap(),
        );

        for chunk in chunk_list {
            let _ = provider.handle_call("testupload", OP_UPLOAD_CHUNK, &serialize(chunk).unwrap());
        }

        let list = provider
            .handle_call(
                "testupload",
                OP_LIST_OBJECTS,
                &serialize(&container).unwrap(),
            )
            .unwrap();
        let object_list: BlobList = deserialize(&list).unwrap();
        assert_eq!(1, object_list.blobs.len());
        assert_eq!("testfile", object_list.blobs[0].id);

        let blob = Blob {
            container: Container::new("uploadbucket".to_string()),
            id: "testfile".to_string(),
            byte_size: 0,
        };

        let info = provider
            .handle_call("testupload", OP_GET_OBJECT_INFO, &serialize(&blob).unwrap())
            .unwrap();
        let objinfo: Blob = deserialize(&info).unwrap();
        assert_eq!(10427, objinfo.byte_size);
        let _ = provider
            .handle_call("testupload", OP_REMOVE_OBJECT, &serialize(&blob).unwrap())
            .unwrap();
        let _ = provider
            .handle_call(
                "testupload",
                OP_REMOVE_CONTAINER,
                &serialize(&container).unwrap(),
            )
            .unwrap();
    }

    fn gen_config(module: &str) -> CapabilityConfiguration {
        CapabilityConfiguration {
            module: module.to_string(),
            values: minio_config(),
        }
    }

    fn minio_config() -> HashMap<String, String> {
        let mut hm = HashMap::new();
        hm.insert("ENDPOINT".to_string(), "http://localhost:9000".to_string());
        hm.insert("REGION".to_string(), "us-east-1".to_string());
        hm.insert("AWS_ACCESS_KEY".to_string(), "minioadmin".to_string());
        hm.insert(
            "AWS_SECRET_ACCESS_KEY".to_string(),
            "minioadmin".to_string(),
        );

        hm
    }

    struct TestDispatcher {
        chunks: RwLock<Vec<FileChunk>>,
        wg: RwLock<Option<WaitGroup>>,
        expected_chunks: u64,
    }

    impl TestDispatcher {
        fn new(wg: WaitGroup, expected_chunks: u64) -> TestDispatcher {
            TestDispatcher {
                chunks: RwLock::new(vec![]),
                wg: RwLock::new(Some(wg)),
                expected_chunks,
            }
        }
    }

    impl Dispatcher for TestDispatcher {
        fn dispatch(
            &self,
            _actor: &str,
            _op: &str,
            msg: &[u8],
        ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
            let fc: FileChunk = deserialize(msg)?;
            self.chunks.write().unwrap().push(fc);
            if self.chunks.read().unwrap().len() == self.expected_chunks as usize {
                *self.wg.write().unwrap() = None;
            }
            Ok(vec![])
        }
    }
}

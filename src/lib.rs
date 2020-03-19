#[macro_use]
extern crate wascc_codec as codec;

#[macro_use]
extern crate log;

use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use codec::core::OP_CONFIGURE;
use codec::deserialize;
use codec::{blobstore::*, serialize};
use rusoto_s3::S3Client;
use std::error::Error;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use tokio::task;
use wascc_codec::core::CapabilityConfiguration;

mod s3;

capability_provider!(S3Provider, S3Provider::new);

const CAPABILITY_ID: &str = "wascc:blobstore";

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

pub struct S3Provider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    clients: RwLock<HashMap<String, Arc<S3Client>>>,
    uploads: RwLock<HashMap<String, FileUpload>>,
}

impl Default for S3Provider {
    fn default() -> Self {
        match env_logger::try_init() {
            Ok(_) => {}
            Err(_) => {}
        }

        S3Provider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            clients: RwLock::new(HashMap::new()),
            uploads: RwLock::new(HashMap::new()),
        }
    }
}

impl S3Provider {
    pub fn new() -> Self {
        Self::default()
    }

    fn configure(&self, config: CapabilityConfiguration) -> Result<Vec<u8>, Box<dyn Error>> {
        self.clients.write().unwrap().insert(
            config.module.clone(),
            Arc::new(s3::client_for_config(&config)?),
        );

        Ok(vec![])
    }

    async fn create_container(
        &self,
        actor: &str,
        container: Container,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        s3::create_bucket(&self.clients.read().unwrap()[actor], &container.id).await?;

        Ok(vec![])
    }

    async fn remove_container(
        &self,
        actor: &str,
        container: Container,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        s3::remove_bucket(&self.clients.read().unwrap()[actor], &container.id).await?;

        Ok(vec![])
    }

    async fn upload_chunk(&self, actor: &str, chunk: FileChunk) -> Result<Vec<u8>, Box<dyn Error>> {
        let key = upload_key(&chunk.container, &chunk.id, &actor);
        self.uploads
            .write()
            .unwrap()
            .entry(key.clone())
            .and_modify(|u| {
                u.chunks.push(chunk);
            });
        let complete = self.uploads.read().unwrap()[&key].is_complete();
        if complete {
            s3::complete_upload(
                &self.clients.read().unwrap()[actor],
                &self.uploads.read().unwrap()[&key],
            )
            .await?;
            self.uploads.write().unwrap().remove(&key);
        }
        Ok(vec![])
    }

    fn start_upload(&self, actor: &str, chunk: FileChunk) -> Result<Vec<u8>, Box<dyn Error>> {
        let key = upload_key(&chunk.container, &chunk.id, &actor);

        let upload = FileUpload {
            chunks: vec![],
            container: chunk.container.to_string(),
            id: chunk.id.to_string(),
            total_bytes: chunk.total_bytes,
            expected_chunks: expected_chunks(chunk.total_bytes, chunk.chunk_size),
        };

        self.uploads.write().unwrap().insert(key, upload);

        Ok(vec![])
    }

    async fn remove_object(&self, actor: &str, blob: Blob) -> Result<Vec<u8>, Box<dyn Error>> {
        s3::remove_object(
            &self.clients.read().unwrap()[actor],
            &blob.container,
            &blob.id,
        )
        .await?;

        Ok(vec![])
    }

    async fn get_object_info(&self, actor: &str, blob: Blob) -> Result<Vec<u8>, Box<dyn Error>> {
        let info = s3::head_object(
            &self.clients.read().unwrap()[actor],
            &blob.container,
            &blob.id,
        )
        .await;

        let blob = if let Ok(ob) = info {
            Blob {
                id: blob.id.to_string(),
                container: blob.container.to_string(),
                byte_size: ob.content_length.unwrap() as u64,
            }
        } else {
            Blob {
                id: "none".to_string(),
                container: "none".to_string(),
                byte_size: 0,
            }
        };

        Ok(serialize(&blob)?)
    }

    async fn list_objects(
        &self,
        actor: &str,
        container: Container,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let objects = s3::list_objects(&self.clients.read().unwrap()[actor], &container.id).await?;
        let blobs = if let Some(v) = objects {
            v.iter()
                .map(|ob| Blob {
                    id: ob.key.clone().unwrap(),
                    container: container.id.to_string(),
                    byte_size: ob.size.unwrap() as u64,
                })
                .collect()
        } else {
            vec![]
        };
        let bloblist = BlobList { blobs };
        Ok(serialize(&bloblist)?)
    }

    async fn start_download(
        &self,
        actor: &str,
        request: StreamRequest,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let obj = self
            .get_object_info(
                actor,
                Blob {
                    byte_size: 0,
                    container: request.container.to_string(),
                    id: request.id.to_string(),
                },
            )
            .await?;
        let blob: Blob = deserialize(&obj)?;

        //TODO: figure out how to get rid of the ceremony of duplicating all this
        //stuff to make the borrow checker happy for the task spawn
        let actor = actor.to_string();
        let client = self.clients.read().unwrap()[&actor].clone();
        let d = self.dispatcher.clone();
        let container = blob.container.to_string();
        let id = blob.id.to_string();
        let chunk_size = request.chunk_size;
        let byte_size = blob.byte_size;
        let chunk_count = expected_chunks(byte_size, chunk_size);

        ::std::thread::spawn(move || {
            for idx in 0..chunk_count {
                println!("HERE");
                dispatch_chunk(
                    idx,
                    d.clone(),
                    client.clone(),
                    container.to_string(),
                    id.to_string(),
                    chunk_size,
                    chunk_count,
                    actor.clone(),
                );
            }
        });

        Ok(vec![])
    }
}

fn dispatch_chunk(
    idx: u64,
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    client: Arc<S3Client>,
    container: String,
    id: String,
    chunk_size: u64,
    byte_size: u64,
    actor: String,
) {
    let start = idx * chunk_size;
    let end = start + chunk_size;
    use tokio::runtime::Runtime;
    let mut runtime = Runtime::new().unwrap();
    let bytes = runtime
        .block_on(s3::get_blob_range(&client, &container, &id, start, end))
        .unwrap();
    let fc = FileChunk {
        sequence_no: idx + 1,
        container,
        id,
        chunk_size,
        total_bytes: byte_size,
        chunk_bytes: bytes,
    };
    match dispatcher.read().unwrap().dispatch(
        &format!("{}!{}", actor, OP_RECEIVE_CHUNK),
        &serialize(&fc).unwrap(),
    ) {
        Ok(_) => {}
        Err(_) => error!("Failed to dispatch block to actor {}", actor),
    }
}

impl CapabilityProvider for S3Provider {
    fn capability_id(&self) -> &'static str {
        CAPABILITY_ID
    }

    // Invoked by the runtime host to give this provider plugin the ability to communicate
    // with actors
    fn configure_dispatch(&self, dispatcher: Box<dyn Dispatcher>) -> Result<(), Box<dyn Error>> {
        trace!("Dispatcher received.");
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    fn name(&self) -> &'static str {
        "S3 Blob Store"
    }

    // Invoked by host runtime to allow an actor to make use of the capability
    // All providers MUST handle the "configure" message, even if no work will be done
    #[tokio::main]
    async fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        trace!("Received host call from {}, operation - {}", actor, op);

        match op {
            OP_CONFIGURE if actor == "system" => self.configure(deserialize(msg)?),
            OP_CREATE_CONTAINER => self.create_container(actor, deserialize(msg)?).await,
            OP_REMOVE_CONTAINER => self.remove_container(actor, deserialize(msg)?).await,
            OP_REMOVE_OBJECT => self.remove_object(actor, deserialize(msg)?).await,
            OP_LIST_OBJECTS => self.list_objects(actor, deserialize(msg)?).await,
            OP_UPLOAD_CHUNK => self.upload_chunk(actor, deserialize(msg)?).await,
            OP_START_DOWNLOAD => self.start_download(actor, deserialize(msg)?).await,
            OP_START_UPLOAD => self.start_upload(actor, deserialize(msg)?),
            OP_GET_OBJECT_INFO => self.get_object_info(actor, deserialize(msg)?).await,

            _ => Err("bad dispatch".into()),
        }
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
                container: "updownbucket".to_string(),
                id: "updowntestfile".to_string(),
                total_bytes: data.len() as u64,
                sequence_no: idx as u64 + 1,
            })
            .collect();

        let first_chunk = FileChunk {
            chunk_bytes: vec![],
            chunk_size: 100,
            container: "updownbucket".to_string(),
            id: "updowntestfile".to_string(),
            total_bytes: data.len() as u64,
            sequence_no: 0,
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
            container: "updownbucket".to_string(),
            id: "updowntestfile".to_string(),
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
                container: "uploadbucket".to_string(),
                id: "testfile".to_string(),
                total_bytes: data.len() as u64,
                sequence_no: idx as u64 + 1,
            })
            .collect();

        let first_chunk = FileChunk {
            chunk_bytes: vec![],
            chunk_size: 100,
            container: "uploadbucket".to_string(),
            id: "testfile".to_string(),
            total_bytes: data.len() as u64,
            sequence_no: 0,
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
            container: "uploadbucket".to_string(),
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
        fn dispatch(&self, _op: &str, msg: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
            info!("Received dispatch");
            let fc: FileChunk = deserialize(msg)?;
            self.chunks.write().unwrap().push(fc);
            if self.chunks.read().unwrap().len() == self.expected_chunks as usize {
                *self.wg.write().unwrap() = None;
            }
            Ok(vec![])
        }
    }
}

#[macro_use]
extern crate wascc_codec as codec;

#[macro_use]
extern crate log;

use codec::blobstore::*;
use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use codec::core::OP_CONFIGURE;
use prost::Message;
use wascc_codec::core::CapabilityConfiguration;

use chunks::Chunks;
use std::error::Error;
use std::io::Write;
use std::{    
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

mod chunks;

capability_provider!(FileSystemProvider, FileSystemProvider::new);

const CAPABILITY_ID: &str = "wascc:blobstore";

pub struct FileSystemProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    rootdir: RwLock<PathBuf>,
}

impl Default for FileSystemProvider {
    fn default() -> Self {
        env_logger::init();

        FileSystemProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            rootdir: RwLock::new(PathBuf::new()),
        }
    }
}

impl FileSystemProvider {
    pub fn new() -> Self {
        Self::default()
    }

    fn configure(
        &self,
        config: impl Into<CapabilityConfiguration>,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let config = config.into();

        let mut lock = self.rootdir.write().unwrap();
        let root_dir = config.values["ROOT"].clone();
        info!("File System Blob Store Container Root: '{}'", root_dir);
        *lock = PathBuf::from(root_dir);

        Ok(vec![])
    }

    fn create_container(
        &self,
        _actor: &str,
        container: Container,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let cdir = self.container_to_path(&container);
        std::fs::create_dir_all(cdir)?;
        Ok(vec![])
    }

    fn remove_container(
        &self,
        _actor: &str,
        container: Container,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let cdir = self.container_to_path(&container);
        std::fs::remove_dir(cdir)?;
        Ok(vec![])
    }

    fn start_upload(&self, _actor: &str, blob: FileChunk) -> Result<Vec<u8>, Box<dyn Error>> {
        let blob = Blob {
            byte_size: 0,
            id: blob.id,
            container: blob.container,
        };
        info!("Starting upload: {}/{}", blob.container, blob.id);
        let bfile = self.blob_to_path(&blob);
        std::fs::write(bfile, &[])?;
        Ok(vec![])
    }

    fn remove_object(&self, _actor: &str, blob: Blob) -> Result<Vec<u8>, Box<dyn Error>> {
        let bfile = self.blob_to_path(&blob);
        std::fs::remove_file(&bfile)?;
        Ok(vec![])
    }

    fn get_object_info(&self, _actor: &str, blob: Blob) -> Result<Vec<u8>, Box<dyn Error>> {
        let bfile = self.blob_to_path(&blob);
        let blob: Blob = if bfile.exists() {
            Blob {
                id: blob.id,
                container: blob.container,
                byte_size: bfile.metadata().unwrap().len(),
            }
        } else {
            Blob::default()
        };
        let mut buf = Vec::new();
        blob.encode(&mut buf)?;
        Ok(buf)
    }

    fn list_objects(&self, _actor: &str, container: Container) -> Result<Vec<u8>, Box<dyn Error>> {
        let cpath = self.container_to_path(&container);
        let (blobs, _errors): (Vec<_>, Vec<_>) = std::fs::read_dir(&cpath)?
            .map(|e| {
                e.map(|e| Blob {
                    id: e.file_name().into_string().unwrap(),
                    container: container.id.to_string(),
                    byte_size: e.metadata().unwrap().len(),
                })
            })
            .partition(Result::is_ok);
        let blobs = blobs.into_iter().map(Result::unwrap).collect();
        let bloblist = BlobList { blobs };
        let mut buf = Vec::new();
        bloblist.encode(&mut buf)?;
        Ok(buf)
    }

    fn upload_chunk(&self, _actor: &str, chunk: FileChunk) -> Result<Vec<u8>, Box<dyn Error>> {
        let bpath = Path::join(
            &Path::join(&self.rootdir.read().unwrap(), chunk.container.to_string()),
            chunk.id.to_string(),
        );
        let mut file = OpenOptions::new().create(false).append(true).open(bpath)?;
        info!(
            "Receiving file chunk: {} for {}/{}",
            chunk.sequence_no, chunk.container, chunk.id
        );

        file.write(chunk.chunk_bytes.as_ref())?;

        Ok(vec![])
    }

    fn start_download(
        &self,
        actor: &str,
        request: StreamRequest,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        info!("Received request to start download : {:?}", request);
        let actor = actor.to_string();
        let bpath = Path::join(
            &Path::join(&self.rootdir.read().unwrap(), request.container.to_string()),
            request.id.to_string(),
        );
        let byte_size = &bpath.metadata()?.len();
        let bfile = std::fs::File::open(bpath)?;
        let chunk_size = if request.chunk_size == 0 {
            chunks::DEFAULT_CHUNK_SIZE
        } else {
            request.chunk_size as usize
        };
        let xfer = Transfer {
            blob_id: request.id.clone(),
            container: request.container.clone(),
            total_size: *byte_size,
            chunk_size: chunk_size as _,
            total_chunks: *byte_size / chunk_size as u64,
        };
        let iter = Chunks::new(bfile, chunk_size);
        let d = self.dispatcher.clone();
        std::thread::spawn(move || {
            iter.enumerate().for_each(|(i, chunk)| {
                dispatch_chunk(&xfer, &actor, i, d.clone(), chunk);
            });
        });

        Ok(vec![])
    }

    fn blob_to_path(&self, blob: &Blob) -> PathBuf {
        let cdir = Path::join(&self.rootdir.read().unwrap(), blob.container.to_string());
        Path::join(&cdir, blob.id.to_string())
    }

    fn container_to_path(&self, container: &Container) -> PathBuf {
        Path::join(&self.rootdir.read().unwrap(), container.id.to_string())
    }
}

fn dispatch_chunk(
    xfer: &Transfer,
    actor: &str,
    i: usize,
    d: Arc<RwLock<Box<dyn Dispatcher>>>,
    chunk: Result<Vec<u8>, std::io::Error>,
) {
    match chunk {
        Ok(chunk) => {
            let fc = FileChunk {
                sequence_no: i as u64,
                container: xfer.container.to_string(),
                id: xfer.blob_id.to_string(),
                chunk_bytes: chunk,
                chunk_size: xfer.chunk_size,
                total_bytes: xfer.total_size,
            };
            let mut buf = Vec::new();
            fc.encode(&mut buf).unwrap();
            match d
                .read()
                .unwrap()
                .dispatch(&format!("{}!{}", actor, OP_RECEIVE_CHUNK), &buf)
            {
                Ok(_) => {}
                Err(_) => {}
            }
        }
        Err(_) => {}
    }
}

impl CapabilityProvider for FileSystemProvider {
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
        "waSCC Blob Store Provider (File System)" 
    }

    // Invoked by host runtime to allow an actor to make use of the capability
    // All providers MUST handle the "configure" message, even if no work will be done
    fn handle_call(&self, actor: &str, op: &str, msg: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
        trace!("Received host call from {}, operation - {}", actor, op);

        match op {
            OP_CONFIGURE if actor == "system" => self.configure(msg.to_vec().as_ref()),
            OP_CREATE_CONTAINER => self.create_container(actor, msg.into()),
            OP_REMOVE_CONTAINER => self.remove_container(actor, msg.into()),
            OP_REMOVE_OBJECT => self.remove_object(actor, msg.into()),
            OP_LIST_OBJECTS => self.list_objects(actor, msg.into()),
            OP_UPLOAD_CHUNK => self.upload_chunk(actor, msg.into()),
            OP_START_DOWNLOAD => self.start_download(actor, msg.into()),
            OP_START_UPLOAD => self.start_upload(actor, msg.into()),
            OP_GET_OBJECT_INFO => self.get_object_info(actor, msg.into()),
            _ => Err("bad dispatch".into()),
        }
    }
}

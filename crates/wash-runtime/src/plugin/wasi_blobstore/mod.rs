mod filesystem;
mod in_memory;
#[cfg(feature = "wasm_component_model_implements")]
mod multiplexed;
#[cfg(feature = "wasm_component_model_implements")]
mod multiplexed_async;
mod nats;

pub use filesystem::FilesystemBlobstore;
pub use in_memory::InMemoryBlobstore;
#[cfg(feature = "wasm_component_model_implements")]
pub use multiplexed::{
    BlobBackend, BlobBackendError, BlobId, BlobProvider, FilesystemBackend, FilesystemProvider,
    InMemoryBackend, InMemoryProvider, MultiplexedBlobstore, NatsBlobBackend, NatsBlobProvider,
};
#[cfg(feature = "wasm_component_model_implements")]
pub use multiplexed_async::MultiplexedAsyncBlobstore;
pub use nats::NatsBlobstore;

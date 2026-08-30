mod filesystem;
mod fs_store;
pub(crate) mod in_memory;
#[cfg(feature = "wasm_component_model_implements")]
pub(crate) mod multiplexed;
#[cfg(feature = "wasm_component_model_implements")]
pub(crate) mod multiplexed_async;
pub(crate) mod nats;
mod redis;

pub use filesystem::FilesystemKeyValue;
pub use in_memory::InMemoryKeyValue;
#[cfg(feature = "wasm_component_model_implements")]
pub use multiplexed::{
    FilesystemBackend, FilesystemProvider, InMemoryBackend, InMemoryProvider, KeyResponse,
    KvBackend, KvId, KvProvider, MultiplexedKeyValue, NatsBackend, NatsProvider, RedisBackend,
    RedisProvider, StoreError,
};
#[cfg(feature = "wasm_component_model_implements")]
pub use multiplexed_async::MultiplexedAsyncKeyValue;
pub use nats::NatsKeyValue;
pub use redis::RedisKeyValue;

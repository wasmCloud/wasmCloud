use super::logging::logging;
use super::{blobstore, bus, format_opt, messaging};

use core::convert::Infallible;
use core::fmt::Debug;
use core::future::Future;
use core::pin::Pin;
use core::str::FromStr;
use core::time::Duration;

use std::ops::RangeInclusive;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures::{Stream, TryStreamExt};
use nkeys::{KeyPair, KeyPairType};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::{instrument, trace};

#[derive(Clone, Default)]
pub struct Handler {
    blobstore: Option<Arc<dyn Blobstore + Sync + Send>>,
    bus: Option<Arc<dyn Bus + Sync + Send>>,
    incoming_http: Option<Arc<dyn IncomingHttp + Sync + Send>>,
    outgoing_http: Option<Arc<dyn OutgoingHttp + Sync + Send>>,
    keyvalue_atomic: Option<Arc<dyn KeyValueAtomic + Sync + Send>>,
    keyvalue_eventual: Option<Arc<dyn KeyValueEventual + Sync + Send>>,
    logging: Option<Arc<dyn Logging + Sync + Send>>,
    messaging: Option<Arc<dyn Messaging + Sync + Send>>,
}

impl Debug for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handler")
            .field("blobstore", &format_opt(&self.blobstore))
            .field("bus", &format_opt(&self.bus))
            .field("incoming_http", &format_opt(&self.incoming_http))
            .field("keyvalue_atomic", &format_opt(&self.keyvalue_atomic))
            .field("keyvalue_eventual", &format_opt(&self.keyvalue_eventual))
            .field("logging", &format_opt(&self.logging))
            .field("messaging", &format_opt(&self.messaging))
            .field("outgoing_http", &format_opt(&self.outgoing_http))
            .finish()
    }
}

fn proxy<'a, T: ?Sized>(
    field: &'a Option<Arc<T>>,
    interface: &str,
    method: &str,
) -> anyhow::Result<&'a Arc<T>> {
    trace!("call `{interface}` handler");
    field
        .as_ref()
        .with_context(|| format!("cannot handle `{method}`"))
}

impl Handler {
    fn proxy_bus(&self, method: &str) -> anyhow::Result<&Arc<dyn Bus + Sync + Send>> {
        proxy(&self.bus, "Bus", method)
    }

    fn proxy_blobstore(&self, method: &str) -> anyhow::Result<&Arc<dyn Blobstore + Sync + Send>> {
        proxy(&self.blobstore, "Blobstore", method)
    }

    fn proxy_keyvalue_atomic(
        &self,
        method: &str,
    ) -> anyhow::Result<&Arc<dyn KeyValueAtomic + Sync + Send>> {
        proxy(&self.keyvalue_atomic, "KeyvalueAtomic", method)
    }

    fn proxy_keyvalue_eventual(
        &self,
        method: &str,
    ) -> anyhow::Result<&Arc<dyn KeyValueEventual + Sync + Send>> {
        proxy(&self.keyvalue_eventual, "KeyvalueEventual", method)
    }

    fn proxy_messaging(&self, method: &str) -> anyhow::Result<&Arc<dyn Messaging + Sync + Send>> {
        proxy(&self.messaging, "Messaging", method)
    }

    /// Replace [`Blobstore`] handler returning the old one, if such was set
    pub fn replace_blobstore(
        &mut self,
        blobstore: Arc<dyn Blobstore + Send + Sync>,
    ) -> Option<Arc<dyn Blobstore + Send + Sync>> {
        self.blobstore.replace(blobstore)
    }

    /// Replace [`Bus`] handler returning the old one, if such was set
    pub fn replace_bus(
        &mut self,
        bus: Arc<dyn Bus + Send + Sync>,
    ) -> Option<Arc<dyn Bus + Send + Sync>> {
        self.bus.replace(bus)
    }

    /// Replace [`IncomingHttp`] handler returning the old one, if such was set
    pub fn replace_incoming_http(
        &mut self,
        incoming_http: Arc<dyn IncomingHttp + Send + Sync>,
    ) -> Option<Arc<dyn IncomingHttp + Send + Sync>> {
        self.incoming_http.replace(incoming_http)
    }

    /// Replace [`KeyValueAtomic`] handler returning the old one, if such was set
    pub fn replace_keyvalue_atomic(
        &mut self,
        keyvalue_atomic: Arc<dyn KeyValueAtomic + Send + Sync>,
    ) -> Option<Arc<dyn KeyValueAtomic + Send + Sync>> {
        self.keyvalue_atomic.replace(keyvalue_atomic)
    }

    /// Replace [`KeyValueEventual`] handler returning the old one, if such was set
    pub fn replace_keyvalue_eventual(
        &mut self,
        keyvalue_eventual: Arc<dyn KeyValueEventual + Send + Sync>,
    ) -> Option<Arc<dyn KeyValueEventual + Send + Sync>> {
        self.keyvalue_eventual.replace(keyvalue_eventual)
    }

    /// Replace [`Logging`] handler returning the old one, if such was set
    pub fn replace_logging(
        &mut self,
        logging: Arc<dyn Logging + Send + Sync>,
    ) -> Option<Arc<dyn Logging + Send + Sync>> {
        self.logging.replace(logging)
    }

    /// Replace [`Messaging`] handler returning the old one, if such was set
    pub fn replace_messaging(
        &mut self,
        messaging: Arc<dyn Messaging + Send + Sync>,
    ) -> Option<Arc<dyn Messaging + Send + Sync>> {
        self.messaging.replace(messaging)
    }

    /// Replace [`OutgoingHttp`] handler returning the old one, if such was set
    pub fn replace_outgoing_http(
        &mut self,
        outgoing_http: Arc<dyn OutgoingHttp + Send + Sync>,
    ) -> Option<Arc<dyn OutgoingHttp + Send + Sync>> {
        self.outgoing_http.replace(outgoing_http)
    }
}

#[derive(Clone, Debug)]
/// Actor identifier
pub enum ActorIdentifier {
    /// Actor call alias identifier
    Alias(String),
    /// Actor public key identifier
    Key(Arc<KeyPair>),
}

impl From<&str> for ActorIdentifier {
    fn from(s: &str) -> Self {
        if let Ok(key) = KeyPair::from_public_key(s) {
            if key.key_pair_type() == KeyPairType::Module {
                return Self::Key(Arc::new(key));
            }
        }
        Self::Alias(s.into())
    }
}

impl FromStr for ActorIdentifier {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(s.into())
    }
}

impl PartialEq for ActorIdentifier {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Alias(l), Self::Alias(r)) => l == r,
            (Self::Key(l), Self::Key(r)) => l.public_key() == r.public_key(),
            _ => false,
        }
    }
}

impl Eq for ActorIdentifier {}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Target entity
pub enum TargetEntity {
    /// Link target entity
    Link(Option<String>),
    /// Actor target entity
    Actor(ActorIdentifier),
}

impl TryFrom<bus::lattice::ActorIdentifier> for ActorIdentifier {
    type Error = anyhow::Error;

    fn try_from(entity: bus::lattice::ActorIdentifier) -> Result<Self, Self::Error> {
        match entity {
            bus::lattice::ActorIdentifier::PublicKey(key) => {
                let key =
                    KeyPair::from_public_key(&key).context("failed to parse actor public key")?;
                Ok(ActorIdentifier::Key(Arc::new(key)))
            }
            bus::lattice::ActorIdentifier::Alias(alias) => Ok(ActorIdentifier::Alias(alias)),
        }
    }
}

impl TryFrom<bus::lattice::TargetEntity> for TargetEntity {
    type Error = anyhow::Error;

    fn try_from(entity: bus::lattice::TargetEntity) -> Result<Self, Self::Error> {
        match entity {
            bus::lattice::TargetEntity::Link(name) => Ok(Self::Link(name)),
            bus::lattice::TargetEntity::Actor(actor) => actor.try_into().map(TargetEntity::Actor),
        }
    }
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
/// Call target identifier
pub enum TargetInterface {
    /// `wasi:blobstore/blobstore`
    WasiBlobstoreBlobstore,
    /// `wasi:http/outgoing-handler`
    WasiHttpOutgoingHandler,
    /// `wasi:keyvalue/atomic`
    WasiKeyvalueAtomic,
    /// `wasi:keyvalue/eventual`
    WasiKeyvalueEventual,
    /// `wasi:logging/logging`
    WasiLoggingLogging,
    /// `wasmcloud:messaging/consumer`
    WasmcloudMessagingConsumer,
    /// Custom interface
    Custom {
        /// Package namespace
        namespace: String,
        /// Package name
        package: String,
        /// Interface name
        interface: String,
    },
}

/// Outgoing HTTP request
pub struct OutgoingHttpRequest {
    /// Whether to use TLS
    pub use_tls: bool,
    /// TLS authority
    pub authority: String,
    /// HTTP request
    pub request: ::http::Request<Box<dyn AsyncRead + Sync + Send + Unpin>>,
    /// The timeout for the initial connect.
    pub connect_timeout: Duration,
    /// The timeout for receiving the first byte of the response body.
    pub first_byte_timeout: Duration,
    /// The timeout for receiving the next chunk of bytes in the response body
    /// stream.
    pub between_bytes_timeout: Duration,
}

#[async_trait]
/// `wasi:blobstore/blobstore` implementation
pub trait Blobstore {
    /// Handle `wasi:blobstore/blobstore.create-container`
    async fn create_container(&self, name: &str) -> anyhow::Result<()>;

    /// Handle `wasi:blobstore/blobstore.container-exists`
    async fn container_exists(&self, name: &str) -> anyhow::Result<bool>;

    /// Handle `wasi:blobstore/blobstore.delete-container`
    async fn delete_container(&self, name: &str) -> anyhow::Result<()>;

    /// Handle `wasi:blobstore/container.info`
    async fn container_info(
        &self,
        name: &str,
    ) -> anyhow::Result<blobstore::container::ContainerMetadata>;

    /// Handle `wasi:blobstore/container.get-data`
    async fn get_data(
        &self,
        container: &str,
        name: String,
        range: RangeInclusive<u64>,
    ) -> anyhow::Result<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)>;

    /// Handle `wasi:blobstore/container.has-object`
    async fn has_object(&self, container: &str, name: String) -> anyhow::Result<bool>;

    /// Handle `wasi:blobstore/container.write-data`
    async fn write_data(
        &self,
        container: &str,
        name: String,
        value: Box<dyn AsyncRead + Sync + Send + Unpin>,
    ) -> anyhow::Result<()>;

    /// Handle `wasi:blobstore/blobstore.delete-objects`
    async fn delete_objects(&self, container: &str, names: Vec<String>) -> anyhow::Result<()>;

    /// Handle `wasi:blobstore/container.list-objects`
    async fn list_objects(
        &self,
        container: &str,
    ) -> anyhow::Result<Box<dyn Stream<Item = anyhow::Result<String>> + Sync + Send + Unpin>>;

    /// Handle `wasi:blobstore/container.object-info`
    async fn object_info(
        &self,
        container: &str,
        name: String,
    ) -> anyhow::Result<blobstore::container::ObjectMetadata>;

    /// Handle `wasi:blobstore/container.clear`
    async fn clear_container(&self, container: &str) -> anyhow::Result<()> {
        let names = self
            .list_objects(container)
            .await
            .context("failed to list objects")?;
        let names = names
            .try_collect()
            .await
            .context("failed to collect object names")?;
        self.delete_objects(container, names).await
    }
}

#[async_trait]
/// `wasmcloud:bus/host` implementation
pub trait Bus {
    /// Identify the target of wasmbus module invocation
    async fn identify_wasmbus_target(
        &self,
        binding: &str,
        namespace: &str,
    ) -> anyhow::Result<TargetEntity>;

    /// Identify the target of component interface invocation
    async fn identify_interface_target(
        &self,
        interface: &TargetInterface,
    ) -> anyhow::Result<Option<TargetEntity>>;

    /// Set interface call target
    async fn set_target(
        &self,
        target: Option<TargetEntity>,
        interfaces: Vec<TargetInterface>,
    ) -> anyhow::Result<()>;

    /// Handle `wasmcloud:bus/host.call`
    async fn call(
        &self,
        target: Option<TargetEntity>,
        operation: String,
    ) -> anyhow::Result<(
        Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
        Box<dyn AsyncWrite + Sync + Send + Unpin>,
        Box<dyn AsyncRead + Sync + Send + Unpin>,
    )>;

    /// Handle `wasmcloud:bus/config.get`
    async fn get(
        &self,
        key: &str,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, super::guest_config::ConfigError>>;

    /// Handle `wasmcloud:bus/config.get_all`
    async fn get_all(
        &self,
    ) -> anyhow::Result<Result<Vec<(String, Vec<u8>)>, super::guest_config::ConfigError>>;

    /// Handle `wasmcloud:bus/host.call` without streaming
    async fn call_sync(
        &self,
        target: Option<TargetEntity>,
        operation: String,
        mut payload: Vec<u8>,
    ) -> anyhow::Result<Vec<u8>> {
        let (res, mut input, mut output) = self
            .call(target, operation)
            .await
            .context("failed to process call")?;
        input
            .write_all(&payload)
            .await
            .context("failed to write request")?;
        payload.clear();
        output
            .read_to_end(&mut payload)
            .await
            .context("failed to read output")?;
        res.await.map_err(|e| anyhow!(e).context("call failed"))?;
        Ok(payload)
    }
}

#[async_trait]
/// `wasi:http/incoming-handler` implementation
pub trait IncomingHttp {
    /// Handle `wasi:http/incoming-handler`
    async fn handle(
        &self,
        request: ::http::Request<Box<dyn AsyncRead + Sync + Send + Unpin>>,
    ) -> anyhow::Result<::http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>>;
}

#[async_trait]
/// `wasi:keyvalue/atomic` implementation
pub trait KeyValueAtomic {
    /// Handle `wasi:keyvalue/atomic.increment`
    async fn increment(&self, bucket: &str, key: String, delta: u64) -> anyhow::Result<u64>;

    /// Handle `wasi:keyvalue/atomic.compare-and-swap`
    async fn compare_and_swap(
        &self,
        bucket: &str,
        key: String,
        old: u64,
        new: u64,
    ) -> anyhow::Result<bool>;
}

#[async_trait]
/// `wasi:keyvalue/eventual` implementation
pub trait KeyValueEventual {
    /// Handle `wasi:keyvalue/eventual.get`
    async fn get(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<Option<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)>>;

    /// Handle `wasi:keyvalue/eventual.set`
    async fn set(
        &self,
        bucket: &str,
        key: String,
        value: Box<dyn AsyncRead + Sync + Send + Unpin>,
    ) -> anyhow::Result<()>;

    /// Handle `wasi:keyvalue/eventual.delete`
    async fn delete(&self, bucket: &str, key: String) -> anyhow::Result<()>;

    /// Handle `wasi:keyvalue/eventual.exists`
    async fn exists(&self, bucket: &str, key: String) -> anyhow::Result<bool>;
}

#[async_trait]
/// `wasi:logging/logging` implementation
pub trait Logging {
    /// Handle `wasi:logging/logging.log`
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()>;
}

#[async_trait]
/// `wasmcloud:messaging/consumer` implementation
pub trait Messaging {
    /// Handle `wasmcloud:messaging/consumer.request`
    async fn request(
        &self,
        subject: String,
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> anyhow::Result<messaging::types::BrokerMessage>;

    /// Handle `wasmcloud:messaging/consumer.request_multi`
    async fn request_multi(
        &self,
        subject: String,
        body: Option<Vec<u8>>,
        timeout: Duration,
        max_results: u32,
    ) -> anyhow::Result<Vec<messaging::types::BrokerMessage>>;

    /// Handle `wasmcloud:messaging/consumer.publish`
    async fn publish(&self, msg: messaging::types::BrokerMessage) -> anyhow::Result<()>;
}

#[async_trait]
/// `wasi:http/outgoing-handler` implementation
pub trait OutgoingHttp {
    /// Handle `wasi:http/outgoing-handler`
    async fn handle(
        &self,
        request: OutgoingHttpRequest,
    ) -> anyhow::Result<::http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>>;
}

#[async_trait]
impl Blobstore for Handler {
    #[instrument]
    async fn create_container(&self, name: &str) -> anyhow::Result<()> {
        self.proxy_blobstore("wasi:blobstore/blobstore.create-container")?
            .create_container(name)
            .await
    }

    #[instrument]
    async fn container_exists(&self, name: &str) -> anyhow::Result<bool> {
        self.proxy_blobstore("wasi:blobstore/blobstore.container-exists")?
            .container_exists(name)
            .await
    }

    #[instrument]
    async fn delete_container(&self, name: &str) -> anyhow::Result<()> {
        self.proxy_blobstore("wasi:blobstore/blobstore.delete-container")?
            .delete_container(name)
            .await
    }

    #[instrument]
    async fn container_info(
        &self,
        name: &str,
    ) -> anyhow::Result<blobstore::container::ContainerMetadata> {
        self.proxy_blobstore("wasi:blobstore/container.info")?
            .container_info(name)
            .await
    }

    #[instrument]
    async fn get_data(
        &self,
        container: &str,
        name: String,
        range: RangeInclusive<u64>,
    ) -> anyhow::Result<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)> {
        self.proxy_blobstore("wasi:blobstore/container.get-data")?
            .get_data(container, name, range)
            .await
    }

    #[instrument]
    async fn has_object(&self, container: &str, name: String) -> anyhow::Result<bool> {
        self.proxy_blobstore("wasi:blobstore/container.has-object")?
            .has_object(container, name)
            .await
    }

    #[instrument(skip(value))]
    async fn write_data(
        &self,
        container: &str,
        name: String,
        value: Box<dyn AsyncRead + Sync + Send + Unpin>,
    ) -> anyhow::Result<()> {
        self.proxy_blobstore("wasi:blobstore/container.write-data")?
            .write_data(container, name, value)
            .await
    }

    #[instrument]
    async fn delete_objects(&self, container: &str, names: Vec<String>) -> anyhow::Result<()> {
        self.proxy_blobstore("wasi:blobstore/container.delete-objects")?
            .delete_objects(container, names)
            .await
    }

    #[instrument]
    async fn list_objects(
        &self,
        container: &str,
    ) -> anyhow::Result<Box<dyn Stream<Item = anyhow::Result<String>> + Sync + Send + Unpin>> {
        self.proxy_blobstore("wasi:blobstore/container.list-objects")?
            .list_objects(container)
            .await
    }

    #[instrument]
    async fn object_info(
        &self,
        container: &str,
        name: String,
    ) -> anyhow::Result<blobstore::container::ObjectMetadata> {
        self.proxy_blobstore("wasi:blobstore/container.object-info")?
            .object_info(container, name)
            .await
    }

    #[instrument]
    async fn clear_container(&self, container: &str) -> anyhow::Result<()> {
        self.proxy_blobstore("wasi:blobstore/container.clear")?
            .clear_container(container)
            .await
    }
}

#[async_trait]
impl Bus for Handler {
    #[instrument(level = "trace", skip_all)]
    async fn identify_wasmbus_target(
        &self,
        binding: &str,
        namespace: &str,
    ) -> anyhow::Result<TargetEntity> {
        if let Some(ref bus) = self.bus {
            trace!("call `Bus` handler");
            bus.identify_wasmbus_target(binding, namespace).await
        } else {
            bail!("host cannot identify the Wasmbus call target")
        }
    }

    #[instrument(level = "trace", skip_all)]
    async fn identify_interface_target(
        &self,
        interface: &TargetInterface,
    ) -> anyhow::Result<Option<TargetEntity>> {
        if let Some(ref bus) = self.bus {
            trace!("call `Bus` handler");
            bus.identify_interface_target(interface).await
        } else {
            bail!("host cannot identify the interface call target")
        }
    }

    #[instrument(level = "trace", skip_all)]
    async fn set_target(
        &self,
        target: Option<TargetEntity>,
        interfaces: Vec<TargetInterface>,
    ) -> anyhow::Result<()> {
        self.proxy_bus("wasmcloud:bus/lattice.set-target")?
            .set_target(target, interfaces)
            .await
    }

    #[instrument(level = "trace", skip_all)]
    async fn get(
        &self,
        key: &str,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, super::guest_config::ConfigError>> {
        self.proxy_bus("wasmcloud:bus/config.get")?.get(key).await
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_all(
        &self,
    ) -> anyhow::Result<Result<Vec<(String, Vec<u8>)>, super::guest_config::ConfigError>> {
        self.proxy_bus("wasmcloud:bus/config.get_all")?
            .get_all()
            .await
    }

    #[instrument(level = "trace", skip_all)]
    async fn call(
        &self,
        target: Option<TargetEntity>,
        operation: String,
    ) -> anyhow::Result<(
        Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
        Box<dyn AsyncWrite + Sync + Send + Unpin>,
        Box<dyn AsyncRead + Sync + Send + Unpin>,
    )> {
        self.proxy_bus("wasmcloud:bus/host.call")?
            .call(target, operation)
            .await
    }

    #[instrument(level = "trace", skip_all)]
    async fn call_sync(
        &self,
        target: Option<TargetEntity>,
        operation: String,
        payload: Vec<u8>,
    ) -> anyhow::Result<Vec<u8>> {
        self.proxy_bus("wasmcloud:bus/host.call-sync")?
            .call_sync(target, operation, payload)
            .await
    }
}

#[async_trait]
impl Logging for Handler {
    #[instrument(level = "trace", skip_all)]
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        if let Some(ref logging) = self.logging {
            trace!("call `Logging` handler");
            logging.log(level, context, message).await
        } else {
            // discard all log invocations by default
            Ok(())
        }
    }
}

#[async_trait]
impl KeyValueAtomic for Handler {
    async fn increment(&self, bucket: &str, key: String, delta: u64) -> anyhow::Result<u64> {
        self.proxy_keyvalue_atomic("wasi:keyvalue/atomic.increment")?
            .increment(bucket, key, delta)
            .await
    }

    async fn compare_and_swap(
        &self,
        bucket: &str,
        key: String,
        old: u64,
        new: u64,
    ) -> anyhow::Result<bool> {
        self.proxy_keyvalue_atomic("wasi:keyvalue/atomic.compare-and-swap")?
            .compare_and_swap(bucket, key, old, new)
            .await
    }
}

#[async_trait]
impl KeyValueEventual for Handler {
    #[instrument]
    async fn get(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<Option<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)>> {
        self.proxy_keyvalue_eventual("wasi:keyvalue/eventual.get")?
            .get(bucket, key)
            .await
    }

    #[instrument(skip(value))]
    async fn set(
        &self,
        bucket: &str,
        key: String,
        value: Box<dyn AsyncRead + Sync + Send + Unpin>,
    ) -> anyhow::Result<()> {
        self.proxy_keyvalue_eventual("wasi:keyvalue/eventual.set")?
            .set(bucket, key, value)
            .await
    }

    #[instrument]
    async fn delete(&self, bucket: &str, key: String) -> anyhow::Result<()> {
        self.proxy_keyvalue_eventual("wasi:keyvalue/eventual.delete")?
            .delete(bucket, key)
            .await
    }

    #[instrument]
    async fn exists(&self, bucket: &str, key: String) -> anyhow::Result<bool> {
        self.proxy_keyvalue_eventual("wasi:keyvalue/eventual.exists")?
            .exists(bucket, key)
            .await
    }
}

#[async_trait]
impl IncomingHttp for Handler {
    #[instrument(skip(request))]
    async fn handle(
        &self,
        request: ::http::Request<Box<dyn AsyncRead + Sync + Send + Unpin>>,
    ) -> anyhow::Result<::http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>> {
        proxy(
            &self.incoming_http,
            "IncomingHttp",
            "wasi:http/incoming-handler.handle",
        )?
        .handle(request)
        .await
    }
}

#[async_trait]
impl Messaging for Handler {
    #[instrument(skip(body))]
    async fn request(
        &self,
        subject: String,
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> anyhow::Result<messaging::types::BrokerMessage> {
        self.proxy_messaging("wasmcloud:messaging/consumer.request")?
            .request(subject, body, timeout)
            .await
    }

    #[instrument(skip(body))]
    async fn request_multi(
        &self,
        subject: String,
        body: Option<Vec<u8>>,
        timeout: Duration,
        max_results: u32,
    ) -> anyhow::Result<Vec<messaging::types::BrokerMessage>> {
        self.proxy_messaging("wasmcloud:messaging/consumer.request-multi")?
            .request_multi(subject, body, timeout, max_results)
            .await
    }

    #[instrument(skip(msg))]
    async fn publish(&self, msg: messaging::types::BrokerMessage) -> anyhow::Result<()> {
        self.proxy_messaging("wasmcloud:messaging/consumer.publish")?
            .publish(msg)
            .await
    }
}

#[async_trait]
impl OutgoingHttp for Handler {
    #[instrument(skip(request))]
    async fn handle(
        &self,
        request: OutgoingHttpRequest,
    ) -> anyhow::Result<::http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>> {
        proxy(
            &self.outgoing_http,
            "OutgoingHttp",
            "wasi:http/outgoing-handler.handle",
        )?
        .handle(request)
        .await
    }
}

/// A [Handler] builder used to configure it
#[derive(Clone, Default)]
pub(crate) struct HandlerBuilder {
    /// [`Blobstore`] handler
    pub blobstore: Option<Arc<dyn Blobstore + Sync + Send>>,
    /// [`Bus`] handler
    pub bus: Option<Arc<dyn Bus + Sync + Send>>,
    /// [`IncomingHttp`] handler
    pub incoming_http: Option<Arc<dyn IncomingHttp + Sync + Send>>,
    /// [`KeyValueAtomic`] handler
    pub keyvalue_atomic: Option<Arc<dyn KeyValueAtomic + Sync + Send>>,
    /// [`KeyValueEventual`] handler
    pub keyvalue_eventual: Option<Arc<dyn KeyValueEventual + Sync + Send>>,
    /// [`Logging`] handler
    pub logging: Option<Arc<dyn Logging + Sync + Send>>,
    /// [`Messaging`] handler
    pub messaging: Option<Arc<dyn Messaging + Sync + Send>>,
    /// [`OutgoingHttp`] handler
    pub outgoing_http: Option<Arc<dyn OutgoingHttp + Sync + Send>>,
}

impl HandlerBuilder {
    /// Set [`Blobstore`] handler
    pub fn blobstore(self, blobstore: Arc<impl Blobstore + Sync + Send + 'static>) -> Self {
        Self {
            blobstore: Some(blobstore),
            ..self
        }
    }

    /// Set [`Bus`] handler
    pub fn bus(self, bus: Arc<impl Bus + Sync + Send + 'static>) -> Self {
        Self {
            bus: Some(bus),
            ..self
        }
    }

    /// Set [`IncomingHttp`] handler
    pub fn incoming_http(
        self,
        incoming_http: Arc<impl IncomingHttp + Sync + Send + 'static>,
    ) -> Self {
        Self {
            incoming_http: Some(incoming_http),
            ..self
        }
    }

    /// Set [`KeyValueAtomic`] handler
    pub fn keyvalue_atomic(
        self,
        keyvalue_atomic: Arc<impl KeyValueAtomic + Sync + Send + 'static>,
    ) -> Self {
        Self {
            keyvalue_atomic: Some(keyvalue_atomic),
            ..self
        }
    }

    /// Set [`KeyValueEventual`] handler
    pub fn keyvalue_eventual(
        self,
        keyvalue_eventual: Arc<impl KeyValueEventual + Sync + Send + 'static>,
    ) -> Self {
        Self {
            keyvalue_eventual: Some(keyvalue_eventual),
            ..self
        }
    }

    /// Set [`Logging`] handler
    pub fn logging(self, logging: Arc<impl Logging + Sync + Send + 'static>) -> Self {
        Self {
            logging: Some(logging),
            ..self
        }
    }

    /// Set [`Messaging`] handler
    pub fn messaging(self, messaging: Arc<impl Messaging + Sync + Send + 'static>) -> Self {
        Self {
            messaging: Some(messaging),
            ..self
        }
    }

    /// Set [`OutgoingHttp`] handler
    pub fn outgoing_http(
        self,
        outgoing_http: Arc<impl OutgoingHttp + Sync + Send + 'static>,
    ) -> Self {
        Self {
            outgoing_http: Some(outgoing_http),
            ..self
        }
    }
}

impl Debug for HandlerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerBuilder")
            .field("blobstore", &format_opt(&self.blobstore))
            .field("bus", &format_opt(&self.bus))
            .field("incoming_http", &format_opt(&self.incoming_http))
            .field("keyvalue_atomic", &format_opt(&self.keyvalue_atomic))
            .field("keyvalue_eventual", &format_opt(&self.keyvalue_eventual))
            .field("logging", &format_opt(&self.logging))
            .field("messaging", &format_opt(&self.messaging))
            .field("outgoing_http", &format_opt(&self.outgoing_http))
            .finish()
    }
}

impl From<Handler> for HandlerBuilder {
    fn from(
        Handler {
            blobstore,
            bus,
            incoming_http,
            keyvalue_atomic,
            keyvalue_eventual,
            logging,
            messaging,
            outgoing_http,
        }: Handler,
    ) -> Self {
        Self {
            blobstore,
            bus,
            incoming_http,
            keyvalue_atomic,
            keyvalue_eventual,
            logging,
            messaging,
            outgoing_http,
        }
    }
}

impl From<HandlerBuilder> for Handler {
    fn from(
        HandlerBuilder {
            blobstore,
            bus,
            incoming_http,
            keyvalue_atomic,
            keyvalue_eventual,
            logging,
            messaging,
            outgoing_http,
        }: HandlerBuilder,
    ) -> Self {
        Self {
            blobstore,
            bus,
            incoming_http,
            outgoing_http,
            keyvalue_atomic,
            keyvalue_eventual,
            logging,
            messaging,
        }
    }
}

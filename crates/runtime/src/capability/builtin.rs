use super::bus;
use super::logging::logging;
use super::{format_opt, messaging};

use core::convert::Infallible;
use core::fmt::Debug;
use core::future::Future;
use core::pin::Pin;
use core::str::FromStr;
use core::time::Duration;

use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use nkeys::{KeyPair, KeyPairType};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::{instrument, trace};

#[derive(Clone, Default)]
pub struct Handler {
    blobstore: Option<Arc<dyn Blobstore + Sync + Send>>,
    bus: Option<Arc<dyn Bus + Sync + Send>>,
    incoming_http: Option<Arc<dyn IncomingHttp + Sync + Send>>,
    keyvalue_atomic: Option<Arc<dyn KeyValueAtomic + Sync + Send>>,
    keyvalue_readwrite: Option<Arc<dyn KeyValueReadWrite + Sync + Send>>,
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
            .field("keyvalue_readwrite", &format_opt(&self.keyvalue_readwrite))
            .field("logging", &format_opt(&self.logging))
            .field("messaging", &format_opt(&self.messaging))
            .finish()
    }
}

impl Handler {
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

    /// Replace [`KeyValueReadWrite`] handler returning the old one, if such was set
    pub fn replace_keyvalue_readwrite(
        &mut self,
        keyvalue_readwrite: Arc<dyn KeyValueReadWrite + Send + Sync>,
    ) -> Option<Arc<dyn KeyValueReadWrite + Send + Sync>> {
        self.keyvalue_readwrite.replace(keyvalue_readwrite)
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

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
/// Call target identifier
pub enum TargetInterface {
    /// `wasi:keyvalue/atomic`
    WasiKeyvalueAtomic,
    /// `wasi:keyvalue/readwrite`
    WasiKeyvalueReadwrite,
    /// `wasi:logging/logging`
    WasiLoggingLogging,
    /// `wasmcloud:blobstore/consumer`
    WasmcloudBlobstoreConsumer,
    /// `wasmcloud:messaging/consumer`
    WasmcloudMessagingConsumer,
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
        request: http::Request<Box<dyn AsyncRead + Sync + Send + Unpin>>,
    ) -> anyhow::Result<http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>>;
}

#[async_trait]
/// `wasi:blobstore/consumer` implementation
pub trait Blobstore {
    // TODO: Implement
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
/// `wasi:keyvalue/readwrite` implementation
pub trait KeyValueReadWrite {
    /// Handle `wasi:keyvalue/readwrite.get`
    async fn get(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)>;

    /// Handle `wasi:keyvalue/readwrite.set`
    async fn set(
        &self,
        bucket: &str,
        key: String,
        value: Box<dyn AsyncRead + Sync + Send + Unpin>,
    ) -> anyhow::Result<()>;

    /// Handle `wasi:keyvalue/readwrite.delete`
    async fn delete(&self, bucket: &str, key: String) -> anyhow::Result<()>;

    /// Handle `wasi:keyvalue/readwrite.exists`
    async fn exists(&self, bucket: &str, key: String) -> anyhow::Result<bool>;
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
impl Bus for Handler {
    #[instrument]
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

    #[instrument]
    async fn set_target(
        &self,
        target: Option<TargetEntity>,
        interfaces: Vec<TargetInterface>,
    ) -> anyhow::Result<()> {
        if let Some(ref bus) = self.bus {
            trace!("call `Bus` handler");
            bus.set_target(target, interfaces).await
        } else {
            bail!("host cannot set call target")
        }
    }

    #[instrument]
    async fn call(
        &self,
        target: Option<TargetEntity>,
        operation: String,
    ) -> anyhow::Result<(
        Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
        Box<dyn AsyncWrite + Sync + Send + Unpin>,
        Box<dyn AsyncRead + Sync + Send + Unpin>,
    )> {
        if let Some(ref bus) = self.bus {
            trace!("call `Bus` handler");
            bus.call(target, operation).await
        } else {
            bail!("host cannot handle `{operation}`")
        }
    }

    async fn call_sync(
        &self,
        target: Option<TargetEntity>,
        operation: String,
        payload: Vec<u8>,
    ) -> anyhow::Result<Vec<u8>> {
        if let Some(ref bus) = self.bus {
            trace!("call `Bus` handler");
            bus.call_sync(target, operation, payload).await
        } else {
            bail!("host cannot handle `{operation}`")
        }
    }
}

#[async_trait]
impl Logging for Handler {
    #[instrument]
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
        trace!("call `KeyValueAtomic` handler");
        self.keyvalue_atomic
            .as_ref()
            .context("cannot handle `wasi:keyvalue/atomic.increment`")?
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
        trace!("call `KeyValueAtomic` handler");
        self.keyvalue_atomic
            .as_ref()
            .context("cannot handle `wasi:keyvalue/atomic.compare_and_swap`")?
            .compare_and_swap(bucket, key, old, new)
            .await
    }
}

#[async_trait]
impl KeyValueReadWrite for Handler {
    #[instrument]
    async fn get(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<(Box<dyn AsyncRead + Sync + Send + Unpin>, u64)> {
        trace!("call `KeyValueReadWrite` handler");
        self.keyvalue_readwrite
            .as_ref()
            .context("cannot handle `wasi:keyvalue/readwrite.get`")?
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
        trace!("call `KeyValueReadWrite` handler");
        self.keyvalue_readwrite
            .as_ref()
            .context("cannot handle `wasi:keyvalue/readwrite.set`")?
            .set(bucket, key, value)
            .await
    }

    #[instrument]
    async fn delete(&self, bucket: &str, key: String) -> anyhow::Result<()> {
        trace!("call `KeyValueReadWrite` handler");
        self.keyvalue_readwrite
            .as_ref()
            .context("cannot handle `wasi:keyvalue/readwrite.delete`")?
            .delete(bucket, key)
            .await
    }

    #[instrument]
    async fn exists(&self, bucket: &str, key: String) -> anyhow::Result<bool> {
        trace!("call `KeyValueReadWrite` handler");
        self.keyvalue_readwrite
            .as_ref()
            .context("cannot handle `wasi:keyvalue/readwrite.exists`")?
            .exists(bucket, key)
            .await
    }
}

#[async_trait]
impl IncomingHttp for Handler {
    #[instrument(skip(request))]
    async fn handle(
        &self,
        request: http::Request<Box<dyn AsyncRead + Sync + Send + Unpin>>,
    ) -> anyhow::Result<http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>> {
        trace!("call `IncomingHttp` handler");
        self.incoming_http
            .as_ref()
            .context("cannot handle `wasi:http/incoming-handler.handle`")?
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
        trace!("call `Messaging` handler");
        self.messaging
            .as_ref()
            .context("cannot handle `wasmcloud:messaging/consumer.request`")?
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
        trace!("call `Messaging` handler");
        self.messaging
            .as_ref()
            .context("cannot handle `wasmcloud:messaging/consumer.request_multi`")?
            .request_multi(subject, body, timeout, max_results)
            .await
    }

    #[instrument(skip(msg))]
    async fn publish(&self, msg: messaging::types::BrokerMessage) -> anyhow::Result<()> {
        trace!("call `Messaging` handler");
        self.messaging
            .as_ref()
            .context("cannot handle `wasmcloud:messaging/consumer.publish`")?
            .publish(msg)
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
    /// [`KeyValueReadWrite`] handler
    pub keyvalue_readwrite: Option<Arc<dyn KeyValueReadWrite + Sync + Send>>,
    /// [`Logging`] handler
    pub logging: Option<Arc<dyn Logging + Sync + Send>>,
    /// [`Messaging`] handler
    pub messaging: Option<Arc<dyn Messaging + Sync + Send>>,
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

    /// Set [`KeyValueReadWrite`] handler
    pub fn keyvalue_readwrite(
        self,
        keyvalue_readwrite: Arc<impl KeyValueReadWrite + Sync + Send + 'static>,
    ) -> Self {
        Self {
            keyvalue_readwrite: Some(keyvalue_readwrite),
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
}

impl Debug for HandlerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerBuilder")
            .field("blobstore", &format_opt(&self.blobstore))
            .field("bus", &format_opt(&self.bus))
            .field("incoming_http", &format_opt(&self.incoming_http))
            .field("keyvalue_atomic", &format_opt(&self.keyvalue_atomic))
            .field("keyvalue_readwrite", &format_opt(&self.keyvalue_readwrite))
            .field("logging", &format_opt(&self.logging))
            .field("messaging", &format_opt(&self.messaging))
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
            keyvalue_readwrite,
            logging,
            messaging,
        }: Handler,
    ) -> Self {
        Self {
            blobstore,
            bus,
            incoming_http,
            keyvalue_atomic,
            keyvalue_readwrite,
            logging,
            messaging,
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
            keyvalue_readwrite,
            logging,
            messaging,
        }: HandlerBuilder,
    ) -> Self {
        Self {
            blobstore,
            bus,
            incoming_http,
            keyvalue_atomic,
            keyvalue_readwrite,
            logging,
            messaging,
        }
    }
}

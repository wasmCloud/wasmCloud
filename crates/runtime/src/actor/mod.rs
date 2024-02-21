mod component;

pub use component::{
    Component, Instance as ComponentInstance, InterfaceInstance as ComponentInterfaceInstance,
};

use crate::capability::logging::logging;
use crate::capability::{
    Blobstore, Bus, IncomingHttp, KeyValueAtomic, KeyValueEventual, Logging, Messaging,
    OutgoingHttp,
};
use crate::Runtime;

use component::{incoming_http_bindings, logging_bindings, InterfaceInstance};

use core::fmt::Debug;

use std::sync::Arc;

use anyhow::{bail, ensure, Context, Result};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tracing::instrument;
use wascap::jwt;
use wascap::wasm::extract_claims;

/// Actor instance configuration
#[derive(Clone, Debug, Default)]
pub struct Config {
    /// Whether actors are required to be signed to be executed
    pub require_signature: bool,
}

/// Extracts and validates claims contained within `WebAssembly` binary, if such are found
fn claims(wasm: impl AsRef<[u8]>) -> Result<Option<jwt::Claims<jwt::Actor>>> {
    let Some(claims) = extract_claims(wasm).context("failed to extract module claims")? else {
        return Ok(None);
    };
    let v = jwt::validate_token::<jwt::Actor>(&claims.jwt)
        .context("failed to validate module token")?;
    ensure!(!v.expired, "token expired at `{}`", v.expires_human);
    ensure!(
        !v.cannot_use_yet,
        "token cannot be used before `{}`",
        v.not_before_human
    );
    ensure!(v.signature_valid, "signature is not valid");
    Ok(Some(claims.claims))
}

/// A pre-loaded wasmCloud actor, which is either a module or a component
#[derive(Clone, Debug)]
pub struct Actor(Component);

impl Actor {
    /// Compiles WebAssembly binary using [Runtime].
    ///
    /// # Errors
    ///
    /// Fails if [Component::new] or [Module::new] fails
    #[instrument(level = "trace", skip_all)]
    pub fn new(rt: &Runtime, wasm: impl AsRef<[u8]>) -> Result<Self> {
        let wasm = wasm.as_ref();
        // TODO: Optimize parsing, add functionality to `wascap` to parse from a custom section
        // directly
        match wasmparser::Parser::new(0).parse_all(wasm).next() {
            Some(Ok(wasmparser::Payload::Version {
                encoding: wasmparser::Encoding::Component,
                ..
            })) => Component::new(rt, wasm).map(Actor),
            // fallback to module type
            _ => bail!("TODO: convert"),
        }
    }

    /// Reads the WebAssembly binary asynchronously and calls [Actor::new].
    ///
    /// # Errors
    ///
    /// Fails if either reading `wasm` fails or [Self::new] fails
    #[instrument(skip(wasm))]
    pub async fn read(rt: &Runtime, mut wasm: impl AsyncRead + Unpin) -> Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf)
            .await
            .context("failed to read Wasm")?;
        Self::new(rt, buf)
    }

    /// Reads the WebAssembly binary synchronously and calls [Actor::new].
    ///
    /// # Errors
    ///
    /// Fails if either reading `wasm` fails or [Self::new] fails
    #[instrument(skip(wasm))]
    pub fn read_sync(rt: &Runtime, mut wasm: impl std::io::Read) -> Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf).context("failed to read Wasm")?;
        Self::new(rt, buf)
    }

    /// [Claims](jwt::Claims) associated with this [Actor].
    #[instrument(level = "trace")]
    pub fn claims(&self) -> Option<&jwt::Claims<jwt::Actor>> {
        self.0.claims()
    }

    /// Like [Self::instantiate], but moves the [Actor].
    #[instrument]
    pub fn into_instance(self) -> anyhow::Result<Instance> {
        self.0.into_instance().map(Instance)
    }

    /// Like [Self::instantiate], but moves the [Actor] and returns associated [jwt::Claims].
    #[instrument]
    pub async fn into_instance_claims(
        self,
    ) -> anyhow::Result<(Instance, Option<jwt::Claims<jwt::Actor>>)> {
        let (instance, claims) = self.0.into_instance_claims()?;
        Ok((Instance(instance), claims))
    }

    /// Instantiate the actor.
    ///
    /// # Errors
    ///
    /// Fails if instantiation of the underlying module or component fails
    #[instrument(level = "trace", skip_all)]
    pub async fn instantiate(&self) -> anyhow::Result<Instance> {
        self.0.instantiate().map(Instance)
    }

    /// Instantiate the actor and invoke an operation on it.
    ///
    /// # Errors
    ///
    /// Fails if [`Instance::call`] fails
    #[instrument(level = "trace", skip_all)]
    pub async fn call(
        &self,
        instance: &str,
        name: &str,
        params: Vec<wrpc_transport::Value>,
    ) -> anyhow::Result<Vec<wrpc_transport::Value>> {
        self.instantiate()
            .await
            .context("failed to instantiate actor")?
            .call(instance, name, params)
            .await
    }

    /// Instantiates and returns a [`IncomingHttpInstance`] if exported by the [`Instance`].
    ///
    /// # Errors
    ///
    /// Fails if either instantiation fails or no incoming HTTP bindings are exported by the [`Instance`]
    pub async fn as_incoming_http(
        &self,
    ) -> anyhow::Result<InterfaceInstance<incoming_http_bindings::IncomingHttp>> {
        self.instantiate()
            .await
            .context("failed to instantiate actor")?
            .into_incoming_http()
            .await
    }

    /// Instantiates and returns a [`LoggingInstance`] if exported by the [`Instance`].
    ///
    /// # Errors
    ///
    /// Fails if either instantiation fails or no logging bindings are exported by the [`Instance`]
    pub async fn as_logging(&self) -> anyhow::Result<InterfaceInstance<logging_bindings::Logging>> {
        self.instantiate()
            .await
            .context("failed to instantiate actor")?
            .into_logging()
            .await
    }
}

/// A pre-loaded, configured wasmCloud actor component instance
#[derive(Debug)]
pub struct Instance(ComponentInstance);

/// A pre-loaded, configured [Logging] actor component instance
pub struct LoggingInstance(ComponentInterfaceInstance<component::logging_bindings::Logging>);

/// A pre-loaded, configured [`IncomingHttp`] actor component instance
pub struct IncomingHttpInstance(
    ComponentInterfaceInstance<component::incoming_http_bindings::IncomingHttp>,
);

#[async_trait]
impl Logging for LoggingInstance {
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        self.0.log(level, context, message).await
    }
}

#[async_trait]
impl IncomingHttp for IncomingHttpInstance {
    async fn handle(
        &self,
        request: http::Request<Box<dyn AsyncRead + Sync + Send + Unpin>>,
    ) -> anyhow::Result<http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>> {
        self.0.handle(request).await
    }
}

impl Instance {
    /// Reset [`Instance`] state to defaults
    pub async fn reset(&mut self, rt: &Runtime) {
        self.0.reset(rt).await
    }

    /// Set [`Blobstore`] handler for this [Instance].
    pub fn blobstore(&mut self, blobstore: Arc<dyn Blobstore + Send + Sync>) -> &mut Self {
        self.0.blobstore(blobstore);
        self
    }

    /// Set [`Bus`] handler for this [Instance].
    pub fn bus(&mut self, bus: Arc<dyn Bus + Send + Sync>) -> &mut Self {
        self.0.bus(bus);
        self
    }

    /// Set [`IncomingHttp`] handler for this [Instance].
    pub fn incoming_http(
        &mut self,
        incoming_http: Arc<dyn IncomingHttp + Send + Sync>,
    ) -> &mut Self {
        self.0.incoming_http(incoming_http);
        self
    }

    /// Set [`KeyValueAtomic`] handler for this [Instance].
    pub fn keyvalue_atomic(
        &mut self,
        keyvalue_atomic: Arc<dyn KeyValueAtomic + Send + Sync>,
    ) -> &mut Self {
        self.0.keyvalue_atomic(keyvalue_atomic);
        self
    }

    /// Set [`KeyValueEventual`] handler for this [Instance].
    pub fn keyvalue_eventual(
        &mut self,
        keyvalue_eventual: Arc<dyn KeyValueEventual + Send + Sync>,
    ) -> &mut Self {
        self.0.keyvalue_eventual(keyvalue_eventual);
        self
    }

    /// Set [`Logging`] handler for this [Instance].
    pub fn logging(&mut self, logging: Arc<dyn Logging + Send + Sync>) -> &mut Self {
        self.0.logging(logging);
        self
    }

    /// Set [`Messaging`] handler for this [Instance].
    pub fn messaging(&mut self, messaging: Arc<dyn Messaging + Send + Sync>) -> &mut Self {
        self.0.messaging(messaging);
        self
    }

    /// Set [`OutgoingHttp`] handler for this [Instance].
    pub fn outgoing_http(
        &mut self,
        outgoing_http: Arc<dyn OutgoingHttp + Send + Sync>,
    ) -> &mut Self {
        self.0.outgoing_http(outgoing_http);
        self
    }

    /// Set actor stderr stream. If another stderr was set, it is replaced and the old one is flushed and shut down if supported by underlying actor implementation.
    ///
    /// # Errors
    ///
    /// Fails if flushing and shutting down old stream fails
    pub async fn stderr(
        &mut self,
        stderr: impl AsyncWrite + Send + Sync + Unpin + 'static,
    ) -> anyhow::Result<&mut Self> {
        self.0.stderr(stderr).await?;
        Ok(self)
    }

    /// Invoke an operation on an [Instance] producing a response
    ///
    /// # Errors
    ///
    /// Outermost error represents a failure in calling the actor, innermost - the
    /// application-layer error originating from within the actor itself
    #[instrument(level = "debug", skip_all)]
    pub async fn call(
        &mut self,
        instance: &str,
        name: &str,
        params: Vec<wrpc_transport::Value>,
    ) -> anyhow::Result<Vec<wrpc_transport::Value>> {
        self.0.call(instance, name, params).await
    }

    /// Instantiates and returns a [`IncomingHttpInstance`] if exported by the [`Instance`].
    ///
    /// # Errors
    ///
    /// Fails if no incoming HTTP bindings are exported by the [`Instance`]
    pub async fn into_incoming_http(
        self,
    ) -> anyhow::Result<InterfaceInstance<incoming_http_bindings::IncomingHttp>> {
        self.0.into_incoming_http().await
    }

    /// Instantiates and returns a [`LoggingInstance`] if exported by the [`Instance`].
    ///
    /// # Errors
    ///
    /// Fails if no logging bindings are exported by the [`Instance`]
    pub async fn into_logging(
        self,
    ) -> anyhow::Result<InterfaceInstance<logging_bindings::Logging>> {
        self.0.into_logging().await
    }
}

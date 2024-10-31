use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context as _};
use async_trait::async_trait;
use bytes::Bytes;
use secrecy::Secret;
use tokio::sync::RwLock;
use tracing::{error, instrument, warn};
use wasmcloud_runtime::capability;
use wasmcloud_runtime::capability::logging::logging;
use wasmcloud_runtime::capability::secrets::store::SecretValue;
use wasmcloud_runtime::capability::{secrets, CallTargetInterface};
use wasmcloud_runtime::component::{
    Bus, Bus1_0_0, Config, InvocationErrorIntrospect, InvocationErrorKind, Logging,
    ReplacedInstanceTarget, Secrets,
};
use wasmcloud_tracing::context::TraceContextInjector;
use wrpc_transport::InvokeExt as _;

use super::config::ConfigBundle;
use super::injector_to_headers;

#[derive(Clone, Debug)]
pub struct Handler {
    pub nats: Arc<async_nats::Client>,
    // ConfigBundle is perfectly safe to pass around, but in order to update it on the fly, we need
    // to have it behind a lock since it can be cloned and because the `Actor` struct this gets
    // placed into is also inside of an Arc
    pub config_data: Arc<RwLock<ConfigBundle>>,
    /// Secrets are cached per-[`Handler`] so they can be used at runtime without consulting the secrets
    /// backend for each request. The [`SecretValue`] is wrapped in the [`Secret`] type from the `secrecy`
    /// crate to ensure that it is not accidentally logged or exposed in error messages.
    pub secrets: Arc<RwLock<HashMap<String, Secret<SecretValue>>>>,
    /// The lattice this handler will use for RPC
    pub lattice: Arc<str>,
    /// The identifier of the component that this handler is associated with
    pub component_id: Arc<str>,
    /// The current link targets. `instance` -> `link-name`
    /// Instance specification does not include a version
    pub targets: Arc<RwLock<HashMap<Box<str>, Arc<str>>>>,
    /// The current trace context of the handler, required to propagate trace context
    /// when crossing the Wasm guest/host boundary
    pub trace_ctx: Arc<RwLock<Vec<(String, String)>>>,

    /// Map of link names -> instance -> Target
    ///
    /// While a target may often be a component ID, it is not guaranteed to be one, and could be
    /// some other identifier of where to send invocations, representing one or more lattice entities.
    ///
    /// Lattice entities could be:
    /// - A (single) Component ID
    /// - A routing group
    /// - Some other opaque string
    #[allow(clippy::type_complexity)]
    pub instance_links: Arc<RwLock<HashMap<Box<str>, HashMap<Box<str>, Box<str>>>>>,

    pub invocation_timeout: Duration,
}

impl Handler {
    /// Used for creating a new handler from an existing one. This is different than clone because
    /// some fields shouldn't be copied between component instances such as link targets.
    pub fn copy_for_new(&self) -> Self {
        Handler {
            nats: self.nats.clone(),
            config_data: self.config_data.clone(),
            secrets: self.secrets.clone(),
            lattice: self.lattice.clone(),
            component_id: self.component_id.clone(),
            targets: Arc::default(),
            trace_ctx: Arc::default(),
            instance_links: self.instance_links.clone(),
            invocation_timeout: self.invocation_timeout,
        }
    }
}

#[async_trait]
impl Bus1_0_0 for Handler {
    /// Set the current link name in use by the handler, which is otherwise "default".
    ///
    /// Link names are important to set to differentiate similar operations (ex. `wasi:keyvalue/store.get`)
    /// that should go to different targets (ex. a capability provider like `kv-redis` vs `kv-vault`)
    #[instrument(level = "debug", skip(self))]
    async fn set_link_name(&self, link_name: String, interfaces: Vec<Arc<CallTargetInterface>>) {
        let interfaces = interfaces.iter().map(Deref::deref);
        let mut targets = self.targets.write().await;
        if link_name == "default" {
            for CallTargetInterface {
                namespace,
                package,
                interface,
            } in interfaces
            {
                targets.remove(&format!("{namespace}:{package}/{interface}").into_boxed_str());
            }
        } else {
            let link_name = Arc::from(link_name);
            for CallTargetInterface {
                namespace,
                package,
                interface,
            } in interfaces
            {
                targets.insert(
                    format!("{namespace}:{package}/{interface}").into_boxed_str(),
                    Arc::clone(&link_name),
                );
            }
        }
    }
}

#[async_trait]
impl Bus for Handler {
    /// Set the current link name in use by the handler, which is otherwise "default".
    ///
    /// Link names are important to set to differentiate similar operations (ex. `wasi:keyvalue/store.get`)
    /// that should go to different targets (ex. a capability provider like `kv-redis` vs `kv-vault`)
    #[instrument(level = "debug", skip(self))]
    async fn set_link_name(
        &self,
        link_name: String,
        interfaces: Vec<Arc<CallTargetInterface>>,
    ) -> anyhow::Result<Result<(), String>> {
        let links = self.instance_links.read().await;
        // Ensure that all interfaces have an established link with the given name.
        if let Some(interface_missing_link) = interfaces.iter().find_map(|i| {
            let instance = i.as_instance();
            // This could be expressed in one line as a `!(bool).then_some`, but the negation makes it confusing
            if links
                .get(link_name.as_str())
                .and_then(|l| l.get(instance.as_str()))
                .is_none()
            {
                Some(instance)
            } else {
                None
            }
        }) {
            return Ok(Err(format!(
                "interface `{interface_missing_link}` does not have an existing link with name `{link_name}`"
            )));
        }
        // Explicitly drop the lock before calling `set_link_name` just to avoid holding the lock for longer than needed
        drop(links);

        Bus1_0_0::set_link_name(self, link_name, interfaces).await;
        Ok(Ok(()))
    }
}

impl wrpc_transport::Invoke for Handler {
    type Context = Option<ReplacedInstanceTarget>;
    type Outgoing = <wrpc_transport_nats::Client as wrpc_transport::Invoke>::Outgoing;
    type Incoming = <wrpc_transport_nats::Client as wrpc_transport::Invoke>::Incoming;

    async fn invoke<P>(
        &self,
        target_instance: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        // Reading a trace context should _never_ block because writing happens once at the beginning of a component
        // invocation. If it does block here, it's a bug in the runtime, and it's better to deal with a
        // disconnected trace than to block on the invocation for an extended period of time.
        if let Ok(trace_context) = self.trace_ctx.try_read() {
            wasmcloud_tracing::context::attach_span_context(&trace_context);
        }

        let links = self.instance_links.read().await;
        let targets = self.targets.read().await;

        let target_instance = match target_instance {
            Some(
                ReplacedInstanceTarget::BlobstoreBlobstore
                | ReplacedInstanceTarget::BlobstoreContainer,
            ) => "wasi:blobstore/blobstore",
            Some(ReplacedInstanceTarget::KeyvalueAtomics) => "wasi:keyvalue/atomics",
            Some(ReplacedInstanceTarget::KeyvalueStore) => "wasi:keyvalue/store",
            Some(ReplacedInstanceTarget::KeyvalueBatch) => "wasi:keyvalue/batch",
            Some(ReplacedInstanceTarget::HttpIncomingHandler) => "wasi:http/incoming-handler",
            Some(ReplacedInstanceTarget::HttpOutgoingHandler) => "wasi:http/outgoing-handler",
            None => instance.split_once('@').map_or(instance, |(l, _)| l),
        };

        let link_name = targets
            .get(target_instance)
            .map_or("default", AsRef::as_ref);

        let instances = links.get(link_name).with_context(|| {
            warn!(
                instance,
                link_name,
                ?target_instance,
                ?self.component_id,
                "no links with link name found for instance"
            );
            format!("link `{link_name}` not found for instance `{target_instance}`")
        })?;

        // Determine the lattice target ID we should be sending to
        let id = instances.get(target_instance).with_context(||{
            warn!(
                instance,
                ?target_instance,
                ?self.component_id,
                "component is not linked to a lattice target for the given instance"
            );
            format!("failed to call `{func}` in instance `{instance}` (failed to find a configured link with name `{link_name}` from component `{id}`, please check your configuration)", id = self.component_id)
        })?;

        let mut headers = injector_to_headers(&TraceContextInjector::default_with_span());
        headers.insert("source-id", &*self.component_id);
        headers.insert("link-name", link_name);
        let nats = wrpc_transport_nats::Client::new(
            Arc::clone(&self.nats),
            format!("{}.{id}", &self.lattice),
            None,
        )
        .await?;
        nats.timeout(self.invocation_timeout)
            .invoke(Some(headers), instance, func, params, paths)
            .await
    }
}

#[async_trait]
impl Config for Handler {
    #[instrument(level = "debug", skip_all)]
    async fn get(
        &self,
        key: &str,
    ) -> anyhow::Result<Result<Option<String>, capability::config::store::Error>> {
        let lock = self.config_data.read().await;
        let conf = lock.get_config().await;
        let data = conf.get(key).cloned();
        Ok(Ok(data))
    }

    #[instrument(level = "debug", skip_all)]
    async fn get_all(
        &self,
    ) -> anyhow::Result<Result<Vec<(String, String)>, capability::config::store::Error>> {
        Ok(Ok(self
            .config_data
            .read()
            .await
            .get_config()
            .await
            .clone()
            .into_iter()
            .collect()))
    }
}

#[async_trait]
impl Logging for Handler {
    #[instrument(level = "trace", skip(self))]
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        match level {
            logging::Level::Trace => {
                tracing::event!(
                    tracing::Level::TRACE,
                    component_id = ?self.component_id,
                    level = level.to_string(),
                    context,
                    "{message}"
                );
            }
            logging::Level::Debug => {
                tracing::event!(
                    tracing::Level::DEBUG,
                    component_id = ?self.component_id,
                    level = level.to_string(),
                    context,
                    "{message}"
                );
            }
            logging::Level::Info => {
                tracing::event!(
                    tracing::Level::INFO,
                    component_id = ?self.component_id,
                    level = level.to_string(),
                    context,
                    "{message}"
                );
            }
            logging::Level::Warn => {
                tracing::event!(
                    tracing::Level::WARN,
                    component_id = ?self.component_id,
                    level = level.to_string(),
                    context,
                    "{message}"
                );
            }
            logging::Level::Error => {
                tracing::event!(
                    tracing::Level::ERROR,
                    component_id = ?self.component_id,
                    level = level.to_string(),
                    context,
                    "{message}"
                );
            }
            logging::Level::Critical => {
                tracing::event!(
                    tracing::Level::ERROR,
                    component_id = ?self.component_id,
                    level = level.to_string(),
                    context,
                    "{message}"
                );
            }
        };
        Ok(())
    }
}

#[async_trait]
impl Secrets for Handler {
    #[instrument(level = "debug", skip_all)]
    async fn get(
        &self,
        key: &str,
    ) -> anyhow::Result<Result<secrets::store::Secret, secrets::store::SecretsError>> {
        if self.secrets.read().await.get(key).is_some() {
            Ok(Ok(Arc::new(key.to_string())))
        } else {
            Ok(Err(secrets::store::SecretsError::NotFound))
        }
    }

    async fn reveal(
        &self,
        secret: secrets::store::Secret,
    ) -> anyhow::Result<secrets::store::SecretValue> {
        let read_lock = self.secrets.read().await;
        let Some(secret_val) = read_lock.get(secret.as_str()) else {
            // NOTE(brooksmtownsend): This error case should never happen, since we check for existence during `get` and
            // fail to start the component if the secret is missing. We might hit this during wRPC testing with resources.
            const ERROR_MSG: &str = "secret not found to reveal, ensure the secret is declared and associated with this component at startup";
            // NOTE: This "secret" is just the name of the key, not the actual secret value. Regardless the secret itself
            // both wasn't found and is wrapped by `secrecy` so it won't be logged.
            error!(?secret, ERROR_MSG);
            bail!(ERROR_MSG)
        };
        use secrecy::ExposeSecret;
        Ok(secret_val.expose_secret().clone())
    }
}

impl InvocationErrorIntrospect for Handler {
    fn invocation_error_kind(&self, err: &anyhow::Error) -> InvocationErrorKind {
        if let Some(err) = err.root_cause().downcast_ref::<std::io::Error>() {
            if err.kind() == std::io::ErrorKind::NotConnected {
                return InvocationErrorKind::NotFound;
            }
        }
        InvocationErrorKind::Trap
    }
}

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _, Ok};
use async_nats::jetstream;
use futures::{stream, Stream, StreamExt};
use nkeys::KeyPair;
use tokio::fs;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tracing::{debug, error, info, instrument, trace, trace_span, warn, Instrument as _, Span};
use wasmcloud_core::messaging::{add_tls_ca, ConnectionConfig, ConsumerConfig};
use wasmcloud_provider_sdk::provider::InvocationStreams;
use wasmcloud_provider_sdk::ProviderConnection;
use wasmcloud_runtime::capability::wrpc;
use wasmcloud_tracing::KeyValue;

use crate::bindings;
use crate::bindings::exports::wrpc::extension;
use crate::bindings::wrpc::extension::configurable::BaseConfig;
use crate::bindings::wrpc::extension::types::InterfaceConfig;
use crate::bindings::wrpc::extension::types::{BindRequest, BindResponse, HealthCheckResponse};
use crate::wasmbus::providers::{check_health, watch_config};
use crate::wasmbus::{Component, InvocationContext};

#[derive(Clone)]
struct Provider {
    config: ConnectionConfig,
    components: Arc<RwLock<HashMap<String, Arc<Component>>>>,
    messaging_links:
        Arc<RwLock<HashMap<Arc<str>, Arc<RwLock<HashMap<Box<str>, async_nats::Client>>>>>>,
    subscriptions: Arc<Mutex<HashMap<Arc<str>, HashMap<Box<str>, JoinSet<()>>>>>,
    lattice_id: Arc<str>,
    host_id: Arc<str>,
    /// Channel to signal provider shutdown
    quit_tx: broadcast::Sender<()>,
}

impl Provider {
    async fn connect(
        &self,
        config: &HashMap<String, String>,
    ) -> anyhow::Result<(async_nats::Client, ConnectionConfig)> {
        // NOTE: Big part of this is copy-pasted from `provider-messaging-nats`
        let config = if config.is_empty() {
            self.config.clone()
        } else {
            match ConnectionConfig::from_map(config) {
                Result::Ok(cc) => self.config.merge(&cc),
                Result::Err(err) => {
                    error!(?err, "failed to build connection configuration");
                    return Result::Err(anyhow!(err).context("failed to build connection config"));
                }
            }
        };
        let mut opts = match (&config.auth_jwt, &config.auth_seed) {
            (Some(jwt), Some(seed)) => {
                let seed = KeyPair::from_seed(seed).context("failed to parse seed key pair")?;
                let seed = Arc::new(seed);
                async_nats::ConnectOptions::with_jwt(jwt.to_string(), move |nonce| {
                    let seed = seed.clone();
                    async move { seed.sign(&nonce).map_err(async_nats::AuthError::new) }
                })
            }
            (None, None) => async_nats::ConnectOptions::default(),
            _ => bail!("must provide both jwt and seed for jwt authentication"),
        };
        if let Some(tls_ca) = config.tls_ca.as_deref() {
            opts = add_tls_ca(tls_ca, opts)?;
        } else if let Some(tls_ca_file) = config.tls_ca_file.as_deref() {
            let ca = fs::read_to_string(tls_ca_file)
                .await
                .context("failed to read TLS CA file")?;
            opts = add_tls_ca(&ca, opts)?;
        }

        // Use the first visible cluster_uri
        let url = config.cluster_uris.first().context("invalid address")?;

        // Override inbox prefix if specified
        if let Some(ref prefix) = config.custom_inbox_prefix {
            opts = opts.custom_inbox_prefix(prefix);
        }
        let nats = opts
            .name("builtin NATS Messaging Provider")
            .connect(url.as_ref())
            .await
            .context("failed to connect to NATS")?;
        Ok((nats, config))
    }
}

#[instrument(skip_all)]
async fn handle_message(
    components: Arc<RwLock<HashMap<String, Arc<Component>>>>,
    lattice_id: Arc<str>,
    host_id: Arc<str>,
    target_id: Arc<str>,
    link_name: Box<str>,
    msg: async_nats::Message,
) {
    use wrpc::exports::wasmcloud::messaging0_2_0::handler::Handler as _;

    opentelemetry_nats::attach_span_context(&msg);
    let component = {
        let components = components.read().await;
        let Some(component) = components.get(target_id.as_ref()) else {
            warn!(?target_id, "linked component not found");
            return;
        };
        Arc::clone(component)
    };
    let _permit = match component
        .permits
        .acquire()
        .instrument(trace_span!("acquire_message_permit"))
        .await
    {
        Result::Ok(permit) => permit,
        Result::Err(err) => {
            error!(?err, "failed to acquire execution permit");
            return;
        }
    };
    match component
        .instantiate(component.handler.copy_for_new(), component.events.clone())
        .handle_message(
            InvocationContext {
                span: Span::current(),
                start_at: Instant::now(),
                attributes: vec![
                    KeyValue::new("component.ref", Arc::clone(&component.image_reference)),
                    KeyValue::new("lattice", Arc::clone(&lattice_id)),
                    KeyValue::new("host", Arc::clone(&host_id)),
                    KeyValue::new("component.id", Arc::clone(&target_id)),
                    KeyValue::new("link.name", link_name.as_ref().to_string()),
                ],
            },
            wrpc::wasmcloud::messaging0_2_0::types::BrokerMessage {
                subject: msg.subject.into_string(),
                body: msg.payload,
                reply_to: msg.reply.map(async_nats::Subject::into_string),
            },
        )
        .await
    {
        Result::Ok(Result::Ok(())) => {}
        Result::Ok(Result::Err(err)) => {
            warn!(?err, "component failed to handle message")
        }
        Result::Err(err) => {
            warn!(?err, "failed to call component")
        }
    }
}

impl extension::manageable::Handler<Option<wasmcloud_provider_sdk::Context>> for Provider {
    async fn bind(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        _req: BindRequest,
    ) -> anyhow::Result<Result<BindResponse, String>> {
        anyhow::Ok(Result::<BindResponse, String>::Ok(BindResponse {
            identity_token: None,
            provider_pubkey: None,
        }))
    }

    async fn health_request(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
    ) -> anyhow::Result<Result<HealthCheckResponse, String>> {
        anyhow::Ok(Result::<HealthCheckResponse, String>::Ok(
            HealthCheckResponse {
                healthy: true,
                message: Some("OK".to_string()),
            },
        ))
    }

    async fn shutdown(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
    ) -> anyhow::Result<Result<(), String>> {
        // Clean up subscriptions
        let mut subscriptions = self.subscriptions.lock().await;
        for (_, mut component_subs) in subscriptions.drain() {
            for (_, mut tasks) in component_subs.drain() {
                tasks.abort_all();
            }
        }
        // Signal shutdown
        let _ = self.quit_tx.send(());
        anyhow::Ok(Result::<(), String>::Ok(()))
    }
}

impl extension::configurable::Handler<Option<wasmcloud_provider_sdk::Context>> for Provider {
    #[instrument(level = "debug", skip_all)]
    async fn update_base_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        _config: BaseConfig,
    ) -> anyhow::Result<Result<(), String>> {
        anyhow::Ok(Result::<(), String>::Ok(()))
    }

    #[instrument(level = "debug", skip_all)]
    async fn update_interface_export_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        source_id: String,
        link_name: String,
        config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config_vals: HashMap<String, String> = config.config.into_iter().collect();

        // Establish NATS connection
        let nats = match self.connect(&config_vals).await {
            Result::Ok((client, _)) => client,
            Result::Err(e) => {
                let error_msg = format!("Failed to connect to NATS: {}", e);
                error!(%error_msg, %source_id, %link_name, "NATS connection failed");
                return anyhow::Ok(Result::<(), String>::Err(error_msg));
            }
        };

        // Store the client component_id -> link_name -> client
        {
            let mut links = self.messaging_links.write().await;
            let component_links = links
                .entry(source_id.clone().into())
                .or_insert_with(|| Arc::new(RwLock::new(HashMap::new())));

            let mut component_links = component_links.write().await;
            component_links.insert(link_name.clone().into(), nats);
        }

        debug!(%source_id, %link_name, "Successfully updated interface export config");
        anyhow::Ok(Result::<(), String>::Ok(()))
    }

    #[instrument(level = "debug", skip_all)]
    async fn update_interface_import_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        target_id: String,
        link_name: String,
        config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config_vals: HashMap<String, String> = config.config.into_iter().collect();

        // Establish NATS connection
        let (nats, connection_config) = match self.connect(&config_vals).await {
            Result::Ok((client, connection_config)) => (client, connection_config),
            Result::Err(e) => {
                let error_msg = format!("Failed to connect to NATS: {}", e);
                error!(%error_msg, %target_id, %link_name, "NATS connection failed");
                return anyhow::Ok(Result::<(), String>::Err(error_msg));
            }
        };

        let mut tasks = JoinSet::new();

        for ConsumerConfig {
            stream,
            consumer,
            max_messages,
            max_bytes,
        } in connection_config.consumers
        {
            let js = jetstream::new(nats.clone());
            let stream = match js.get_stream(stream).await {
                Result::Ok(s) => s,
                Result::Err(e) => {
                    let error_msg = format!("Failed to get stream: {}", e);
                    error!(%error_msg, %target_id, %link_name);
                    return anyhow::Ok(Result::<(), String>::Err(error_msg));
                }
            };
            let consumer = match stream.get_consumer(&consumer).await {
                Result::Ok(c) => c,
                Result::Err(e) => {
                    let error_msg = format!("Failed to get consumer: {}", e);
                    error!(%error_msg, %target_id, %link_name);
                    return anyhow::Ok(Result::<(), String>::Err(error_msg));
                }
            };
            let sub = consumer.batch();
            let sub = if let Some(max_messages) = max_messages {
                sub.max_messages(max_messages)
            } else {
                sub
            };
            let sub = if let Some(max_bytes) = max_bytes {
                sub.max_bytes(max_bytes)
            } else {
                sub
            };
            let mut sub = sub.messages().await.context("failed to subscribe")?;

            let components = Arc::clone(&self.components);
            let lattice_id = Arc::clone(&self.lattice_id);
            let host_id = Arc::clone(&self.host_id);
            let target_id = target_id.clone();
            let link_name = link_name.clone();
            tasks.spawn(async move {
                while let Some(msg) = sub.next().await {
                    let msg = match msg {
                        Result::Ok(msg) => msg,
                        Result::Err(err) => {
                            error!(?err, "failed to receive message");
                            continue;
                        }
                    };
                    let (msg, ack) = msg.split();
                    tokio::spawn(async move {
                        if let Result::Err(err) = ack.ack().await {
                            error!(?err, "failed to ACK message");
                        } else {
                            debug!("successfully ACK'ed message")
                        }
                    });
                    tokio::spawn(handle_message(
                        Arc::clone(&components),
                        Arc::clone(&lattice_id),
                        Arc::clone(&host_id),
                        target_id.clone().into(),
                        link_name.clone().into(),
                        msg,
                    ));
                }
            });
        }
        for sub in connection_config.subscriptions {
            if sub.is_empty() {
                continue;
            }
            let mut sub = match if let Some((subject, queue)) = sub.split_once('|') {
                nats.queue_subscribe(async_nats::Subject::from(subject), queue.into())
                    .await
            } else {
                nats.subscribe(sub).await
            } {
                Result::Ok(s) => s,
                Result::Err(e) => {
                    let error_msg = format!("Failed to subscribe: {}", e);
                    error!(%error_msg, %target_id, %link_name);
                    return anyhow::Ok(Result::<(), String>::Err(error_msg));
                }
            };
            let components = Arc::clone(&self.components);
            let lattice_id = Arc::clone(&self.lattice_id);
            let host_id = Arc::clone(&self.host_id);
            let target_id = target_id.clone();
            let link_name = link_name.clone();
            tasks.spawn(async move {
                while let Some(msg) = sub.next().await {
                    tokio::spawn(handle_message(
                        Arc::clone(&components),
                        Arc::clone(&lattice_id),
                        Arc::clone(&host_id),
                        target_id.clone().into(),
                        link_name.clone().into(),
                        msg,
                    ));
                }
            });
        }

        // Store subscriptions in the nested structure: component_id -> link_name -> tasks
        let mut subscriptions = self.subscriptions.lock().await;
        subscriptions
            .entry(target_id.into())
            .or_insert_with(HashMap::new)
            .insert(link_name.into(), tasks);

        anyhow::Ok(Result::<(), String>::Ok(()))
    }

    #[instrument(level = "debug", skip_all)]
    async fn delete_interface_import_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        target_id: String,
        link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        let mut subscriptions = self.subscriptions.lock().await;
        if let Some(component_subs) = subscriptions.get_mut(target_id.as_str()) {
            if let Some(mut tasks) = component_subs.remove(&Box::<str>::from(link_name.as_str())) {
                tasks.shutdown().await;
            }
            // Clean up empty component entry
            if component_subs.is_empty() {
                subscriptions.remove(target_id.as_str());
            }
        }
        anyhow::Ok(Result::<(), String>::Ok(()))
    }

    #[instrument(level = "debug", skip_all)]
    async fn delete_interface_export_config(
        &self,
        _cx: Option<wasmcloud_provider_sdk::Context>,
        source_id: String,
        link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        // Remove the NATS client from the nested structure
        let source_id_arc = Arc::<str>::from(source_id.as_str());
        let link_name_box = Box::<str>::from(link_name.as_str());

        let mut links = self.messaging_links.write().await;
        if let Some(component_links) = links.get(&source_id_arc) {
            let mut component_links = component_links.write().await;
            component_links.remove(&link_name_box);

            // Clean up empty component entry if needed
            let is_empty = component_links.is_empty();
            drop(component_links);

            if is_empty {
                links.remove(&source_id_arc);
            }
        }
        anyhow::Ok(Result::<(), String>::Ok(()))
    }
}

/// Run the extension interface serve loop for builtin providers.
async fn run_extension_serve_loop(
    extension_invocations: InvocationStreams,
    mut quit_rx: broadcast::Receiver<()>,
    provider_id: String,
) {
    use std::future::Future;
    use tokio::select;

    // same as in ['serve_provider_extension']
    fn map_invocation_stream(
        (instance, name, invocations): (
            &'static str,
            &'static str,
            Pin<
                Box<
                    dyn Stream<
                            Item = anyhow::Result<
                                Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>,
                            >,
                        > + Send
                        + 'static,
                >,
            >,
        ),
    ) -> impl Stream<
        Item = (
            &'static str,
            &'static str,
            anyhow::Result<Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>>,
        ),
    > {
        invocations.map(move |res| (instance, name, res))
    }

    let mut invocations =
        stream::select_all(extension_invocations.into_iter().map(map_invocation_stream));
    let mut tasks = JoinSet::new();

    info!(
        provider_id,
        "Starting builtin provider extension serve loop"
    );

    loop {
        select! {
            Some((instance, name, res)) = invocations.next() => {
                match res {
                    std::result::Result::Ok(fut) => {
                        tasks.spawn(async move {
                            if let Err(err) = fut.await {
                                warn!(?err, instance, name, "failed to serve invocation");
                            }
                            trace!(instance, name, "successfully served invocation");
                        });
                    },
                    Err(err) => {
                        warn!(?err, instance, name, "failed to accept invocation");
                    }
                }
            },
            _ = quit_rx.recv() => {
                info!(provider_id, "Builtin provider received shutdown signal");
                // Graceful shutdown: wait for in-flight tasks
                let task_count = tasks.len();
                if task_count > 0 {
                    info!(provider_id, task_count, "Waiting for in-flight tasks");
                    while tasks.join_next().await.is_some() {}
                }
                info!(provider_id, "Builtin provider shutdown complete");
                return;
            }
        }
    }
}

impl crate::wasmbus::Host {
    /// Initializes and starts the internal NATS messaging provider.
    ///
    /// The provider starts with default configuration and will receive
    /// actual configuration via the update_base_config wRPC call after binding.
    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn start_messaging_nats_provider(
        self: Arc<Self>,
        provider_id: &str,
        config_names: Vec<String>,
        annotations: BTreeMap<String, String>,
    ) -> anyhow::Result<JoinSet<()>> {
        info!(
            "Starting internal NATS messaging provider with ID: {}",
            provider_id
        );

        let host_id = self.host_key.public_key();

        // Fetch initial config to get connection settings
        let (config_bundle, _secrets) = self
            .fetch_config_and_secrets(
                &config_names,
                None, // No claims for builtin providers
                annotations.get("wasmcloud.dev/appspec"),
            )
            .await?;
        let host_config = config_bundle.get_config().await;
        let config = ConnectionConfig::from_map(&host_config).context("failed to parse config")?;

        // Create quit channel for shutdown coordination
        // The provider's shutdown() method will signal quit_tx when called via wRPC
        let (quit_tx, quit_rx) = broadcast::channel(1);
        let config_shutdown_rx = quit_tx.subscribe();

        let conn = ProviderConnection::new(
            Arc::clone(&self.rpc_nats),
            Arc::from(provider_id),
            Arc::clone(&self.host_config.lattice),
            host_id.clone(),
        )
        .context("failed to establish provider connection")?;

        let provider = Provider {
            config,
            components: Arc::clone(&self.components),
            messaging_links: Arc::clone(&self.messaging_links),
            subscriptions: Arc::default(),
            host_id: Arc::from(host_id.as_str()),
            lattice_id: Arc::clone(&self.host_config.lattice),
            quit_tx,
        };

        // Get extension wRPC client and call serve
        let extension_wrpc_client = conn
            .get_wrpc_extension_serve_client_custom(None)
            .await
            .context("failed to create extension wRPC client")?;

        // Call bindings::serve before spawning - the returned InvocationStreams is 'static
        let extension_invocations = bindings::serve(&extension_wrpc_client, provider)
            .await
            .context("failed to serve extension capability interface")?;

        let mut tasks = JoinSet::new();

        // Spawn the serve loop
        let provider_id_for_task = provider_id.to_string();
        tasks.spawn(run_extension_serve_loop(
            extension_invocations,
            quit_rx,
            provider_id_for_task,
        ));

        // Create wRPC client to communicate with the provider we just started
        let wrpc_client = Arc::new(
            self.provider_manager
                .produce_extension_wrpc_client(provider_id)
                .await?,
        );

        // Perform the full health-check, bind, and configuration flow.
        let config_bundle = self
            .complete_provider_configuration(
                provider_id,
                &config_names,
                None, // No claims for builtin providers
                &annotations,
                &wrpc_client,
            )
            .await?;

        // Spawn the periodic health checker
        tasks.spawn(check_health(
            Arc::clone(&wrpc_client),
            self.event_publisher.clone(),
            self.host_key.public_key(),
            provider_id.to_string(),
        ));

        // Spawn the config watcher task with proper async shutdown signaling
        if let Some(bundle) = config_bundle {
            let provider_id_owned = provider_id.to_string();
            let rpc_nats = self.rpc_nats.clone();
            let lattice = self.host_config.lattice.clone();
            let host_id = Arc::from(self.host_key.public_key());
            tasks.spawn(async move {
                let config_bundle_arc = Arc::new(RwLock::new(bundle));
                let mut shutdown_rx = config_shutdown_rx;

                tokio::select! {
                    _ = watch_config(
                        rpc_nats,
                        config_bundle_arc,
                        lattice,
                        host_id,
                        provider_id_owned.clone(),
                    ) => {
                        trace!(provider_id = %provider_id_owned, "config watcher finished");
                    }
                    _ = shutdown_rx.recv() => {
                        trace!(provider_id = %provider_id_owned, "NATS messaging provider received shutdown signal, config watcher stopping");
                    }
                }
            });
        }

        info!("Internal NATS messaging provider started successfully");
        Ok(tasks)
    }
}

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use futures::{stream, Stream};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::spawn;
use tokio::sync::{RwLock, TryLockError};
use tracing::{debug, error, instrument};
use wasmcloud_core::LatticeTarget;
use wasmcloud_runtime::capability::config::runtime::ConfigError;
use wasmcloud_runtime::capability::logging::logging;
use wasmcloud_runtime::capability::{
    blobstore, keyvalue, messaging, Blobstore, Bus, CallTargetInterface, Config, KeyValueAtomics,
    KeyValueStore, LatticeInterfaceTarget, Logging, Messaging, OutgoingHttp, TargetEntity,
};
use wasmcloud_tracing::context::TraceContextInjector;
use wasmtime_wasi_http::body::HyperIncomingBody;
use wrpc_transport_legacy::{Client, DynamicTuple, IncomingInputStream};

use crate::bindings::{wasmcloud, wrpc};

use super::config::ConfigBundle;
use super::injector_to_headers;

#[derive(Clone, Debug)]
pub struct Handler {
    pub nats: Arc<async_nats::Client>,
    // ConfigBundle is perfectly safe to pass around, but in order to update it on the fly, we need
    // to have it behind a lock since it can be cloned and because the `Actor` struct this gets
    // placed into is also inside of an Arc
    pub config_data: Arc<RwLock<ConfigBundle>>,
    /// The lattice this handler will use for RPC
    pub lattice: String,
    /// The identifier of the component that this handler is associated with
    pub component_id: String,
    /// The current link targets. `instance:interface` -> `link-name`
    pub targets: Arc<RwLock<HashMap<CallTargetInterface, String>>>,
    /// The current trace context of the handler, required to propagate trace context
    /// when crossing the Wasm guest/host boundary
    pub trace_ctx: Arc<RwLock<Vec<(String, String)>>>,

    /// Map of link names -> WIT ns & package -> WIT interface -> Target
    ///
    /// While a target may often be a component ID, it is not guaranteed to be one, and could be
    /// some other identifier of where to send invocations, representing one or more lattice entities.
    ///
    /// Lattice entities could be:
    /// - A (single) Component ID
    /// - A routing group
    /// - Some other opaque string
    #[allow(clippy::type_complexity)]
    pub interface_links:
        Arc<RwLock<HashMap<String, HashMap<String, HashMap<String, LatticeTarget>>>>>,
    /// Map of interface -> function name -> function type
    ///
    /// When invoking a function that the component imports, this map is consulted to determine the
    /// result types of the function, which is required for the wRPC protocol to set up proper
    /// subscriptions for the return types.
    pub polyfills: Arc<HashMap<String, HashMap<String, wrpc_types::DynamicFunction>>>,

    pub invocation_timeout: Duration,
}

impl Handler {
    /// Used for creating a new handler from an existing one. This is different than clone because
    /// some fields shouldn't be copied between component instances such as link targets.
    pub fn copy_for_new(&self) -> Self {
        Handler {
            nats: self.nats.clone(),
            config_data: self.config_data.clone(),
            lattice: self.lattice.clone(),
            component_id: self.component_id.clone(),
            targets: Arc::default(),
            trace_ctx: Arc::default(),
            interface_links: self.interface_links.clone(),
            polyfills: self.polyfills.clone(),
            invocation_timeout: self.invocation_timeout,
        }
    }

    #[instrument(level = "trace", skip(self))]
    fn wrpc_client(
        &self,
        LatticeInterfaceTarget { id, link_name, .. }: &LatticeInterfaceTarget,
    ) -> wasmcloud_core::wrpc::Client {
        let injector = TraceContextInjector::default_with_span();
        let mut headers = injector_to_headers(&injector);
        headers.insert("source-id", self.component_id.as_str());
        headers.insert("link-name", link_name.as_str());
        wasmcloud_core::wrpc::Client::new(
            Arc::clone(&self.nats),
            &self.lattice,
            id,
            headers,
            self.invocation_timeout,
        )
    }

    /// Set the current trace context in use by the handler
    pub async fn set_trace_context(&self, trace_ctx: Vec<(String, String)>) {
        *self.trace_ctx.write().await = trace_ctx;
    }

    #[instrument(level = "trace", skip(self))]
    async fn wrpc_blobstore_blobstore(&self) -> anyhow::Result<wasmcloud_core::wrpc::Client> {
        let (ns, pkg, iface) = ("wasi", "blobstore", "blobstore");
        let lit = self
            .identify_wrpc_target(&CallTargetInterface::from_parts((
                ns, pkg, iface,
            )))
            .await
            .with_context( || {
                let mut msg = format!("failed to call interface `{ns}:{pkg}/{iface}`");
                if let Ok(false) = self.try_link_exists_for_interface(ns, pkg, iface) {
                    msg.push_str(&format!(" (failed to find a configured link from component [{}], please check your configuration)", self.component_id));
                }
                msg
            })?;
        Ok(self.wrpc_client(&lit))
    }

    #[instrument(level = "trace", skip(self))]
    async fn wrpc_http_outgoing_handler(&self) -> anyhow::Result<wasmcloud_core::wrpc::Client> {
        let (ns, pkg, iface) = ("wasi", "http", "outgoing-handler");
        let lit = self
            .identify_wrpc_target(&CallTargetInterface::from_parts((
                ns, pkg, iface
            )))
            .await
            .with_context(|| {
                let mut msg = format!("failed to call interface `{ns}:{pkg}/{iface}`");
                if let Ok(false) = self.try_link_exists_for_interface(ns, pkg, iface) {
                    msg.push_str(&format!(" (failed to find a configured link from component [{}], please check your configuration)", self.component_id));
                }
                msg
            })?;
        Ok(self.wrpc_client(&lit))
    }

    #[instrument(level = "trace", skip(self))]
    async fn wrpc_keyvalue_atomics(&self) -> anyhow::Result<wasmcloud_core::wrpc::Client> {
        let (ns, pkg, iface) = ("wasi", "keyvalue", "atomics");
        let lit = self
            .identify_wrpc_target(&CallTargetInterface::from_parts((
                ns, pkg, iface,
            )))
            .await
            .with_context(|| {
                let mut msg = format!("failed to call interface `{ns}:{pkg}/{iface}`");
                if let Ok(false) = self.try_link_exists_for_interface(ns, pkg, iface) {
                    msg.push_str(&format!(" (failed to find a configured link from component [{}], please check your configuration)", self.component_id));
                }
                msg
            })?;
        Ok(self.wrpc_client(&lit))
    }

    #[instrument(level = "trace", skip(self))]
    async fn wrpc_keyvalue_store(&self) -> anyhow::Result<wasmcloud_core::wrpc::Client> {
        let (ns, pkg, iface) = ("wasi", "keyvalue", "store");
        let lit = self
            .identify_wrpc_target(&CallTargetInterface::from_parts((
                "wasi", "keyvalue", "store",
            )))
            .await
            .with_context(|| {
                let mut msg = format!("failed to call interface `{ns}:{pkg}/{iface}`");
                if let Ok(false) = self.try_link_exists_for_interface(ns, pkg, iface) {
                    msg.push_str(&format!(" (failed to find a configured link from component [{}], please check your configuration)", self.component_id));
                }
                msg
            })?;
        Ok(self.wrpc_client(&lit))
    }

    #[instrument(level = "trace", skip(self))]
    async fn wrpc_messaging_consumer(&self) -> anyhow::Result<wasmcloud_core::wrpc::Client> {
        let (ns, pkg, iface) = ("wasmcloud", "messaging", "consumer");
        let lit = self
            .identify_wrpc_target(&CallTargetInterface::from_parts((
                ns, pkg, iface,
            )))
            .await
            .with_context(|| {
                let mut msg = format!("failed to call interface `{ns}:{pkg}/{iface}`");
                if let Ok(false) = self.try_link_exists_for_interface(ns, pkg, iface) {
                    msg.push_str(&format!(" (failed to find a configured link from component [{}], please check your configuration)", self.component_id));
                }
                msg
            })?;
        Ok(self.wrpc_client(&lit))
    }

    /// Try to find a link for a given interface on the current component
    ///
    /// While normally `interface_links` must be awaited, we use `try_read()` here and pass
    /// along errors on any inability to read the actual value.
    fn try_link_exists_for_interface(
        &self,
        ns: &str,
        pkg: &str,
        iface: &str,
    ) -> Result<bool, TryLockError> {
        let links = self.interface_links.try_read()?;
        Ok(links
            .iter()
            .find_map(|(_k, v)| {
                v.get(&format!("{ns}:{pkg}"))
                    .map(|map| map.contains_key(iface))
            })
            .is_some())
    }
}

#[async_trait]
impl Blobstore for Handler {
    #[instrument(level = "trace", skip(self))]
    async fn create_container(&self, name: &str) -> anyhow::Result<()> {
        use wrpc_interface_blobstore::Blobstore;

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        let (res, tx) = wrpc
            .invoke_create_container(name)
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.create-container`")?;
        // TODO: return a result directly
        res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    async fn container_exists(&self, name: &str) -> anyhow::Result<bool> {
        use wrpc_interface_blobstore::Blobstore;

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        let (res, tx) = wrpc
            .invoke_container_exists(name)
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.container-exists`")?;
        // TODO: return a result directly
        let exists = res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(exists)
    }

    #[instrument(level = "trace", skip(self))]
    async fn delete_container(&self, name: &str) -> anyhow::Result<()> {
        use wrpc_interface_blobstore::Blobstore;

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        let (res, tx) = wrpc
            .invoke_delete_container(name)
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.delete-container`")?;
        // TODO: return a result directly
        res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    async fn container_info(
        &self,
        name: &str,
    ) -> anyhow::Result<blobstore::container::ContainerMetadata> {
        use wrpc_interface_blobstore::{Blobstore, ContainerMetadata};

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        let (res, tx) = wrpc
            .invoke_get_container_info(name)
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.get-container-info`")?;
        // TODO: return a result directly
        let ContainerMetadata { created_at } =
            res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(blobstore::container::ContainerMetadata {
            name: name.to_string(),
            created_at,
        })
    }

    #[instrument(level = "trace", skip(self))]
    async fn get_data(
        &self,
        container: &str,
        name: String,
        range: RangeInclusive<u64>,
    ) -> anyhow::Result<IncomingInputStream> {
        use wrpc_interface_blobstore::{Blobstore, ObjectId};

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        let (res, tx) = wrpc
            .invoke_get_container_data(
                &ObjectId {
                    container: container.to_string(),
                    object: name,
                },
                *range.start(),
                *range.end(),
            )
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.get-container-data`")?;
        // TODO: return a result directly
        let data = res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(data)
    }

    #[instrument(level = "trace", skip(self))]
    async fn has_object(&self, container: &str, name: String) -> anyhow::Result<bool> {
        use wrpc_interface_blobstore::{Blobstore, ObjectId};

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        let (res, tx) = wrpc
            .invoke_has_object(&ObjectId {
                container: container.to_string(),
                object: name,
            })
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.has-object`")?;
        // TODO: return a result directly
        let has = res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(has)
    }

    #[instrument(level = "trace", skip(self, value))]
    async fn write_data(
        &self,
        container: &str,
        name: String,
        mut value: Box<dyn AsyncRead + Sync + Send + Unpin>,
    ) -> anyhow::Result<()> {
        use wrpc_interface_blobstore::{Blobstore, ObjectId};

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        let mut buf = vec![];
        value
            .read_to_end(&mut buf)
            .await
            .context("failed to read value")?;
        let (res, tx) = wrpc
            .invoke_write_container_data(
                &ObjectId {
                    container: container.to_string(),
                    object: name,
                },
                stream::iter([buf.into()]),
            )
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.write-container-data`")?;
        // TODO: return a result directly
        res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    async fn delete_objects(&self, container: &str, names: Vec<String>) -> anyhow::Result<()> {
        use wrpc_interface_blobstore::Blobstore;

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        let (res, tx) = wrpc
            .invoke_delete_objects(container, names.iter().map(String::as_str))
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.write-container-data`")?;
        // TODO: return a result directly
        res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    async fn list_objects(
        &self,
        container: &str,
    ) -> anyhow::Result<Box<dyn Stream<Item = anyhow::Result<Vec<String>>> + Sync + Send + Unpin>>
    {
        use wrpc_interface_blobstore::Blobstore;

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        // TODO: implement a stream with limit and offset
        let (res, tx) = wrpc
            .invoke_list_container_objects(container, None, None)
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.list-container-objects`")?;
        let names = res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(names)
    }

    #[instrument(level = "trace", skip(self))]
    async fn object_info(
        &self,
        container: &str,
        name: String,
    ) -> anyhow::Result<blobstore::container::ObjectMetadata> {
        use wrpc_interface_blobstore::{Blobstore, ObjectId, ObjectMetadata};

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        let (res, tx) = wrpc
            .invoke_get_object_info(&ObjectId {
                container: container.to_string(),
                object: name.to_string(),
            })
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.get-object-info`")?;
        // TODO: return a result directly
        let ObjectMetadata { created_at, size } =
            res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(blobstore::container::ObjectMetadata {
            name,
            container: container.to_string(),
            created_at,
            size,
        })
    }

    #[instrument(level = "trace", skip(self))]
    async fn clear_container(&self, container: &str) -> anyhow::Result<()> {
        use wrpc_interface_blobstore::Blobstore;

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        let (res, tx) = wrpc
            .invoke_clear_container(container)
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.clear-container`")?;
        // TODO: return a result directly
        res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    async fn copy_object(
        &self,
        src_container: String,
        src_name: String,
        dest_container: String,
        dest_name: String,
    ) -> anyhow::Result<()> {
        use wrpc_interface_blobstore::{Blobstore, ObjectId};

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        let (res, tx) = wrpc
            .invoke_copy_object(
                &ObjectId {
                    container: src_container,
                    object: src_name,
                },
                &ObjectId {
                    container: dest_container,
                    object: dest_name,
                },
            )
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.copy-object`")?;
        // TODO: return a result directly
        res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    async fn move_object(
        &self,
        src_container: String,
        src_name: String,
        dest_container: String,
        dest_name: String,
    ) -> anyhow::Result<()> {
        use wrpc_interface_blobstore::{Blobstore, ObjectId};

        let wrpc = self.wrpc_blobstore_blobstore().await?;
        let (res, tx) = wrpc
            .invoke_move_object(
                &ObjectId {
                    container: src_container,
                    object: src_name,
                },
                &ObjectId {
                    container: dest_container,
                    object: dest_name,
                },
            )
            .await
            .context("failed to invoke `wrpc:blobstore/blobstore.move-object`")?;
        // TODO: return a result directly
        res.map_err(|err| anyhow!(err).context("function failed"))?;
        tx.await.context("failed to transmit parameters")?;
        Ok(())
    }
}

#[async_trait]
impl Bus for Handler {
    #[instrument(level = "trace", skip(self))]
    async fn identify_interface_target(
        &self,
        target_interface: &CallTargetInterface,
    ) -> Option<TargetEntity> {
        let links = self.interface_links.read().await;
        let targets = self.targets.read().await;
        let link_name = targets
            .get(target_interface)
            .map_or("default", String::as_str);
        let (namespace, package, interface) = target_interface.as_parts();

        // Determine the lattice target ID we should be sending to
        let lattice_target_id = links
            .get(link_name)
            .and_then(|packages| packages.get(&format!("{namespace}:{package}")))
            .and_then(|interfaces| interfaces.get(interface));

        // If we managed to find a target ID, convert it into an entity
        let target_entity = lattice_target_id.map(|id| {
            TargetEntity::Lattice(LatticeInterfaceTarget {
                id: id.clone(),
                interface: target_interface.clone(),
                link_name: link_name.to_string(),
            })
        });

        if target_entity.is_none() {
            debug!(
                ?links,
                interface,
                namespace,
                package,
                ?self.component_id,
                "component is not linked to a lattice target for the given interface"
            );
        }
        target_entity
    }

    /// Set the current link name in use by the handler, which is otherwise "default".
    ///
    /// Link names are important to set to differentiate similar operations (ex. `wasi:keyvalue/store.get`)
    /// that should go to different targets (ex. a capability provider like `kv-redis` vs `kv-vault`)
    #[instrument(level = "debug", skip(self))]
    async fn set_link_name(
        &self,
        link_name: String,
        interfaces: Vec<CallTargetInterface>,
    ) -> anyhow::Result<()> {
        let mut targets = self.targets.write().await;
        if link_name == "default" {
            for interface in interfaces {
                targets.remove(&interface);
            }
        } else {
            for interface in interfaces {
                targets.insert(interface, link_name.clone());
            }
        }
        Ok(())
    }

    #[instrument(level = "info", skip(self, params, instance, name), fields(interface = instance, function = name))]
    async fn call<T>(
        &self,
        target: TargetEntity,
        instance: &str,
        name: &str,
        params: Bytes,
    ) -> anyhow::Result<(T::Outgoing, T::Incoming)> {
        if let TargetEntity::Lattice(lit) = target {
            let rx = Subject::from(self.nats.new_inbox());
            let (result_rx, handshake_rx, nested) = try_join!(
                async {
                    self.nats
                        .subscribe(Subject::from(result_subject(&rx)))
                        .await
                        .context("failed to subscribe on result subject")
                },
                async {
                    self.nats
                        .subscribe(rx.clone())
                        .await
                        .context("failed to subscribe on handshake subject")
                },
                futures::future::try_join_all(paths.iter().map(|path| async {
                    self.nats
                        .subscribe(Subject::from(subscribe_path(&rx, path.as_ref())))
                        .await
                        .context("failed to subscribe on nested result subject")
                }))
            )?;
            let nested: SubscriberTree = zip(paths.iter(), nested).collect();
            ensure!(
                paths.is_empty() == nested.is_empty(),
                "failed to construct subscription tree"
            );
            let ServerInfo {
                mut max_payload, ..
            } = self.nats.server_info();
            max_payload = max_payload.saturating_sub(rx.len());
            let param_tx = Subject::from(invocation_subject(&self.prefix, instance, func));
            if let Some(headers) = cx {
                // based on https://github.com/nats-io/nats.rs/blob/0942c473ce56163fdd1fbc62762f8164e3afa7bf/async-nats/src/header.rs#L215-L224
                max_payload = max_payload
                    .saturating_sub(b"NATS/1.0\r\n".len())
                    .saturating_sub(b"\r\n".len());
                for (k, vs) in headers.iter() {
                    let k: &[u8] = k.as_ref();
                    for v in vs {
                        max_payload = max_payload
                            .saturating_sub(k.len())
                            .saturating_sub(b": ".len())
                            .saturating_sub(v.as_str().len())
                            .saturating_sub(b"\r\n".len());
                    }
                }
                trace!("publishing handshake");
                self.nats
                    .publish_with_reply_and_headers(
                        param_tx.clone(),
                        rx,
                        headers,
                        params.split_to(max_payload.min(params.len())),
                    )
                    .await
            } else {
                trace!("publishing handshake");
                self.nats
                    .publish_with_reply(
                        param_tx.clone(),
                        rx,
                        params.split_to(max_payload.min(params.len())),
                    )
                    .await
            }
            .context("failed to send handshake")?;
            Ok((
                ParamWriter::Root(RootParamWriter::new(
                    SubjectWriter::new(
                        Arc::clone(&self.nats),
                        param_tx.clone(),
                        self.nats.publish_sink(param_tx),
                    ),
                    handshake_rx,
                    params,
                )),
                Reader {
                    buffer: Bytes::default(),
                    incoming: result_rx,
                    nested: Arc::new(std::sync::Mutex::new(nested)),
                },
            ));
        } else {
            bail!(
                "component [{}] attempted to invoke a function [{}/{}] on an unknown target [{}]",
                self.component_id,
                instance,
                name,
                target.id().unwrap_or("<unknown>"),
            )
        }
    }
}

#[async_trait]
impl Config for Handler {
    #[instrument(level = "debug", skip_all)]
    async fn get(&self, key: &str) -> anyhow::Result<Result<Option<String>, ConfigError>> {
        let lock = self.config_data.read().await;
        let conf = lock.get_config().await;
        let data = conf.get(key).cloned();
        Ok(Ok(data))
    }

    #[instrument(level = "debug", skip_all)]
    async fn get_all(&self) -> anyhow::Result<Result<Vec<(String, String)>, ConfigError>> {
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

fn keyvalue_error_from_wrpc(err: wrpc::keyvalue::store::Error) -> keyvalue::store::Error {
    match err {
        wrpc::keyvalue::store::Error::NoSuchStore => keyvalue::store::Error::NoSuchStore,
        wrpc::keyvalue::store::Error::AccessDenied => keyvalue::store::Error::AccessDenied,
        wrpc::keyvalue::store::Error::Other(other) => keyvalue::store::Error::Other(other),
    }
}

#[async_trait]
impl KeyValueAtomics for Handler {
    #[instrument(level = "trace", skip(self))]
    async fn increment(
        &self,
        bucket: &str,
        key: String,
        delta: u64,
    ) -> anyhow::Result<Result<u64, keyvalue::store::Error>> {
        let wrpc = self.wrpc_keyvalue_atomics().await?;
        let res = wrpc::keyvalue::atomics::increment(&wrpc, bucket, &key, delta)
            .await
            .context("failed to invoke `wrpc:keyvalue/atomics.increment`")?;
        Ok(res.map_err(keyvalue_error_from_wrpc))
    }
}

#[async_trait]
impl KeyValueStore for Handler {
    #[instrument(level = "trace", skip(self))]
    async fn get(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, keyvalue::store::Error>> {
        let wrpc = self.wrpc_keyvalue_store().await?;
        let res = wrpc::keyvalue::store::get(&wrpc, bucket, &key)
            .await
            .context("failed to invoke `wrpc:keyvalue/store.get`")?;
        Ok(res.map_err(keyvalue_error_from_wrpc))
    }

    #[instrument(level = "trace", skip(self, value))]
    async fn set(
        &self,
        bucket: &str,
        key: String,
        value: Vec<u8>,
    ) -> anyhow::Result<Result<(), keyvalue::store::Error>> {
        let wrpc = self.wrpc_keyvalue_store().await?;
        let res = wrpc::keyvalue::store::set(&wrpc, bucket, &key, &value)
            .await
            .context("failed to invoke `wrpc:keyvalue/store.set`")?;
        Ok(res.map_err(keyvalue_error_from_wrpc))
    }

    #[instrument(level = "trace", skip(self))]
    async fn delete(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<Result<(), keyvalue::store::Error>> {
        let wrpc = self.wrpc_keyvalue_store().await?;
        let res = wrpc::keyvalue::store::delete(&wrpc, bucket, &key)
            .await
            .context("failed to invoke `wrpc:keyvalue/store.delete`")?;
        Ok(res.map_err(keyvalue_error_from_wrpc))
    }

    #[instrument(level = "trace", skip(self))]
    async fn exists(
        &self,
        bucket: &str,
        key: String,
    ) -> anyhow::Result<Result<bool, keyvalue::store::Error>> {
        let wrpc = self.wrpc_keyvalue_store().await?;
        let res = wrpc::keyvalue::store::exists(&wrpc, bucket, &key)
            .await
            .context("failed to invoke `wrpc:keyvalue/store.exists`")?;
        Ok(res.map_err(keyvalue_error_from_wrpc))
    }

    #[instrument(level = "trace", skip(self))]
    async fn list_keys(
        &self,
        bucket: &str,
        cursor: Option<u64>,
    ) -> anyhow::Result<Result<keyvalue::store::KeyResponse, keyvalue::store::Error>> {
        let wrpc = self.wrpc_keyvalue_store().await?;
        match wrpc::keyvalue::store::list_keys(&wrpc, bucket, cursor)
            .await
            .context("failed to invoke `wrpc:keyvalue/store.list_keys`")?
        {
            Ok(wrpc::keyvalue::store::KeyResponse { keys, cursor }) => {
                Ok(Ok(keyvalue::store::KeyResponse { keys, cursor }))
            }
            Err(err) => Ok(Err(keyvalue_error_from_wrpc(err))),
        }
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
                    component_id = self.component_id,
                    ?level,
                    context,
                    "{message}"
                );
            }
            logging::Level::Debug => {
                tracing::event!(
                    tracing::Level::DEBUG,
                    component_id = self.component_id,
                    ?level,
                    context,
                    "{message}"
                );
            }
            logging::Level::Info => {
                tracing::event!(
                    tracing::Level::INFO,
                    component_id = self.component_id,
                    ?level,
                    context,
                    "{message}"
                );
            }
            logging::Level::Warn => {
                tracing::event!(
                    tracing::Level::WARN,
                    component_id = self.component_id,
                    ?level,
                    context,
                    "{message}"
                );
            }
            logging::Level::Error => {
                tracing::event!(
                    tracing::Level::ERROR,
                    component_id = self.component_id,
                    ?level,
                    context,
                    "{message}"
                );
            }
            logging::Level::Critical => {
                tracing::event!(
                    tracing::Level::ERROR,
                    component_id = self.component_id,
                    ?level,
                    context,
                    "{message}"
                );
            }
        };
        Ok(())
    }
}

#[async_trait]
impl Messaging for Handler {
    #[instrument(level = "trace", skip(self, body))]
    async fn request(
        &self,
        subject: String,
        body: Vec<u8>,
        timeout: Duration,
    ) -> anyhow::Result<Result<messaging::types::BrokerMessage, String>> {
        let wrpc = self.wrpc_messaging_consumer().await?;
        let res = wasmcloud::messaging::consumer::request(
            &wrpc,
            &subject,
            &body,
            timeout.as_millis().try_into().unwrap_or(u32::MAX),
        )
        .await
        .context("failed to invoke `wasmcloud:messaging/consumer.request`")?;
        Ok(res.map(
            |wasmcloud::messaging::types::BrokerMessage {
                 subject,
                 body,
                 reply_to,
             }| {
                messaging::types::BrokerMessage {
                    subject,
                    body,
                    reply_to,
                }
            },
        ))
    }

    #[instrument(level = "trace", skip_all)]
    async fn publish(
        &self,
        messaging::types::BrokerMessage {
            subject,
            body,
            reply_to,
        }: messaging::types::BrokerMessage,
    ) -> anyhow::Result<Result<(), String>> {
        let wrpc = self.wrpc_messaging_consumer().await?;
        wasmcloud::messaging::consumer::publish(
            &wrpc,
            &wasmcloud::messaging::types::BrokerMessage {
                subject,
                body,
                reply_to,
            },
        )
        .await
        .context("failed to invoke `wasmcloud:messaging/consumer.publish`")
    }
}

#[async_trait]
impl OutgoingHttp for Handler {
    #[instrument(level = "debug", skip_all)]
    async fn handle(
        &self,
        request: wasmtime_wasi_http::types::OutgoingRequest,
    ) -> anyhow::Result<
        Result<
            http::Response<HyperIncomingBody>,
            wasmtime_wasi_http::bindings::http::types::ErrorCode,
        >,
    > {
        use wrpc_interface_http::OutgoingHandler;
        // Reading a trace context should _never_ block because writing happens once at the beginning of a component
        // invocation. If it does block here, it's a bug in the runtime, and it's better to deal with a
        // disconnected trace than to block on the invocation for an extended period of time.
        if let Ok(trace_context) = self.trace_ctx.try_read() {
            wasmcloud_tracing::context::attach_span_context(&trace_context);
        }

        let wrpc = self.wrpc_http_outgoing_handler().await?;
        let (res, body_errors, tx) = wrpc
            .invoke_handle_wasmtime(request)
            .await
            .context("failed to invoke `wrpc:http/outgoing-handler.handle`")?;
        spawn(async move {
            if let Err(err) = tx.await {
                error!(?err, "failed to transmit parameter values");
            }
        });
        // TODO: Do not ignore outgoing body errors
        let _ = body_errors;
        Ok(res)
    }
}

#[derive(Clone, Debug)]
pub struct SubjectWriter {
    nats: Arc<async_nats::Client>,
    tx: Subject,
    publisher: Publisher,
}

impl SubjectWriter {
    fn new(nats: Arc<async_nats::Client>, tx: Subject, publisher: Publisher) -> Self {
        Self {
            nats,
            tx,
            publisher,
        }
    }
}

impl wrpc_transport::Index<Self> for SubjectWriter {
    #[instrument(level = "trace", skip(self))]
    fn index(&self, path: &[usize]) -> anyhow::Result<Self> {
        Ok(Self {
            nats: Arc::clone(&self.nats),
            tx: index_path(self.tx.as_str(), path).into(),
            publisher: self.publisher.clone(),
        })
    }
}

impl AsyncWrite for SubjectWriter {
    #[instrument(level = "trace", skip_all, ret, fields(buf = format!("{buf:02x?}")))]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        trace!("polling for readiness");
        match self.publisher.poll_ready_unpin(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(..)) => return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
            Poll::Ready(Ok(())) => {}
        }
        let ServerInfo { max_payload, .. } = self.nats.server_info();
        if max_payload == 0 {
            return Poll::Ready(Err(std::io::ErrorKind::WriteZero.into()));
        }
        if buf.len() > max_payload {
            (buf, _) = buf.split_at(max_payload);
        }
        trace!("starting send");
        match self.publisher.start_send_unpin(Bytes::copy_from_slice(buf)) {
            Ok(()) => Poll::Ready(Ok(buf.len())),
            Err(..) => Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into())),
        }
    }

    #[instrument(level = "trace", skip_all, ret)]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        trace!("flushing");
        self.publisher
            .poll_flush_unpin(cx)
            .map_err(|_| std::io::ErrorKind::BrokenPipe.into())
    }

    #[instrument(level = "trace", skip_all, ret)]
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        trace!("writing empty buffer to shut down stream");
        ready!(self.as_mut().poll_write(cx, &[]))?;
        trace!("closing");
        self.publisher
            .poll_close_unpin(cx)
            .map_err(|_| std::io::ErrorKind::BrokenPipe.into())
    }
}

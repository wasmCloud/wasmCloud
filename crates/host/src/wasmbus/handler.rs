use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context as _};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use tokio::sync::RwLock;
use tracing::{debug, error, instrument};
use wasmcloud_core::LatticeTarget;
use wasmcloud_runtime::capability::config::runtime::ConfigError;
use wasmcloud_runtime::capability::logging::logging;
use wasmcloud_runtime::capability::{
    Bus, CallTargetInterface, Config, LatticeInterfaceTarget, Logging, TargetEntity,
};
use wasmcloud_tracing::context::TraceContextInjector;
use wasmtime::component::{Type, Val};
use wasmtime_wasi_http::body::{HyperIncomingBody, HyperOutgoingBody};
use wasmtime_wasi_http::types::OutgoingRequestConfig;
use wit_parser::Function;
use wrpc_runtime_wasmtime::read_value;
use wrpc_transport::Index;
use wrpc_transport::Session;
use wrpc_transport::{Invocation, Invoke};
use wrpc_transport_nats::{ClientErrorWriter, IndexedParamWriter, ParamWriter, Reader};

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
    pub polyfills: Arc<HashMap<String, HashMap<String, Function>>>,

    pub exports: Arc<HashMap<String, HashMap<String, Function>>>,

    /// Reference to store for collection of instances
    // pub store:,
    pub invocation_timeout: Duration,

    pub func_paths_map: Arc<HashMap<(Arc<String>, Arc<String>), Arc<Vec<Arc<[Option<usize>]>>>>>,
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
            exports: self.exports.clone(),
            func_paths_map: self.func_paths_map.clone(),
        }
    }

    /// Produces a wrpc builder that can build wrpc clients
    pub fn wrpc_builder(
        &self,
        LatticeInterfaceTarget { id, link_name, .. }: &LatticeInterfaceTarget,
    ) -> crate::wrpc::ClientBuilder {
        let injector = TraceContextInjector::default_with_span();
        let mut headers = injector_to_headers(&injector);
        headers.insert("source-id", self.component_id.as_str());
        headers.insert("link-name", link_name.as_str());
        crate::wrpc::ClientBuilder::new(Arc::clone(&self.nats), &self.lattice, &id)
    }

    async fn wrpc_blobstore_blobstore(&self) -> anyhow::Result<wasmcloud_core::wrpc::Client> {
        let lit = self
            .identify_wrpc_target(&CallTargetInterface::from_parts((
                "wasi",
                "blobstore",
                "blobstore",
            )))
            .await
            .context("unknown `wasi:blobstore/blobstore` target")?;
        Ok(self.wrpc_builder(&lit))
    }

    #[instrument(level = "trace", skip(self))]
    async fn wrpc_http_outgoing_handler(&self) -> anyhow::Result<wasmcloud_core::wrpc::Client> {
        let lit = self
            .identify_wrpc_target(&CallTargetInterface::from_parts((
                "wasi",
                "http",
                "outgoing-handler",
            )))
            .await
            .context("unknown `wasi:http/outgoing-handler` target")?;
        Ok(self.wrpc_builder(&lit))
    }

    #[instrument(level = "trace", skip(self))]
    async fn wrpc_keyvalue_atomics(&self) -> anyhow::Result<crate::wrpc::ClientBuilder> {
        let lit = self
            .identify_wrpc_target(&CallTargetInterface::from_parts((
                "wasi", "keyvalue", "atomics",
            )))
            .await
            .context("unknown `wasi:keyvalue/atomics` target")?;
        Ok(self.wrpc_builder(&lit))
    }

    #[instrument(level = "trace", skip(self))]
    async fn wrpc_keyvalue_store(&self) -> anyhow::Result<crate::wrpc::ClientBuilder> {
        let lit = self
            .identify_wrpc_target(&CallTargetInterface::from_parts((
                "wasi", "keyvalue", "store",
            )))
            .await
            .context("unknown `wasi:keyvalue/store` target")?;
        Ok(self.wrpc_builder(&lit))
    }

    async fn wrpc_messaging_consumer(&self) -> anyhow::Result<crate::wrpc::ClientBuilder> {
        let lit = self
            .identify_wrpc_target(&CallTargetInterface::from_parts((
                "wasmcloud",
                "messaging",
                "consumer",
            )))
            .await
            .context("unknown `wasmcloud:messaging/consumer` target")?;
        Ok(self.wrpc_client(&lit))
    }
}

// #[async_trait]
// impl Blobstore for Handler {
//     #[instrument(level = "trace", skip(self))]
//     async fn create_container(&self, params: Bytes, name: &str) -> anyhow::Result<()> {
//         let builder = self.wrpc_blobstore_blobstore().await?;
//         let wrpc = builder.build();

//         // Get exported function for this instance
//         let exported_func = self
//             .exports
//             .get(&self.component_id)
//             .with_context(|| format!("component_id `{}` not found", self.component_id))?
//             .get("create_container")
//             .with_context(|| "function `create_container` not found")?;

//         // Initialize params with default values
//         let mut params = vec![Val::Bool(false); 1]; // Assuming one parameter for now (the name param)
//         let param_types: Vec<Type> = exported_func
//             .params
//             .iter()
//             .map(|(_, ty)| ty.clone())
//             .collect();

//         // Encode params
//         let mut params_buf = BytesMut::default();
//         let mut deferred = vec![];
//         for (v, ref ty) in zip(&params, param_types) {
//             let mut enc = ValEncoder::new(&mut params_buf, ty);
//             enc.encode(v, &mut params_buf)
//                 .context("failed to encode parameter")?;
//             deferred.push(enc.deferred);
//         }
//         // TODO - take care of async params

//         let Invocation {
//             outgoing,
//             incoming,
//             session,
//         } = wrpc
//             .invoke(
//                 Some(builder.headers()),
//                 &builder.component_id(),
//                 "create_container".to_string(),
//                 params_buf.freeze(),
//                 &[],
//             )
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.create-container`")?;

//         try_join!(
//             async {
//                 try_join_all(
//                     zip(0.., deferred)
//                         .filter_map(|(i, f)| f.map(|f| (outgoing.index(&[i]), f)))
//                         .map(|(w, f)| async move {
//                             let w = w.map_err(Into::into)?;
//                             f(w).await
//                         }),
//                 )
//                 .await
//                 .context("failed to write asynchronous parameters")?;
//                 pin!(outgoing)
//                     .shutdown()
//                     .await
//                     .context("failed to shutdown outgoing stream")
//             },
//             async {
//                 let mut incoming = pin!(incoming);
//                 for (i, (v, ref ty)) in zip(results, func.results()).enumerate() {
//                     read_value(&mut store, &mut incoming, v, ty, &[i])
//                         .await
//                         .context("failed to decode result value")?;
//                 }
//                 Ok(())
//             },
//         )?;
//         match session.finish(Ok(())).await.map_err(Into::into)? {
//             Ok(()) => Ok(()),
//             Err(err) => bail!(anyhow!("{err}").context("session failed")),
//         }
//     }

//     #[instrument(level = "trace", skip(self))]
//     async fn container_exists(&self, name: &str) -> anyhow::Result<bool> {
//         let wrpc = self.wrpc_blobstore_blobstore().await?.build();

//         let (res, tx) = wrpc
//             .invoke_container_exists(name)
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.container-exists`")?;
//         // TODO: return a result directly
//         let exists = res.map_err(|err| anyhow!(err).context("function failed"))?;
//         tx.await.context("failed to transmit parameters")?;
//         Ok(exists)
//     }

//     #[instrument(level = "trace", skip(self))]
//     async fn delete_container(&self, name: &str) -> anyhow::Result<()> {
//         use wrpc_interface_blobstore::Blobstore;

//         let wrpc = self.wrpc_blobstore_blobstore().await?;
//         let (res, tx) = wrpc
//             .invoke_delete_container(name)
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.delete-container`")?;
//         // TODO: return a result directly
//         res.map_err(|err| anyhow!(err).context("function failed"))?;
//         tx.await.context("failed to transmit parameters")?;
//         Ok(())
//     }

//     #[instrument(level = "trace", skip(self))]
//     async fn container_info(
//         &self,
//         name: &str,
//     ) -> anyhow::Result<blobstore::container::ContainerMetadata> {
//         use wrpc_interface_blobstore::{Blobstore, ContainerMetadata};

//         let wrpc = self.wrpc_blobstore_blobstore().await?;
//         let (res, tx) = wrpc
//             .invoke_get_container_info(name)
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.get-container-info`")?;
//         // TODO: return a result directly
//         let ContainerMetadata { created_at } =
//             res.map_err(|err| anyhow!(err).context("function failed"))?;
//         tx.await.context("failed to transmit parameters")?;
//         Ok(blobstore::container::ContainerMetadata {
//             name: name.to_string(),
//             created_at,
//         })
//     }

//     #[instrument(level = "trace", skip(self))]
//     async fn get_data(
//         &self,
//         container: &str,
//         name: String,
//         range: RangeInclusive<u64>,
//     ) -> anyhow::Result<IncomingInputStream> {
//         use wrpc_interface_blobstore::{Blobstore, ObjectId};

//         let wrpc = self.wrpc_blobstore_blobstore().await?;
//         let (res, tx) = wrpc
//             .invoke_get_container_data(
//                 &ObjectId {
//                     container: container.to_string(),
//                     object: name,
//                 },
//                 *range.start(),
//                 *range.end(),
//             )
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.get-container-data`")?;
//         // TODO: return a result directly
//         let data = res.map_err(|err| anyhow!(err).context("function failed"))?;
//         tx.await.context("failed to transmit parameters")?;
//         Ok(data)
//     }

//     #[instrument(level = "trace", skip(self))]
//     async fn has_object(&self, container: &str, name: String) -> anyhow::Result<bool> {
//         use wrpc_interface_blobstore::{Blobstore, ObjectId};

//         let wrpc = self.wrpc_blobstore_blobstore().await?;
//         let (res, tx) = wrpc
//             .invoke_has_object(&ObjectId {
//                 container: container.to_string(),
//                 object: name,
//             })
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.has-object`")?;
//         // TODO: return a result directly
//         let has = res.map_err(|err| anyhow!(err).context("function failed"))?;
//         tx.await.context("failed to transmit parameters")?;
//         Ok(has)
//     }

//     #[instrument(level = "trace", skip(self, value))]
//     async fn write_data(
//         &self,
//         container: &str,
//         name: String,
//         mut value: Box<dyn AsyncRead + Sync + Send + Unpin>,
//     ) -> anyhow::Result<()> {
//         use wrpc_interface_blobstore::{Blobstore, ObjectId};

//         let wrpc = self.wrpc_blobstore_blobstore().await?;
//         let mut buf = vec![];
//         value
//             .read_to_end(&mut buf)
//             .await
//             .context("failed to read value")?;
//         let (res, tx) = wrpc
//             .invoke_write_container_data(
//                 &ObjectId {
//                     container: container.to_string(),
//                     object: name,
//                 },
//                 stream::iter([buf.into()]),
//             )
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.write-container-data`")?;
//         // TODO: return a result directly
//         res.map_err(|err| anyhow!(err).context("function failed"))?;
//         tx.await.context("failed to transmit parameters")?;
//         Ok(())
//     }

//     #[instrument(level = "trace", skip(self))]
//     async fn delete_objects(&self, container: &str, names: Vec<String>) -> anyhow::Result<()> {
//         use wrpc_interface_blobstore::Blobstore;

//         let wrpc = self.wrpc_blobstore_blobstore().await?;
//         let (res, tx) = wrpc
//             .invoke_delete_objects(container, names.iter().map(String::as_str))
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.write-container-data`")?;
//         // TODO: return a result directly
//         res.map_err(|err| anyhow!(err).context("function failed"))?;
//         tx.await.context("failed to transmit parameters")?;
//         Ok(())
//     }

//     #[instrument(level = "trace", skip(self))]
//     async fn list_objects(
//         &self,
//         container: &str,
//     ) -> anyhow::Result<Box<dyn Stream<Item = anyhow::Result<Vec<String>>> + Sync + Send + Unpin>>
//     {
//         use wrpc_interface_blobstore::Blobstore;

//         let wrpc = self.wrpc_blobstore_blobstore().await?;
//         // TODO: implement a stream with limit and offset
//         let (res, tx) = wrpc
//             .invoke_list_container_objects(container, None, None)
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.list-container-objects`")?;
//         let names = res.map_err(|err| anyhow!(err).context("function failed"))?;
//         tx.await.context("failed to transmit parameters")?;
//         Ok(names)
//     }

//     #[instrument(level = "trace", skip(self))]
//     async fn object_info(
//         &self,
//         container: &str,
//         name: String,
//     ) -> anyhow::Result<blobstore::container::ObjectMetadata> {
//         use wrpc_interface_blobstore::{Blobstore, ObjectId, ObjectMetadata};

//         let wrpc = self.wrpc_blobstore_blobstore().await?;
//         let (res, tx) = wrpc
//             .invoke_get_object_info(&ObjectId {
//                 container: container.to_string(),
//                 object: name.to_string(),
//             })
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.get-object-info`")?;
//         // TODO: return a result directly
//         let ObjectMetadata { created_at, size } =
//             res.map_err(|err| anyhow!(err).context("function failed"))?;
//         tx.await.context("failed to transmit parameters")?;
//         Ok(blobstore::container::ObjectMetadata {
//             name,
//             container: container.to_string(),
//             created_at,
//             size,
//         })
//     }

//     #[instrument(level = "trace", skip(self))]
//     async fn clear_container(&self, container: &str) -> anyhow::Result<()> {
//         use wrpc_interface_blobstore::Blobstore;

//         let wrpc = self.wrpc_blobstore_blobstore().await?;
//         let (res, tx) = wrpc
//             .invoke_clear_container(container)
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.clear-container`")?;
//         // TODO: return a result directly
//         res.map_err(|err| anyhow!(err).context("function failed"))?;
//         tx.await.context("failed to transmit parameters")?;
//         Ok(())
//     }

//     #[instrument(level = "trace", skip(self))]
//     async fn copy_object(
//         &self,
//         src_container: String,
//         src_name: String,
//         dest_container: String,
//         dest_name: String,
//     ) -> anyhow::Result<()> {
//         use wrpc_interface_blobstore::{Blobstore, ObjectId};

//         let wrpc = self.wrpc_blobstore_blobstore().await?;
//         let (res, tx) = wrpc
//             .invoke_copy_object(
//                 &ObjectId {
//                     container: src_container,
//                     object: src_name,
//                 },
//                 &ObjectId {
//                     container: dest_container,
//                     object: dest_name,
//                 },
//             )
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.copy-object`")?;
//         // TODO: return a result directly
//         res.map_err(|err| anyhow!(err).context("function failed"))?;
//         tx.await.context("failed to transmit parameters")?;
//         Ok(())
//     }

//     #[instrument(level = "trace", skip(self))]
//     async fn move_object(
//         &self,
//         src_container: String,
//         src_name: String,
//         dest_container: String,
//         dest_name: String,
//     ) -> anyhow::Result<()> {
//         use wrpc_interface_blobstore::{Blobstore, ObjectId};

//         let wrpc = self.wrpc_blobstore_blobstore().await?;
//         let (res, tx) = wrpc
//             .invoke_move_object(
//                 &ObjectId {
//                     container: src_container,
//                     object: src_name,
//                 },
//                 &ObjectId {
//                     container: dest_container,
//                     object: dest_name,
//                 },
//             )
//             .await
//             .context("failed to invoke `wrpc:blobstore/blobstore.move-object`")?;
//         // TODO: return a result directly
//         res.map_err(|err| anyhow!(err).context("function failed"))?;
//         tx.await.context("failed to transmit parameters")?;
//         Ok(())
//     }
// }

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
    async fn call(
        &self,
        target: TargetEntity,
        instance: &str,
        name: &str,
        params: Vec<wrpc_transport::Value>,
    ) -> anyhow::Result<Vec<wrpc_transport::Value>> {
        if let TargetEntity::Lattice(LatticeInterfaceTarget { id, .. }) = target {
            let injector = TraceContextInjector::default_with_span();
            let mut headers = injector_to_headers(&injector);
            headers.insert("source-id", self.component_id.as_str());
            let builder = self.wrpc_builder(&lit);
            let client = builder.build();

            let invocation = client
                .invoke(Some(headers), instance, name, params.freeze(), func_paths)
                .await
                .map_err(anyhow::Error::from)
                .with_context(|| {
                    format!("failed to invoke `{instance}.{name}` polyfill via wRPC")
                })?;

            Ok(invocation)
        } else {
            bail!("component attempted to invoke a function on an unknown target")
        }
    }

    fn get_func_paths(&self, instance: &str, name: &str) -> Option<Arc<Vec<Arc<[Option<usize>]>>>> {
        self.func_paths_map
            .get(&(Arc::new(instance.to_string()), Arc::new(name.to_string())))
            .cloned()
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

// TODO - finalise implementation
// #[async_trait]
// impl Messaging for Handler {
//     #[instrument(level = "trace", skip(self, body))]
//     async fn request(
//         &self,
//         subject: String,
//         body: Vec<u8>,
//         timeout: Duration,
//     ) -> anyhow::Result<Result<messaging::types::BrokerMessage, String>> {
//         let wrpc = self.wrpc_messaging_consumer().await?;
//         let res = wasmcloud::messaging::consumer::request(
//             wrpc.wrpc_client(),
//             &subject,
//             &body,
//             timeout.as_millis().try_into().unwrap_or(u32::MAX),
//         )
//         .await
//         .context("failed to invoke `wasmcloud:messaging/consumer.request`")?;
//         Ok(res.map(
//             |wasmcloud::messaging::types::BrokerMessage {
//                  subject,
//                  body,
//                  reply_to,
//              }| {
//                 messaging::types::BrokerMessage {
//                     subject,
//                     body,
//                     reply_to,
//                 }
//             },
//         ))
//     }

//     #[instrument(level = "trace", skip_all)]
//     async fn publish(
//         &self,
//         messaging::types::BrokerMessage {
//             subject,
//             body,
//             reply_to,
//         }: messaging::types::BrokerMessage,
//     ) -> anyhow::Result<Result<(), String>> {
//         let wrpc = self.wrpc_messaging_consumer().await?;
//         wasmcloud::messaging::consumer::publish(
//             wrpc.wrpc_client(),
//             &wasmcloud::messaging::types::BrokerMessage {
//                 subject,
//                 body,
//                 reply_to,
//             },
//         )
//         .await
//         .context("failed to invoke `wasmcloud:messaging/consumer.publish`")
//     }
// }

// #[async_trait]
// impl OutgoingHttp for Handler {
//     #[instrument(level = "trace", skip_all)]
//     async fn handle(
//         &self,
//         request: hyper::Request<HyperOutgoingBody>,
//         options: OutgoingRequestConfig,
//     ) -> anyhow::Result<
//         Result<
//             http::Response<HyperIncomingBody>,
//             wasmtime_wasi_http::bindings::http::types::ErrorCode,
//         >,
//     > {
//         let wrpc = self.wrpc_http_outgoing_handler().await?;
//         let (res, body_errors, tx) = wrpc
//             .invoke_handle_wasmtime(request)
//             .await
//             .context("failed to invoke `wrpc:http/outgoing-handler.handle`")?;
//         spawn(async move {
//             if let Err(err) = tx.await {
//                 error!(?err, "failed to transmit parameter values");
//             }
//         });
//         // TODO: Do not ignore outgoing body errors
//         let _ = body_errors;
//         Ok(res)
//     }
// }

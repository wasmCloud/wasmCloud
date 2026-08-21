use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context as _;
use tokio::sync::RwLock;
use tracing::info;

use crate::engine::workload::{ResolvedWorkload, UnresolvedWorkload, WorkloadItem};
use crate::observability::Meters;
use crate::plugin::{HostPlugin, WitInterfaces, WorkloadTracker};
use crate::wit::{WitInterface, WitWorld};

pub(super) mod config;
pub(super) mod conn;
pub(super) mod handles;
mod interfaces;
pub(super) mod policy;
mod subscriber;

use config::NatsConfig;
use conn::{ConnHandle, ConnectionRegistry};

pub(super) mod bindings {
    // Imports-only world: installs host imports into the linker and is the
    // canonical source for the shared types.
    crate::wasmtime::component::bindgen!({
        world: "nats-imports",
        imports: { default: async | trappable | tracing },
        with: {
            "wasmcloud:nats/jetstream.message-handle": super::handles::MessageHandle,
            "wasmcloud:nats/jetstream.pull-consumer": super::handles::PullConsumerHandle,
            "wasmcloud:nats/kv.bucket": super::handles::BucketHandle,
        },
    });
}

// Each handler world lives in its own module so their duplicate import types
// don't collide, and so a component exporting only one handler still
// pre-instantiates.
pub(super) mod jetstream_bindings {
    crate::wasmtime::component::bindgen!({
        world: "nats-js-processor",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
        with: {
            "wasmcloud:nats/jetstream.message-handle": super::handles::MessageHandle,
            "wasmcloud:nats/jetstream.pull-consumer": super::handles::PullConsumerHandle,
        },
    });
}

pub(super) mod core_bindings {
    crate::wasmtime::component::bindgen!({
        world: "nats-subscriber",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
    });
}

pub(super) mod kv_bindings {
    crate::wasmtime::component::bindgen!({
        world: "nats-kv-watcher",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
        with: {
            "wasmcloud:nats/kv.bucket": super::handles::BucketHandle,
        },
    });
}

pub(super) const PLUGIN_NATS_ID: &str = "wasmcloud-nats";

const NATS_VERSION: &str = "0.1.0";

/// Per-component subscription state.
pub struct ComponentData {
    pub(super) jetstream_subs: Vec<JetStreamSubscriptionConfig>,
    pub(super) core_subs: Vec<CoreSubscriptionConfig>,
    pub(super) kv_watches: Vec<KvWatchConfig>,
    pub(super) cancel_token: tokio_util::sync::CancellationToken,
}

#[derive(Clone, Debug)]
pub(super) struct JetStreamSubscriptionConfig {
    pub stream: String,
    pub filter_subject: String,
    pub deliver_policy: String,
    pub queue_group: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct CoreSubscriptionConfig {
    pub subject: String,
    pub queue_group: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct KvWatchConfig {
    pub bucket: String,
    pub filter: String,
}

/// Parses `STREAM:filter[:policy[:queue]]`, comma separated.
fn parse_jetstream_subscriptions(raw: &str) -> anyhow::Result<Vec<JetStreamSubscriptionConfig>> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let parts: Vec<&str> = entry.splitn(4, ':').collect();
            let (Some(stream), Some(filter_subject)) = (parts.first(), parts.get(1)) else {
                anyhow::bail!(
                    "invalid jetstream subscription `{entry}`, expected STREAM:filter[:policy[:queue]]"
                )
            };
            if stream.is_empty() || filter_subject.is_empty() {
                anyhow::bail!("jetstream subscription `{entry}` has an empty stream or filter")
            }
            let deliver_policy = parts.get(2).copied().unwrap_or("new");
            if !matches!(deliver_policy, "all" | "last" | "last-per-subject" | "new") {
                anyhow::bail!(
                    "jetstream subscription `{entry}` has an unknown deliver policy \
                     `{deliver_policy}`; expected all, last, last-per-subject, or new"
                )
            }
            Ok(JetStreamSubscriptionConfig {
                stream: (*stream).to_string(),
                filter_subject: (*filter_subject).to_string(),
                deliver_policy: deliver_policy.to_string(),
                queue_group: parts.get(3).map(|s| (*s).to_string()),
            })
        })
        .collect()
}

/// Parses `subject[:queue]`, comma separated.
fn parse_core_subscriptions(raw: &str) -> anyhow::Result<Vec<CoreSubscriptionConfig>> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let mut parts = entry.splitn(2, ':');
            let subject = parts.next().unwrap_or_default();
            if subject.is_empty() {
                anyhow::bail!("core subscription `{entry}` has an empty subject")
            }
            Ok(CoreSubscriptionConfig {
                subject: subject.to_string(),
                queue_group: parts.next().map(str::to_string),
            })
        })
        .collect()
}

/// Parses `bucket:filter`, comma separated.
fn parse_kv_watches(raw: &str) -> anyhow::Result<Vec<KvWatchConfig>> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let Some((bucket, filter)) = entry.split_once(':') else {
                anyhow::bail!("invalid kv watch `{entry}`, expected bucket:filter")
            };
            if bucket.is_empty() || filter.is_empty() {
                anyhow::bail!("kv watch `{entry}` has an empty bucket or filter")
            }
            Ok(KvWatchConfig {
                bucket: bucket.to_string(),
                filter: filter.to_string(),
            })
        })
        .collect()
}

/// `wasmcloud:nats` host plugin — NATS-native capabilities split by interface.
pub struct WasmcloudNats {
    pub(super) tracker: Arc<RwLock<WorkloadTracker<(), ComponentData>>>,
    pub(super) connections: Arc<ConnectionRegistry>,
    pub(super) meters: Arc<RwLock<Meters>>,
    /// Subjects the host itself uses, denied to every workload.
    lattice_prefixes: Vec<String>,
}

impl Default for WasmcloudNats {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmcloudNats {
    pub fn new() -> Self {
        Self {
            tracker: Arc::new(RwLock::new(WorkloadTracker::default())),
            connections: Arc::new(ConnectionRegistry::default()),
            meters: Default::default(),
            lattice_prefixes: Vec::new(),
        }
    }

    /// Denies the host's own lattice subject space to every workload.
    pub fn with_lattice_prefixes(mut self, prefixes: Vec<String>) -> Self {
        self.lattice_prefixes = prefixes;
        self
    }

    /// Resolves the calling workload's connection.
    pub(super) async fn conn_for(&self, workload_id: &str) -> Option<Arc<ConnHandle>> {
        self.connections.get(workload_id).await
    }
}

/// Collects this plugin's interfaces out of a bound set.
fn nats_interfaces<'a>(interfaces: &'a WitInterfaces<'_>) -> Vec<&'a WitInterface> {
    interfaces
        .iter()
        .filter(|i| i.namespace == "wasmcloud" && i.package == "nats")
        .collect()
}

fn serves(interface: &WitInterface, names: &[&str]) -> bool {
    interface
        .interfaces
        .iter()
        .any(|name| names.contains(&name.as_str()))
}

#[async_trait::async_trait]
impl HostPlugin for WasmcloudNats {
    fn id(&self) -> &'static str {
        PLUGIN_NATS_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                format!("wasmcloud:nats/types,core,jetstream,kv@{NATS_VERSION}").as_str(),
            )]),
            exports: HashSet::from([WitInterface::from(
                format!("wasmcloud:nats/jetstream-handler,core-handler,kv-handler@{NATS_VERSION}")
                    .as_str(),
            )]),
        }
    }

    async fn inject_meters(&self, meters: &Meters) {
        *self.meters.write().await = meters.clone();
    }

    /// Validates config, resolves credentials, and opens the workload's own
    /// connection. Failing here fails the deployment rather than the first call.
    async fn on_workload_bind(
        &self,
        workload: &UnresolvedWorkload,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        let bound = nats_interfaces(&interfaces);
        if bound.is_empty() {
            return Ok(());
        }

        let workload_id = workload.id();
        for interface in bound {
            let mut config = NatsConfig::from_map(&interface.config).with_context(|| {
                format!(
                    "invalid wasmcloud:nats configuration for workload `{workload_id}` \
                     interface `{}`",
                    interface.instance()
                )
            })?;

            // Scope request-reply to this workload so two workloads on one
            // server cannot observe each other's responses.
            if config.inbox_prefix.is_none() {
                config.inbox_prefix = Some(conn::workload_inbox_prefix(workload_id));
            }

            // One connection per workload. A capability call names no binding,
            // so a second differing config would leave the grant a call is
            // checked against undefined.
            if self
                .connections
                .has_conflicting(workload_id, &config.connection_key())
                .await
            {
                anyhow::bail!(
                    "workload `{workload_id}` binds wasmcloud:nats more than once with \
                     different configuration; a capability call cannot be attributed to one \
                     of them. Use a single binding until named multi-cluster routing lands"
                )
            }

            let handle = self
                .connections
                .acquire(workload_id, &config, self.lattice_prefixes.clone())
                .await?;

            info!(
                workload_id,
                servers = config.servers.join(","),
                auth = config.auth.kind(),
                server_version = handle.server_version,
                "opened NATS connection"
            );
        }

        Ok(())
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        let bound = nats_interfaces(&interfaces);
        if bound.is_empty() {
            return Ok(());
        }

        let linker = item.linker();
        bindings::wasmcloud::nats::types::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
            linker,
            crate::engine::ctx::extract_active_ctx,
        )?;
        bindings::wasmcloud::nats::core::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
            linker,
            crate::engine::ctx::extract_active_ctx,
        )?;
        bindings::wasmcloud::nats::jetstream::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
            linker,
            crate::engine::ctx::extract_active_ctx,
        )?;
        bindings::wasmcloud::nats::kv::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
            linker,
            crate::engine::ctx::extract_active_ctx,
        )?;

        let exports_handler = bound.iter().any(|i| {
            serves(
                i,
                &["jetstream-handler", "core-handler", "kv-handler", "handler"],
            )
        });
        if !exports_handler {
            return Ok(());
        }

        let mut jetstream_subs = Vec::new();
        let mut core_subs = Vec::new();
        let mut kv_watches = Vec::new();

        for interface in &bound {
            if serves(interface, &["handler", "jetstream-handler"])
                && let Some(raw) = interface.config.get("subscriptions")
            {
                jetstream_subs.extend(parse_jetstream_subscriptions(raw)?);
            }
            if serves(interface, &["handler", "core-handler"])
                && let Some(raw) = interface.config.get("core-subscriptions")
            {
                core_subs.extend(parse_core_subscriptions(raw)?);
            }
            if serves(interface, &["handler", "kv-handler"])
                && let Some(raw) = interface.config.get("kv-watches")
            {
                kv_watches.extend(parse_kv_watches(raw)?);
            }
        }

        let WorkloadItem::Component(component) = item else {
            anyhow::bail!("wasmcloud:nats handlers are only supported on components")
        };

        self.tracker.write().await.add_component(
            component,
            ComponentData {
                cancel_token: tokio_util::sync::CancellationToken::new(),
                jetstream_subs,
                core_subs,
                kv_watches,
            },
        );

        Ok(())
    }

    async fn on_workload_resolved(
        &self,
        workload: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        let (cancel_token, jetstream_subs, core_subs, kv_watches) = {
            let lock = self.tracker.read().await;
            match lock.get_component_data(component_id) {
                Some(data) => (
                    data.cancel_token.clone(),
                    data.jetstream_subs.clone(),
                    data.core_subs.clone(),
                    data.kv_watches.clone(),
                ),
                None => return Ok(()),
            }
        };

        if jetstream_subs.is_empty() && core_subs.is_empty() && kv_watches.is_empty() {
            return Ok(());
        }

        let workload_id = workload.id();
        let Some(handle) = self.conn_for(workload_id).await else {
            anyhow::bail!("no NATS connection bound for workload `{workload_id}`")
        };

        // A subscription outside the grant is a deployment error, not a
        // per-message denial: the workload would otherwise start and silently
        // receive nothing.
        for sub in &core_subs {
            handle
                .policy
                .check_subscription(&sub.subject)
                .map_err(|_| {
                    anyhow::anyhow!(
                        "core subscription `{}` is outside this workload's subject grant",
                        sub.subject
                    )
                })?;
        }
        for sub in &jetstream_subs {
            handle.policy.check_stream(&sub.stream).map_err(|_| {
                anyhow::anyhow!(
                    "jetstream subscription on stream `{}` is outside this workload's stream grant",
                    sub.stream
                )
            })?;
        }
        for watch in &kv_watches {
            handle.policy.check_bucket(&watch.bucket).map_err(|_| {
                anyhow::anyhow!(
                    "kv watch on bucket `{}` is outside this workload's bucket grant",
                    watch.bucket
                )
            })?;
        }

        let fuel_meter = self.meters.read().await.fuel_consumption.clone();

        if !jetstream_subs.is_empty() {
            subscriber::spawn_jetstream_subscriptions(
                workload,
                component_id,
                handle.clone(),
                jetstream_subs,
                cancel_token.clone(),
                fuel_meter.clone(),
            )
            .await?;
        }
        if !core_subs.is_empty() {
            subscriber::spawn_core_subscriptions(
                workload,
                component_id,
                handle.clone(),
                core_subs,
                cancel_token.clone(),
                fuel_meter.clone(),
            )
            .await?;
        }
        if !kv_watches.is_empty() {
            subscriber::spawn_kv_watches(
                workload,
                component_id,
                handle,
                kv_watches,
                cancel_token,
                fuel_meter,
            )
            .await?;
        }

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        let workload_cleanup = |_| async {};
        let component_cleanup = |data: ComponentData| async move {
            data.cancel_token.cancel();
        };

        self.tracker
            .write()
            .await
            .remove_workload_with_cleanup(workload_id, workload_cleanup, component_cleanup)
            .await;

        self.connections.release(workload_id).await;
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.connections.shutdown().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jetstream_subs_basic() {
        let subs = parse_jetstream_subscriptions("ORDERS:orders.*:new").unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].stream, "ORDERS");
        assert_eq!(subs[0].filter_subject, "orders.*");
        assert_eq!(subs[0].deliver_policy, "new");
        assert_eq!(subs[0].queue_group, None);
    }

    #[test]
    fn jetstream_subs_with_queue() {
        let subs =
            parse_jetstream_subscriptions("ORDERS:orders.*:new:workers,EVENTS:evt.>:all:group-a")
                .unwrap();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].queue_group.as_deref(), Some("workers"));
        assert_eq!(subs[1].stream, "EVENTS");
        assert_eq!(subs[1].queue_group.as_deref(), Some("group-a"));
    }

    #[test]
    fn jetstream_subs_reject_malformed() {
        assert!(parse_jetstream_subscriptions("ORDERS").is_err());
        assert!(parse_jetstream_subscriptions(":orders.*").is_err());
    }

    #[test]
    fn jetstream_subs_reject_unknown_deliver_policy() {
        let err = parse_jetstream_subscriptions("ORDERS:orders.*:evrything").unwrap_err();
        assert!(err.to_string().contains("unknown deliver policy"));
    }

    #[test]
    fn core_subs_basic() {
        let subs = parse_core_subscriptions("events.*,metrics.>:stats").unwrap();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].subject, "events.*");
        assert_eq!(subs[0].queue_group, None);
        assert_eq!(subs[1].subject, "metrics.>");
        assert_eq!(subs[1].queue_group.as_deref(), Some("stats"));
    }

    #[test]
    fn kv_watches_basic() {
        let watches = parse_kv_watches("config:*,secrets:prod.>").unwrap();
        assert_eq!(watches.len(), 2);
        assert_eq!(watches[0].bucket, "config");
        assert_eq!(watches[0].filter, "*");
        assert_eq!(watches[1].bucket, "secrets");
        assert_eq!(watches[1].filter, "prod.>");
    }

    #[test]
    fn kv_watches_reject_malformed() {
        assert!(parse_kv_watches("configonly").is_err());
        assert!(parse_kv_watches("config:").is_err());
    }
}

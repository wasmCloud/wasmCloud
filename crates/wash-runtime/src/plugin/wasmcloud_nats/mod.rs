use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::warn;

use crate::engine::workload::{ResolvedWorkload, WorkloadItem};
use crate::observability::Meters;
use crate::plugin::HostPlugin;
use crate::plugin::WorkloadTracker;
use crate::wit::{WitInterface, WitWorld};

pub(super) mod handles;
mod interfaces;
mod subscriber;

pub(super) mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "nats",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
        with: {
            "wasmcloud:nats/jetstream.message-handle": super::handles::MessageHandle,
            "wasmcloud:nats/jetstream.pull-consumer": super::handles::PullConsumerHandle,
            "wasmcloud:nats/kv.bucket": super::handles::BucketHandle,
        },
    });
}

pub(super) const PLUGIN_NATS_ID: &str = "wasmcloud-nats";

/// Per-component data held by the plugin for subscription lifecycle.
pub struct ComponentData {
    pub(super) jetstream_subs: Vec<JetStreamSubscriptionConfig>,
    pub(super) core_subs: Vec<CoreSubscriptionConfig>,
    pub(super) kv_watches: Vec<KvWatchConfig>,
    pub(super) cancel_token: tokio_util::sync::CancellationToken,
}

#[derive(Clone)]
pub(super) struct JetStreamSubscriptionConfig {
    pub stream: String,
    pub filter_subject: String,
    pub deliver_policy: String,
    pub queue_group: Option<String>,
}

#[derive(Clone)]
pub(super) struct CoreSubscriptionConfig {
    pub subject: String,
    pub queue_group: Option<String>,
}

#[derive(Clone)]
pub(super) struct KvWatchConfig {
    pub bucket: String,
    pub filter: String,
}

/// Parse JetStream subscriptions in format: `STREAM:filter[:policy[:queue]]` separated by commas.
fn parse_jetstream_subscriptions(raw: &str) -> Vec<JetStreamSubscriptionConfig> {
    raw.split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let parts: Vec<&str> = entry.splitn(4, ':').collect();
            if parts.len() < 2 {
                warn!("invalid jetstream subscription entry: {entry}");
                return None;
            }
            // TODO: should replace unwrap with unwrap_or
            Some(JetStreamSubscriptionConfig {
                stream: parts.first()?.to_string(),
                filter_subject: parts.get(1)?.to_string(),
                deliver_policy: parts.get(2).unwrap_or(&"new").to_string(),
                queue_group: parts.get(3).map(|s| s.to_string()),
            })
        })
        .collect()
}

/// Parse core subscriptions in format: `subject[:queue]` separated by commas.
fn parse_core_subscriptions(raw: &str) -> Vec<CoreSubscriptionConfig> {
    raw.split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let mut parts = entry.splitn(2, ':');
            let subject = parts.next()?.to_string();
            Some(CoreSubscriptionConfig {
                subject,
                queue_group: parts.next().map(|s| s.to_string()),
            })
        })
        .collect()
}

/// Parse KV watches in format: `bucket:filter` separated by commas.
fn parse_kv_watches(raw: &str) -> Vec<KvWatchConfig> {
    raw.split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let (bucket, filter) = entry.split_once(':')?;
            Some(KvWatchConfig {
                bucket: bucket.to_string(),
                filter: filter.to_string(),
            })
        })
        .collect()
}

/// `wasmcloud:nats` host plugin — NATS-native capabilities split by interface.
#[derive(Clone)]
pub struct WasmcloudNats {
    pub(super) tracker: Arc<RwLock<WorkloadTracker<(), ComponentData>>>,
    pub(super) client: Arc<async_nats::Client>,
    pub(super) jetstream: Arc<async_nats::jetstream::Context>,
    pub(super) meters: Arc<RwLock<Meters>>,
    pub(super) kv_stores: Arc<RwLock<HashMap<String, async_nats::jetstream::kv::Store>>>,
}

impl WasmcloudNats {
    pub fn new(client: Arc<async_nats::Client>) -> Self {
        let jetstream = async_nats::jetstream::new((*client).clone());
        Self {
            client,
            jetstream: Arc::new(jetstream),
            tracker: Arc::new(RwLock::new(WorkloadTracker::default())),
            meters: Default::default(),
            kv_stores: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Cache-checked open of a KV bucket. If absent, attempts to open (not create).
    pub(super) async fn get_kv(&self, bucket: &str) -> Option<async_nats::jetstream::kv::Store> {
        if let Some(store) = self.kv_stores.read().await.get(bucket) {
            return Some(store.clone());
        }
        let store = self.jetstream.get_key_value(bucket).await.ok()?;
        self.kv_stores
            .write()
            .await
            .insert(bucket.to_string(), store.clone());
        Some(store)
    }
}

#[async_trait::async_trait]
impl HostPlugin for WasmcloudNats {
    fn id(&self) -> &'static str {
        PLUGIN_NATS_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasmcloud:nats/types,core,jetstream,kv@0.2.0-draft",
            )]),
            exports: HashSet::from([WitInterface::from(
                "wasmcloud:nats/jetstream-handler,core-handler,kv-handler@0.2.0-draft",
            )]),
        }
    }

    async fn inject_meters(&self, meters: &Meters) {
        *self.meters.write().await = meters.clone();
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        let Some(interface) = interfaces
            .iter()
            .find(|i| i.namespace == "wasmcloud" && i.package == "nats")
        else {
            return Ok(());
        };

        bindings::wasmcloud::nats::types::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
            component_handle.linker(),
            crate::engine::ctx::extract_active_ctx,
        )?;
        bindings::wasmcloud::nats::core::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
            component_handle.linker(),
            crate::engine::ctx::extract_active_ctx,
        )?;
        bindings::wasmcloud::nats::jetstream::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
            component_handle.linker(),
            crate::engine::ctx::extract_active_ctx,
        )?;
        bindings::wasmcloud::nats::kv::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
            component_handle.linker(),
            crate::engine::ctx::extract_active_ctx,
        )?;

        let has_handler = interface
            .interfaces
            .iter()
            .any(|i| i == "jetstream-handler" || i == "core-handler" || i == "kv-handler");
        if has_handler {
            let jetstream_subs = interface
                .config
                .get("subscriptions")
                .map(|s| parse_jetstream_subscriptions(s))
                .unwrap_or_default();
            let core_subs = interface
                .config
                .get("core-subscriptions")
                .map(|s| parse_core_subscriptions(s))
                .unwrap_or_default();
            let kv_watches = interface
                .config
                .get("kv-watches")
                .map(|s| parse_kv_watches(s))
                .unwrap_or_default();

            let WorkloadItem::Component(component_handle) = component_handle else {
                anyhow::bail!("Service can not be tracked");
            };

            self.tracker.write().await.add_component(
                component_handle,
                ComponentData {
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    jetstream_subs,
                    core_subs,
                    kv_watches,
                },
            );
        }

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

        let fuel_meter = self.meters.read().await.fuel_consumption.clone();

        if !jetstream_subs.is_empty() {
            subscriber::spawn_jetstream_subscriptions(
                workload,
                component_id,
                self.jetstream.clone(),
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
                self.client.clone(),
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
                self.jetstream.clone(),
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
        _interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        let workload_cleanup = |_| async {};
        let component_cleanup = |component_data: ComponentData| async move {
            component_data.cancel_token.cancel();
        };

        self.tracker
            .write()
            .await
            .remove_workload_with_cleanup(workload_id, workload_cleanup, component_cleanup)
            .await;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jetstream_subs_basic() {
        let subs = parse_jetstream_subscriptions("ORDERS:orders.*:new");
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].stream, "ORDERS");
        assert_eq!(subs[0].filter_subject, "orders.*");
        assert_eq!(subs[0].deliver_policy, "new");
        assert_eq!(subs[0].queue_group, None);
    }

    #[test]
    fn jetstream_subs_with_queue() {
        let subs =
            parse_jetstream_subscriptions("ORDERS:orders.*:new:workers,EVENTS:evt.>:all:group-a");
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].queue_group.as_deref(), Some("workers"));
        assert_eq!(subs[1].stream, "EVENTS");
        assert_eq!(subs[1].queue_group.as_deref(), Some("group-a"));
    }

    #[test]
    fn core_subs_basic() {
        let subs = parse_core_subscriptions("events.*,metrics.>:stats");
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].subject, "events.*");
        assert_eq!(subs[0].queue_group, None);
        assert_eq!(subs[1].subject, "metrics.>");
        assert_eq!(subs[1].queue_group.as_deref(), Some("stats"));
    }

    #[test]
    fn kv_watches_basic() {
        let watches = parse_kv_watches("config:*,secrets:prod.>");
        assert_eq!(watches.len(), 2);
        assert_eq!(watches[0].bucket, "config");
        assert_eq!(watches[0].filter, "*");
        assert_eq!(watches[1].bucket, "secrets");
        assert_eq!(watches[1].filter, "prod.>");
    }
}

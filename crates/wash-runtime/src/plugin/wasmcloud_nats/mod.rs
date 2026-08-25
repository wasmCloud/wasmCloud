use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use anyhow::Context as _;
use tokio::sync::RwLock;
use tracing::info;

use crate::engine::workload::{ResolvedWorkload, UnresolvedWorkload, WorkloadItem};
use crate::observability::Meters;
use crate::plugin::{HostPlugin, WitInterfaces, WorkloadFailureSink, WorkloadTracker};
use crate::wit::{WitInterface, WitWorld};

mod async_p3;
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

/// Handler worlds for the async `@0.2.0` package. Same split as the sync
/// worlds above, and for the same reason.
pub(super) mod async_jetstream_bindings {
    crate::wasmtime::component::bindgen!({
        world: "nats-async-js-processor",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
        with: {
            "wasmcloud:nats/jetstream@0.2.0.message-handle": super::handles::MessageHandle,
            "wasmcloud:nats/jetstream@0.2.0.pull-consumer": super::handles::PullConsumerHandle,
        },
    });
}

pub(super) mod async_core_bindings {
    crate::wasmtime::component::bindgen!({
        world: "nats-async-subscriber",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
    });
}

pub(super) mod async_kv_bindings {
    crate::wasmtime::component::bindgen!({
        world: "nats-async-kv-watcher",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
        with: {
            "wasmcloud:nats/kv@0.2.0.bucket": super::handles::BucketHandle,
        },
    });
}

pub(super) const PLUGIN_NATS_ID: &str = "wasmcloud-nats";

const NATS_VERSION: &str = "0.1.0";
/// The async (WASI P3) revision. A P3 component cannot bind `@0.1.0`: lifting
/// a sync-signature function with the async canonical ABI fails validation.
const NATS_ASYNC_VERSION: &str = "0.2.0";

/// True when a bound interface asks for the async package.
fn is_async(interface: &WitInterface) -> bool {
    interface
        .version
        .as_ref()
        .is_some_and(|v| (v.major, v.minor) >= (0, 2))
}

/// Per-component subscription state.
pub struct ComponentData {
    pub(super) jetstream_subs: Vec<JetStreamSubscriptionConfig>,
    pub(super) core_subs: Vec<CoreSubscriptionConfig>,
    pub(super) kv_watches: Vec<KvWatchConfig>,
    /// The subscriptions this component picked up from entries that named no
    /// component, in the canonical form [`subscription_spec`] renders. Kept so
    /// the next component of the same workload can tell whether it is about to
    /// pick up the same ones — see [`WasmcloudNats::on_workload_item_bind`].
    pub(super) untargeted_specs: Vec<String>,
    pub(super) cancel_token: tokio_util::sync::CancellationToken,
}

#[derive(Clone, Debug)]
pub(super) struct JetStreamSubscriptionConfig {
    /// The binding this subscription was declared on, and whose connection and
    /// grant it runs under. Empty for a plain, unlabeled binding.
    pub binding: String,
    pub stream: String,
    pub filter_subject: String,
    pub deliver_policy: String,
    pub queue_group: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct CoreSubscriptionConfig {
    pub binding: String,
    pub subject: String,
    pub queue_group: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct KvWatchConfig {
    pub binding: String,
    pub bucket: String,
    pub filter: String,
}

/// Refuses a queue group no NATS server would accept.
///
/// Nothing downstream can recover from an illegal group: the core client
/// rejects the subscribe outright, and JetStream fails the create-consumer call
/// on every attempt, which the delivery loop can only keep retrying while the
/// workload reports running and receives nothing. The deploy is the last moment
/// the author is still watching, so the group is checked here.
fn validate_queue_group(entry: &str, group: &str, jetstream: bool) -> anyhow::Result<()> {
    if group.is_empty() {
        anyhow::bail!(
            "subscription `{entry}` has an empty queue group; drop the trailing `:` to \
             subscribe without one"
        )
    }
    if group.chars().any(|c| c.is_ascii_whitespace()) {
        anyhow::bail!("subscription `{entry}` has a queue group containing whitespace")
    }
    // A JetStream group is carried into a durable name and into one token of
    // the push deliver subject, neither of which admits subject syntax or a
    // path separator.
    if jetstream
        && let Some(bad) = group
            .chars()
            .find(|c| matches!(c, '.' | '*' | '>' | '/' | '\\'))
    {
        anyhow::bail!(
            "jetstream subscription `{entry}` has a queue group containing `{bad}`; the group \
             names a durable consumer, so it cannot contain `.`, `*`, `>`, `/` or `\\`"
        )
    }
    Ok(())
}

/// Parses `STREAM:filter[:policy[:queue]]`, comma separated.
fn parse_jetstream_subscriptions(
    binding: &str,
    raw: &str,
) -> anyhow::Result<Vec<JetStreamSubscriptionConfig>> {
    let subs: Vec<JetStreamSubscriptionConfig> = raw
        .split(',')
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
            let queue_group = parts.get(3).copied();
            if let Some(group) = queue_group {
                validate_queue_group(entry, group, true)?;
            }
            Ok(JetStreamSubscriptionConfig {
                binding: binding.to_string(),
                stream: (*stream).to_string(),
                filter_subject: (*filter_subject).to_string(),
                deliver_policy: deliver_policy.to_string(),
                queue_group: queue_group.map(str::to_string),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    // A queue group on a stream names one shared consumer, and the filter and
    // deliver policy belong to that consumer rather than to the entry that
    // declared it. Two entries that disagree about either can never both get
    // the consumer they asked for — whichever is created second either fails or
    // rewrites the first — so the pair is refused rather than left to a
    // retry loop under a workload that reports running.
    for (position, sub) in subs.iter().enumerate() {
        let Some(group) = sub.queue_group.as_deref() else {
            continue;
        };
        for earlier in subs.iter().take(position) {
            if earlier.stream != sub.stream || earlier.queue_group.as_deref() != Some(group) {
                continue;
            }
            if earlier.filter_subject != sub.filter_subject
                || earlier.deliver_policy != sub.deliver_policy
            {
                anyhow::bail!(
                    "jetstream subscriptions on stream `{}` share the queue group `{group}` but \
                     ask for different deliveries (`{}:{}` and `{}:{}`); one group is one \
                     consumer, so give each delivery its own group",
                    sub.stream,
                    earlier.filter_subject,
                    earlier.deliver_policy,
                    sub.filter_subject,
                    sub.deliver_policy
                )
            }
        }
    }

    Ok(subs)
}

/// Parses `subject[:queue]`, comma separated.
fn parse_core_subscriptions(
    binding: &str,
    raw: &str,
) -> anyhow::Result<Vec<CoreSubscriptionConfig>> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let mut parts = entry.splitn(2, ':');
            let subject = parts.next().unwrap_or_default();
            if subject.is_empty() {
                anyhow::bail!("core subscription `{entry}` has an empty subject")
            }
            let queue_group = parts.next();
            if let Some(group) = queue_group {
                validate_queue_group(entry, group, false)?;
            }
            Ok(CoreSubscriptionConfig {
                binding: binding.to_string(),
                subject: subject.to_string(),
                queue_group: queue_group.map(str::to_string),
            })
        })
        .collect()
}

/// Parses `bucket:filter`, comma separated.
fn parse_kv_watches(binding: &str, raw: &str) -> anyhow::Result<Vec<KvWatchConfig>> {
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
                binding: binding.to_string(),
                bucket: bucket.to_string(),
                filter: filter.to_string(),
            })
        })
        .collect()
}

/// One declared subscription as the author wrote it, used both to recognise the
/// same subscription arriving on a second component and to name it in the error
/// that refuses the pair.
fn subscription_spec(key: &str, binding: &str, value: String) -> String {
    if binding.is_empty() {
        format!("`{key}: {value}`")
    } else {
        format!("`{key}: {value}` on binding `{binding}`")
    }
}

/// `wasmcloud:nats` host plugin — NATS-native capabilities split by interface.
pub struct WasmcloudNats {
    pub(super) tracker: Arc<RwLock<WorkloadTracker<(), ComponentData>>>,
    pub(super) connections: Arc<ConnectionRegistry>,
    pub(super) meters: Arc<RwLock<Meters>>,
    /// Subjects the host itself uses, denied to every workload.
    lattice_prefixes: Vec<String>,
    /// The host's data-plane servers, used by any binding that names none of
    /// its own. Address only: credentials and grants stay per workload, so a
    /// workload that inherits the address still reaches nothing until it is
    /// granted something.
    default_servers: Vec<String>,
    /// How a subscriber loop tells the host its workload has died out of band —
    /// a server-side permission denial parks a subscription that deployed
    /// cleanly, and nothing else would ever move the workload off running.
    failure_sink: arc_swap::ArcSwapOption<WorkloadFailureSink>,
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
            default_servers: Vec::new(),
            failure_sink: arc_swap::ArcSwapOption::empty(),
        }
    }

    /// Denies the host's own lattice subject space to every workload.
    pub fn with_lattice_prefixes(mut self, prefixes: Vec<String>) -> Self {
        self.lattice_prefixes = prefixes;
        self
    }

    /// Sets the servers a binding falls back to when it names none.
    ///
    /// This is the host's own data-plane NATS (`--data-nats-url`, `dataNatsUrl`
    /// in the chart), so the common case — a workload on the cluster's NATS —
    /// needs no `servers` in its manifest. A binding that sets `servers`
    /// overrides this outright rather than merging with it: a workload pointed
    /// at a different cluster means a different cluster, not both.
    ///
    /// Deliberately address-only. Inheriting the host's credentials or grants
    /// would make every workload's reach depend on how the host was launched,
    /// which is the opposite of deny-by-default.
    pub fn with_default_servers(mut self, servers: Vec<String>) -> Self {
        self.default_servers = servers;
        self
    }

    /// Resolves the connection a plain, unlabeled call goes out on.
    pub(super) async fn conn_for(&self, workload_id: &str) -> Option<Arc<ConnHandle>> {
        self.connections.get(workload_id).await
    }

    /// Opens one connection per binding name.
    ///
    /// Split out of the bind hook so a failure can be told apart from one that
    /// never reached a live socket: `opened` is set as soon as a connection is
    /// registered, which is what the caller needs to know before it decides
    /// whether there is anything to hand back.
    async fn open_bindings(
        &self,
        workload_id: &str,
        bindings: Vec<(&str, HashMap<String, String>)>,
        opened: &mut bool,
    ) -> anyhow::Result<()> {
        for (binding, mut merged) in bindings {
            // The host's data plane backs any binding that names no servers of
            // its own. Injected before parsing rather than after, so the
            // resolved address is what `connection_key` identifies the
            // connection by — two bindings that both fall back to the host must
            // read as the same connection, not as two.
            if !merged.contains_key("servers") && !self.default_servers.is_empty() {
                merged.insert("servers".to_string(), self.default_servers.join(","));
            }

            // A binding is described by the manifest as a whole, and only the
            // entries a component actually matched are folded into `merged`. So
            // the connection settings going missing while other keys survive
            // means they were written on an entry nothing matched — most often
            // an entry naming an *imported* interface, in a workload whose only
            // component receives. Such a component exports the handler and
            // never calls out, so it does not cover that entry, and the entry
            // is dropped along with the servers on it. The bare "requires
            // `servers`" this would otherwise fail with sends authors looking
            // at config they can see is present.
            if !merged.contains_key("servers") && !merged.is_empty() {
                // Sorted: the same manifest has to be refused with the same
                // message every time, and `merged` is a `HashMap`.
                let mut present: Vec<String> =
                    merged.keys().map(|key| format!("`{key}`")).collect();
                present.sort();
                anyhow::bail!(
                    "wasmcloud:nats binding `{}` of workload `{workload_id}` has no `servers`, \
                     but does set {}. Connection settings only reach the host from an entry one \
                     of the workload's components matches: a component that exports a handler \
                     and imports nothing does not match an entry naming an imported interface. \
                     Move `servers` (and the credentials and grants beside it) onto the entry \
                     that names the handler this workload exports",
                    describe_binding(binding),
                    present.join(", ")
                )
            }

            let mut config = NatsConfig::from_map(&merged).with_context(|| {
                format!(
                    "invalid wasmcloud:nats configuration for workload `{workload_id}` \
                     binding `{}`",
                    describe_binding(binding)
                )
            })?;

            // Scope request-reply to this workload so two workloads on one
            // server cannot observe each other's responses. Named bindings get
            // one prefix each: two connections sharing an inbox prefix would
            // race for each other's replies.
            if config.inbox_prefix.is_none() {
                config.inbox_prefix = Some(conn::binding_inbox_prefix(workload_id, binding));
            }

            // One connection per binding *name*, and the entries of a name are
            // already folded into the configuration above, so what is left to
            // catch here is a workload rebinding a name it still holds a
            // connection for under a different configuration. That connection
            // is the one live calls already route to, and rebinding cannot
            // quietly move them onto a new grant.
            if self
                .connections
                .has_conflicting(workload_id, binding, &config.connection_key())
                .await
            {
                anyhow::bail!(
                    "workload `{workload_id}` already holds a wasmcloud:nats connection under \
                     `{}` with a different configuration; a capability call carries only its \
                     binding name, so it cannot be attributed to one of them. Give each \
                     configuration its own `(implements ..)` name, or redeploy the workload",
                    describe_binding(binding)
                )
            }

            let handle = self
                .connections
                .acquire(workload_id, binding, &config, self.lattice_prefixes.clone())
                .await?;
            *opened = true;

            info!(
                workload_id,
                binding = describe_binding(binding),
                servers = config.servers.join(","),
                auth = config.auth.kind(),
                server_version = handle.server_version(),
                "opened NATS connection"
            );
        }

        Ok(())
    }

    /// Starts every subscription declared on one binding, on that binding's
    /// connection.
    ///
    /// Grants are per binding too, so each set is checked against the policy of
    /// the connection it will actually run on.
    #[allow(clippy::too_many_arguments)]
    async fn spawn_binding_subscriptions(
        &self,
        workload: &ResolvedWorkload,
        component_id: &str,
        binding: &str,
        jetstream_subs: Vec<JetStreamSubscriptionConfig>,
        core_subs: Vec<CoreSubscriptionConfig>,
        kv_watches: Vec<KvWatchConfig>,
        cancel_token: tokio_util::sync::CancellationToken,
        fuel_meter: crate::observability::FuelConsumptionMeter,
    ) -> anyhow::Result<()> {
        let workload_id = workload.id();
        let Some(handle) = self.connections.get_named(workload_id, binding).await else {
            anyhow::bail!(
                "no NATS connection bound for workload `{workload_id}` binding `{}`",
                describe_binding(binding)
            )
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
            // A stream grant alone would deliver every subject that stream
            // captures, so the filter has to sit inside the subject grant too.
            handle
                .policy
                .check_filter(&sub.filter_subject)
                .map_err(|_| {
                    anyhow::anyhow!(
                        "jetstream subscription filter `{}` on stream `{}` is outside this \
                         workload's subject grant; add it to `subject-allow`",
                        sub.filter_subject,
                        sub.stream
                    )
                })?;
        }
        // A KV watch filter matches keys within a bucket, not NATS subjects,
        // so `bucket-allow` is the grant that governs it.
        for watch in &kv_watches {
            handle.policy.check_bucket(&watch.bucket).map_err(|_| {
                anyhow::anyhow!(
                    "kv watch on bucket `{}` is outside this workload's bucket grant",
                    watch.bucket
                )
            })?;
        }

        // A loop that discovers its subscription can never deliver has no way
        // back to the deployment that started it, so it carries the sink and
        // reports the workload failed from where the discovery happens.
        let failure_sink = self
            .failure_sink
            .load_full()
            .map(|sink| sink.as_ref().clone());

        if !jetstream_subs.is_empty() {
            subscriber::spawn_jetstream_subscriptions(
                workload,
                component_id,
                handle.clone(),
                jetstream_subs,
                cancel_token.clone(),
                fuel_meter.clone(),
                failure_sink.clone(),
                workload_id,
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
                failure_sink.clone(),
                workload_id,
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
                failure_sink,
                workload_id,
            )
            .await?;
        }

        Ok(())
    }
}

/// Collects this plugin's interfaces out of a bound set.
fn nats_interfaces<'a>(interfaces: &'a WitInterfaces<'_>) -> Vec<&'a WitInterface> {
    interfaces
        .iter()
        .filter(|i| i.namespace == "wasmcloud" && i.package == "nats")
        .collect()
}

/// The binding name an interface routes under: its `(implements ..)` label, or
/// the empty string when it is bound plainly.
fn binding_name(interface: &WitInterface) -> &str {
    interface.name.as_deref().unwrap_or(conn::UNNAMED_BINDING)
}

/// Refuses a labeled binding on the sync `@0.1.0` package.
///
/// Label routing is implemented for the async `@0.2.0` interfaces only, and a
/// sync-P2 component cannot carry labeled imports anyway: `wit-bindgen` fails
/// componentization on that shape. Refusing at bind gives that a name, rather
/// than deploying a workload whose component then fails to instantiate against
/// imports nothing linked.
fn labeled_revision(interface: &WitInterface) -> anyhow::Result<()> {
    if interface.name.is_some() && !is_async(interface) {
        anyhow::bail!(
            "wasmcloud:nats interface `{}` is bound under the name `{}`, but named bindings \
             are served only by the async `@{NATS_ASYNC_VERSION}` package; bind it as \
             `@{NATS_ASYNC_VERSION}`, or drop the name to use the single unnamed binding",
            interface.instance(),
            interface.name.as_deref().unwrap_or_default()
        )
    }
    Ok(())
}

/// A binding name for a log line or an error message.
fn describe_binding(binding: &str) -> &str {
    if binding.is_empty() {
        "<unnamed>"
    } else {
        binding
    }
}

/// Folds every bound entry into one configuration per binding name.
///
/// A binding is described by the manifest as a whole rather than by any one
/// entry: the servers, the credentials and the grants belong on the entry that
/// imports `wasmcloud:nats`, while `subscriptions` belong on the entry that
/// exports a handler. Reading each entry as a complete connection spec would
/// refuse that split and leave authors copying the connection into every entry,
/// where a later edit to one copy quietly gives one binding two grants.
///
/// Entries arrive out of a `HashSet`, so they are ordered before they are
/// folded: a manifest that cannot be deployed has to be refused with the same
/// key named every time, not with whichever entry happened to be visited first.
fn merge_binding_configs<'a>(
    bound: &[&'a WitInterface],
) -> anyhow::Result<Vec<(&'a str, HashMap<String, String>)>> {
    let mut ordered = bound.to_vec();
    ordered.sort_by_cached_key(|interface| {
        let mut keys: Vec<String> = interface.config.keys().cloned().collect();
        keys.sort();
        (interface.instance(), keys)
    });

    let mut merged: BTreeMap<&str, HashMap<String, String>> = BTreeMap::new();
    for interface in ordered {
        labeled_revision(interface)?;
        let binding = binding_name(interface);
        let mut pairs: Vec<(&String, &String)> = interface.config.iter().collect();
        pairs.sort_by_key(|(key, _)| *key);

        let config = merged.entry(binding).or_default();
        for (key, value) in pairs {
            let canonical = config::canonical_key(key);
            // Never first-wins. The entries of one binding open one connection
            // checked against one grant, so a key two of them disagree about
            // has no answer that can be picked here, and picking one would let
            // a stale copy of a grant outlive the edit that narrowed it. The
            // key is named and the values are not: one of them may be a
            // credential.
            if let Some(existing) = config.get(&canonical)
                && existing != value
            {
                anyhow::bail!(
                    "conflicting values for `{canonical}` across the wasmcloud:nats entries of \
                     binding `{}`; the entries of one binding are folded into a single \
                     connection configuration, so a key more than one of them sets must agree \
                     (kebab-case and snake_case spellings are the same key)",
                    describe_binding(binding)
                )
            }
            config.insert(canonical, value.clone());
        }
    }

    Ok(merged.into_iter().collect())
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
        const IMPORTS: &str = "wasmcloud:nats/types,core,jetstream,kv";
        const EXPORTS: &str = "wasmcloud:nats/jetstream-handler,core-handler,kv-handler";
        WitWorld {
            imports: HashSet::from([
                WitInterface::from(format!("{IMPORTS}@{NATS_VERSION}").as_str()),
                WitInterface::from(format!("{IMPORTS}@{NATS_ASYNC_VERSION}").as_str()),
            ]),
            exports: HashSet::from([
                WitInterface::from(format!("{EXPORTS}@{NATS_VERSION}").as_str()),
                WitInterface::from(format!("{EXPORTS}@{NATS_ASYNC_VERSION}").as_str()),
            ]),
        }
    }

    /// Named (`(implements ..)`) bindings route per label: one connection, one
    /// grant, and one set of subscriptions each, so a component can bridge two
    /// clusters by importing `wasmcloud:nats` twice under different names.
    ///
    /// Only the async `@0.2.0` package is routable — see [`labeled_revision`].
    fn supports_named_instances(&self) -> bool {
        true
    }

    async fn inject_meters(&self, meters: &Meters) {
        *self.meters.write().await = meters.clone();
    }

    /// A subscription can stop delivering long after it deployed — most often
    /// when the workload's NATS credentials are narrower server-side than the
    /// grants the host checked at bind — and only the subscriber loop is in a
    /// position to notice.
    fn set_workload_failure_sink(&self, sink: WorkloadFailureSink) {
        self.failure_sink.store(Some(Arc::new(sink)));
    }

    /// Validates config, resolves credentials, and opens one connection per
    /// binding name. Failing here fails the deployment rather than the first
    /// call.
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
        let bindings = merge_binding_configs(&bound)?;

        // A bind that fails partway has already opened every connection ahead
        // of the failure, and the workload it belongs to will never resolve.
        // The engine unbinds the plugin that failed, but the sockets are this
        // plugin's to close: left registered, each keeps a client and its event
        // task alive until the host restarts, and the next deploy under the
        // same workload id is then refused for conflicting with a connection
        // nothing is using.
        let mut opened = false;
        let result = self.open_bindings(workload_id, bindings, &mut opened).await;
        if result.is_err() && opened {
            self.connections.release(workload_id).await;
        }
        result
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

        // Register the revisions this workload bound. A binding that names no
        // version gets both, so an unversioned manifest entry works whichever
        // revision the component was built against.
        let (binds_sync, binds_async) =
            bound.iter().fold((false, false), |(sync, async_), i| {
                match (i.version.as_ref(), is_async(i)) {
                    (None, _) => (true, true),
                    (Some(_), true) => (sync, true),
                    (Some(_), false) => (true, async_),
                }
            });

        // Labeled (`(implements ..)`) imports route per binding name; a plain
        // import routes to the workload's unnamed binding. A component can do
        // either, and a workload can hold both, so bind only what is asked for:
        // linking the labeled instances into a component that imports plainly
        // (or the reverse) leaves an unsatisfied import.
        let (has_labeled, has_plain) =
            bound
                .iter()
                .fold((false, false), |(labeled, plain), i| match i.name {
                    Some(_) => (true, plain),
                    None => (labeled, true),
                });
        let bindings = self.connections.bindings_for(item.workload_id()).await;
        // Cheap (`Arc`-backed) clone so the immutable borrow ends before the
        // linker's mutable one.
        let component = item.component().clone();

        let linker = item.linker();
        if binds_sync {
            bindings::wasmcloud::nats0_1_0::types::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                linker,
                crate::engine::ctx::extract_active_ctx,
            )?;
            bindings::wasmcloud::nats0_1_0::core::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                linker,
                crate::engine::ctx::extract_active_ctx,
            )?;
            bindings::wasmcloud::nats0_1_0::jetstream::add_to_linker::<
                _,
                crate::engine::ctx::SharedCtx,
            >(linker, crate::engine::ctx::extract_active_ctx)?;
            bindings::wasmcloud::nats0_1_0::kv::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                linker,
                crate::engine::ctx::extract_active_ctx,
            )?;
        }
        if binds_async {
            use async_p3::bindings::wasmcloud::nats0_2_0 as async_nats_wit;
            // `types` carries record and error definitions only: nothing to
            // route, so it is bound once either way.
            async_nats_wit::types::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                linker,
                crate::engine::ctx::extract_active_ctx,
            )?;
            if has_plain {
                async_nats_wit::core::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                    linker,
                    crate::engine::ctx::extract_active_ctx,
                )?;
                async_nats_wit::jetstream::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                    linker,
                    crate::engine::ctx::extract_active_ctx,
                )?;
                async_nats_wit::kv::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                    linker,
                    crate::engine::ctx::extract_active_ctx,
                )?;
            }
            if has_labeled {
                use async_p3::bindings::named_imports::wasmcloud::nats0_2_0 as labeled_nats_wit;
                let route = |label: &str| -> wasmtime::Result<async_p3::NatsId> {
                    bindings.get(label).cloned().ok_or_else(|| {
                        wasmtime::format_err!(
                            "component imports wasmcloud:nats as `{label}`, but the workload \
                             binds no wasmcloud:nats interface under that name"
                        )
                    })
                };
                labeled_nats_wit::core::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                    linker,
                    &component,
                    route,
                    crate::engine::ctx::extract_active_ctx,
                )?;
                labeled_nats_wit::jetstream::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                    linker,
                    &component,
                    route,
                    crate::engine::ctx::extract_active_ctx,
                )?;
                labeled_nats_wit::kv::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                    linker,
                    &component,
                    route,
                    crate::engine::ctx::extract_active_ctx,
                )?;
            }
        }

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
        let mut untargeted_specs: Vec<String> = Vec::new();

        let component_id = item.id().to_string();
        let workload_id = item.workload_id().to_string();

        for interface in &bound {
            let binding = binding_name(interface);
            // The runtime hands an unnamed host-interface entry to every
            // component whose world covers it, which for a subscription means
            // every handler in the workload receives every message. `component`
            // is how an author says which handler a subscription was written
            // for; entries that name another component are simply not this
            // component's.
            let targeted = match interface.config.get("component") {
                Some(target) if target.as_str() != component_id => continue,
                Some(_) => true,
                None => false,
            };
            if serves(interface, &["handler", "jetstream-handler"])
                && let Some(raw) = interface.config.get("subscriptions")
            {
                let subs = parse_jetstream_subscriptions(binding, raw)?;
                if !targeted {
                    untargeted_specs.extend(subs.iter().map(|sub| {
                        let value = match &sub.queue_group {
                            Some(group) => {
                                format!("{}:{}:{group}", sub.stream, sub.filter_subject)
                            }
                            None => format!("{}:{}", sub.stream, sub.filter_subject),
                        };
                        subscription_spec("subscriptions", binding, value)
                    }));
                }
                jetstream_subs.extend(subs);
            }
            if serves(interface, &["handler", "core-handler"])
                && let Some(raw) = interface.config.get("core-subscriptions")
            {
                let subs = parse_core_subscriptions(binding, raw)?;
                if !targeted {
                    untargeted_specs.extend(subs.iter().map(|sub| {
                        let value = match &sub.queue_group {
                            Some(group) => format!("{}:{group}", sub.subject),
                            None => sub.subject.clone(),
                        };
                        subscription_spec("core-subscriptions", binding, value)
                    }));
                }
                core_subs.extend(subs);
            }
            if serves(interface, &["handler", "kv-handler"])
                && let Some(raw) = interface.config.get("kv-watches")
            {
                let watches = parse_kv_watches(binding, raw)?;
                if !targeted {
                    untargeted_specs.extend(watches.iter().map(|watch| {
                        subscription_spec(
                            "kv-watches",
                            binding,
                            format!("{}:{}", watch.bucket, watch.filter),
                        )
                    }));
                }
                kv_watches.extend(watches);
            }
        }

        let WorkloadItem::Component(component) = item else {
            anyhow::bail!("wasmcloud:nats handlers are only supported on components")
        };

        let mut tracker = self.tracker.write().await;

        // Every component of a workload binds in turn, so the second one to
        // pick up a subscription no entry claimed for it finds the first
        // already holding it. Delivering the same subscription to two handlers
        // is never what was meant — each message would be processed twice, and
        // each handler would see traffic written for the other — and there is
        // no way to tell from here which one it belongs to, so the deployment
        // is refused with the fix in hand rather than started wrong.
        if let Some(tracked) = tracker.workloads.get(&workload_id) {
            for (other_id, other) in &tracked.components {
                if other_id == &component_id {
                    continue;
                }
                if let Some(spec) = untargeted_specs
                    .iter()
                    .find(|spec| other.untargeted_specs.contains(spec))
                {
                    anyhow::bail!(
                        "workload `{workload_id}` declares {spec} without naming a component, so \
                         it attaches to both `{other_id}` and `{component_id}` and every message \
                         would be handled twice. Add `component: <id>` to each \
                         subscription-bearing wasmcloud:nats entry"
                    )
                }
            }
        }

        tracker.add_component(
            component,
            ComponentData {
                cancel_token: tokio_util::sync::CancellationToken::new(),
                jetstream_subs,
                core_subs,
                kv_watches,
                untargeted_specs,
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

        let fuel_meter = self.meters.read().await.fuel_consumption.clone();

        // Subscriptions run on the binding they were declared on: two named
        // bindings mean two connections, two grants, and two sets of loops
        // dispatching into the same component export.
        let mut bindings: Vec<String> = jetstream_subs
            .iter()
            .map(|s| s.binding.clone())
            .chain(core_subs.iter().map(|s| s.binding.clone()))
            .chain(kv_watches.iter().map(|w| w.binding.clone()))
            .collect();
        bindings.sort_unstable();
        bindings.dedup();

        for binding in bindings {
            let jetstream_subs: Vec<_> = jetstream_subs
                .iter()
                .filter(|s| s.binding == binding)
                .cloned()
                .collect();
            let core_subs: Vec<_> = core_subs
                .iter()
                .filter(|s| s.binding == binding)
                .cloned()
                .collect();
            let kv_watches: Vec<_> = kv_watches
                .iter()
                .filter(|w| w.binding == binding)
                .cloned()
                .collect();
            self.spawn_binding_subscriptions(
                workload,
                component_id,
                &binding,
                jetstream_subs,
                core_subs,
                kv_watches,
                cancel_token.clone(),
                fuel_meter.clone(),
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
        // Cancel before draining. A subscriber loop cannot tell a drained
        // client from an outage, so one left running past shutdown retries
        // against a connection that will never come back — warning on every
        // pass and holding its connection and workload graph alive for as long
        // as the process does. Cancelling first sends each loop out through its
        // own cancel arm, and reaches in-flight handlers while their acks can
        // still land; the drain budget then bounds the settle window.
        {
            let mut tracker = self.tracker.write().await;
            for workload in tracker.workloads.values() {
                for data in workload.components.values() {
                    data.cancel_token.cancel();
                }
            }
            tracker.workloads.clear();
            tracker.components.clear();
        }

        self.connections.shutdown().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jetstream_subs_basic() {
        let subs = parse_jetstream_subscriptions("", "ORDERS:orders.*:new").unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].stream, "ORDERS");
        assert_eq!(subs[0].filter_subject, "orders.*");
        assert_eq!(subs[0].deliver_policy, "new");
        assert_eq!(subs[0].queue_group, None);
    }

    #[test]
    fn jetstream_subs_with_queue() {
        let subs = parse_jetstream_subscriptions(
            "",
            "ORDERS:orders.*:new:workers,EVENTS:evt.>:all:group-a",
        )
        .unwrap();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].queue_group.as_deref(), Some("workers"));
        assert_eq!(subs[1].stream, "EVENTS");
        assert_eq!(subs[1].queue_group.as_deref(), Some("group-a"));
    }

    #[test]
    fn jetstream_subs_reject_illegal_queue_groups() {
        for entry in [
            "S:f:new:team.a",
            "S:f:new:has space",
            "S:f:new:a>b",
            "S:f:new:a/b",
        ] {
            let err = parse_jetstream_subscriptions("", entry)
                .unwrap_err()
                .to_string();
            assert!(err.contains("queue group"), "{entry}: {err}");
        }
    }

    #[test]
    fn subs_reject_trailing_colon_queue_group() {
        let err = parse_jetstream_subscriptions("", "STREAM:filter:new:")
            .unwrap_err()
            .to_string();
        assert!(err.contains("empty queue group"), "{err}");

        let err = parse_core_subscriptions("", "subject:")
            .unwrap_err()
            .to_string();
        assert!(err.contains("empty queue group"), "{err}");

        assert!(parse_core_subscriptions("", "subject:workers").is_ok());
    }

    #[test]
    fn core_subs_reject_queue_group_with_whitespace() {
        let err = parse_core_subscriptions("", "subj:bad group")
            .unwrap_err()
            .to_string();
        assert!(err.contains("whitespace"), "{err}");
    }

    #[test]
    fn jetstream_subs_reject_conflicting_queue_group() {
        let err = parse_jetstream_subscriptions(
            "",
            "ORDERS:orders.us.*:new:workers,ORDERS:orders.eu.*:new:workers",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("share the queue group `workers`"), "{err}");

        let err = parse_jetstream_subscriptions(
            "",
            "ORDERS:orders.*:new:workers,ORDERS:orders.*:all:workers",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("share the queue group `workers`"), "{err}");

        // The same delivery declared twice is a duplicate, not a conflict, and
        // a group on another stream is a different consumer entirely.
        assert!(
            parse_jetstream_subscriptions(
                "",
                "ORDERS:orders.*:new:workers,ORDERS:orders.*:new:workers,EVENTS:evt.>:new:workers"
            )
            .is_ok()
        );
    }

    #[test]
    fn jetstream_subs_reject_malformed() {
        assert!(parse_jetstream_subscriptions("", "ORDERS").is_err());
        assert!(parse_jetstream_subscriptions("", ":orders.*").is_err());
    }

    #[test]
    fn jetstream_subs_reject_unknown_deliver_policy() {
        let err = parse_jetstream_subscriptions("", "ORDERS:orders.*:evrything").unwrap_err();
        assert!(err.to_string().contains("unknown deliver policy"));
    }

    #[test]
    fn core_subs_basic() {
        let subs = parse_core_subscriptions("", "events.*,metrics.>:stats").unwrap();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].subject, "events.*");
        assert_eq!(subs[0].queue_group, None);
        assert_eq!(subs[1].subject, "metrics.>");
        assert_eq!(subs[1].queue_group.as_deref(), Some("stats"));
    }

    #[test]
    fn kv_watches_basic() {
        let watches = parse_kv_watches("", "config:*,secrets:prod.>").unwrap();
        assert_eq!(watches.len(), 2);
        assert_eq!(watches[0].bucket, "config");
        assert_eq!(watches[0].filter, "*");
        assert_eq!(watches[1].bucket, "secrets");
        assert_eq!(watches[1].filter, "prod.>");
    }

    #[test]
    fn world_advertises_both_revisions() {
        let world = WasmcloudNats::new().world();
        let versions = |set: &HashSet<WitInterface>| {
            let mut v: Vec<String> = set
                .iter()
                .map(|i| {
                    i.version
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default()
                })
                .collect();
            v.sort();
            v
        };
        assert_eq!(versions(&world.imports), vec!["0.1.0", "0.2.0"]);
        assert_eq!(versions(&world.exports), vec!["0.1.0", "0.2.0"]);
        for iface in world.exports.iter() {
            assert!(
                iface.interfaces.contains("kv-handler"),
                "export entry lost an interface: {:?}",
                iface.interfaces
            );
        }
    }

    #[test]
    fn kv_watches_reject_malformed() {
        assert!(parse_kv_watches("", "configonly").is_err());
        assert!(parse_kv_watches("", "config:").is_err());
    }

    fn entry(spec: &str, name: Option<&str>, config: &[(&str, &str)]) -> WitInterface {
        let mut interface = WitInterface::from(spec);
        interface.name = name.map(str::to_string);
        interface.config = config
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        interface
    }

    fn merge(entries: &[WitInterface]) -> anyhow::Result<Vec<(&str, HashMap<String, String>)>> {
        merge_binding_configs(&entries.iter().collect::<Vec<_>>())
    }

    #[test]
    fn split_entries_fold_into_one_binding() {
        let entries = [
            entry(
                "wasmcloud:nats/types,core,jetstream@0.2.0",
                None,
                &[
                    ("servers", "nats://localhost:4222"),
                    ("subject-allow", "orders.>"),
                    ("stream-allow", "ORDERS"),
                ],
            ),
            entry(
                "wasmcloud:nats/jetstream-handler@0.2.0",
                None,
                &[("subscriptions", "ORDERS:orders.eu.>")],
            ),
        ];

        let merged = merge(&entries).unwrap();
        assert_eq!(merged.len(), 1, "{merged:?}");
        let (binding, config) = &merged[0];
        assert_eq!(*binding, conn::UNNAMED_BINDING);

        let parsed = NatsConfig::from_map(config).unwrap();
        assert_eq!(parsed.servers, vec!["nats://localhost:4222"]);
        assert_eq!(parsed.policy.subject_allow, vec!["orders.>"]);
        assert_eq!(parsed.policy.stream_allow, vec!["ORDERS"]);
        assert_eq!(
            config.get("subscriptions").map(String::as_str),
            Some("ORDERS:orders.eu.>")
        );
    }

    #[test]
    fn entries_may_repeat_a_key_they_agree_on() {
        let entries = [
            entry(
                "wasmcloud:nats/core@0.2.0",
                None,
                &[("servers", "nats://localhost:4222")],
            ),
            entry(
                "wasmcloud:nats/core-handler@0.2.0",
                None,
                &[
                    ("servers", "nats://localhost:4222"),
                    ("core-subscriptions", "orders.eu.new"),
                ],
            ),
        ];
        assert_eq!(merge(&entries).unwrap().len(), 1);
    }

    #[test]
    fn conflicting_entries_are_refused_by_key_name() {
        let a = entry(
            "wasmcloud:nats/core@0.2.0",
            None,
            &[
                ("servers", "nats://localhost:4222"),
                ("subject-allow", "orders.eu.>"),
            ],
        );
        let b = entry(
            "wasmcloud:nats/core-handler@0.2.0",
            None,
            &[("subject_allow", "orders.>")],
        );

        // Both orders name the same key: the alias spellings collide, and the
        // fold does not depend on which entry is visited first.
        for entries in [[a.clone(), b.clone()], [b, a]] {
            let err = merge(&entries).unwrap_err().to_string();
            assert!(
                err.contains("conflicting values for `subject-allow`"),
                "{err}"
            );
            assert!(!err.contains("orders."), "message leaks values: {err}");
        }
    }

    #[test]
    fn a_binding_without_servers_is_still_refused() {
        let entries = [entry(
            "wasmcloud:nats/jetstream-handler@0.2.0",
            None,
            &[("subscriptions", "ORDERS:orders.>")],
        )];
        let merged = merge(&entries).unwrap();
        let err = NatsConfig::from_map(&merged[0].1).unwrap_err().to_string();
        assert!(err.contains("`servers`"), "{err}");
    }

    #[test]
    fn named_bindings_fold_separately() {
        let entries = [
            entry(
                "wasmcloud:nats/core@0.2.0",
                Some("hub"),
                &[
                    ("servers", "nats://hub:4222"),
                    ("subject-allow", "orders.>"),
                ],
            ),
            entry(
                "wasmcloud:nats/core@0.2.0",
                Some("leaf"),
                &[
                    ("servers", "nats://leaf:4222"),
                    ("subject-allow", "telemetry.>"),
                ],
            ),
        ];

        let merged = merge(&entries).unwrap();
        assert_eq!(
            merged.iter().map(|(b, _)| *b).collect::<Vec<_>>(),
            vec!["hub", "leaf"]
        );
        let hub = NatsConfig::from_map(&merged[0].1).unwrap();
        let leaf = NatsConfig::from_map(&merged[1].1).unwrap();
        assert_ne!(hub.connection_key(), leaf.connection_key());
    }

    /// The receive-only trap: a component that exports the handler and imports
    /// nothing does not match an entry naming an imported interface, so the
    /// connection settings written there are dropped before the plugin ever
    /// sees them. Failing with a bare "requires `servers`" would send the
    /// author looking at config they can see is present in the manifest.
    #[tokio::test]
    async fn connection_config_on_an_unmatched_entry_says_so() {
        let plugin = WasmcloudNats::new();
        let mut opened = false;
        let merged = HashMap::from([("core-subscriptions".to_string(), "orders.new".to_string())]);

        let err = plugin
            .open_bindings("wl", vec![(conn::UNNAMED_BINDING, merged)], &mut opened)
            .await
            .expect_err("no servers reached the plugin");
        let msg = err.to_string();

        assert!(
            msg.contains("`core-subscriptions`"),
            "the keys that did arrive are named: {msg}"
        );
        assert!(
            msg.contains("exports a handler"),
            "and the message points at the entry the settings belong on: {msg}"
        );
        assert!(!opened, "nothing was opened");
    }

    /// The common case: a workload on the cluster's own NATS names no servers,
    /// and inherits the host's data-plane address.
    #[tokio::test]
    async fn a_binding_without_servers_falls_back_to_the_host() {
        let plugin =
            WasmcloudNats::new().with_default_servers(vec!["nats://host:4222".to_string()]);
        let mut opened = false;
        let merged = HashMap::from([("subject-allow".to_string(), "orders.>".to_string())]);

        // Reaches a real connect attempt rather than the missing-`servers`
        // refusal, which is what says the default was applied.
        let err = plugin
            .open_bindings("wl", vec![(conn::UNNAMED_BINDING, merged)], &mut opened)
            .await
            .expect_err("nothing is listening on nats://host:4222");
        let msg = err.to_string();

        assert!(
            !msg.contains("has no `servers`"),
            "the host default satisfied the binding: {msg}"
        );
    }

    /// A binding that names its own servers means a different cluster, so the
    /// host default is replaced rather than merged into.
    #[tokio::test]
    async fn a_binding_with_servers_overrides_the_host_default() {
        let plugin =
            WasmcloudNats::new().with_default_servers(vec!["nats://host:4222".to_string()]);
        let mut opened = false;
        let merged = HashMap::from([
            ("servers".to_string(), "nats://elsewhere:4222".to_string()),
            ("subject-allow".to_string(), "orders.>".to_string()),
        ]);

        let err = plugin
            .open_bindings("wl", vec![(conn::UNNAMED_BINDING, merged)], &mut opened)
            .await
            .expect_err("nothing is listening on nats://elsewhere:4222");

        assert!(
            err.chain().any(|e| e.to_string().contains("elsewhere")),
            "the binding's own server is the one dialed: {err:#}"
        );
        assert!(
            !err.chain().any(|e| e.to_string().contains("host:4222")),
            "the host default is replaced, not merged: {err:#}"
        );
    }

    /// Inheriting the address must not inherit reach. A workload that names no
    /// servers and no grant still reaches nothing.
    #[test]
    fn the_host_default_carries_no_grant() {
        let plugin =
            WasmcloudNats::new().with_default_servers(vec!["nats://host:4222".to_string()]);
        assert!(
            plugin.default_servers.len() == 1,
            "only the address is defaulted"
        );

        let cfg = config::NatsConfig::from_map(&HashMap::from([(
            "servers".to_string(),
            "nats://host:4222".to_string(),
        )]))
        .expect("servers alone is a valid config");
        assert!(
            cfg.policy.subject_allow.is_empty(),
            "no grant comes with the address"
        );
    }
}

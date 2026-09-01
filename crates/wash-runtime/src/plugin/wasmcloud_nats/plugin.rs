//! The `wasmcloud:nats` host plugin: the connections a workload's bindings open
//! at bind, and the subscription loops those bindings carry.
//!
//! What a binding is described by is parsed in `super::config`; this module is
//! the `HostPlugin` lifecycle around it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use anyhow::Context as _;
use futures::StreamExt as _;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::engine::workload::{ResolvedWorkload, UnresolvedWorkload, WorkloadItem};
use crate::observability::Meters;
use crate::plugin::bindings::{UNNAMED_BINDING, describe_binding};
use crate::plugin::{HostPlugin, WitInterfaces, WorkloadFailureSink, WorkloadTracker};
use crate::wit::{WitInterface, WitWorld};

use super::config::{
    CoreSubscriptionConfig, JetStreamSubscriptionConfig, KvWatchConfig, NatsConfig,
    parse_core_subscriptions, parse_jetstream_subscriptions, parse_kv_watches, subscription_spec,
};
use super::conn::{self, ConnHandle, ConnectionRegistry};
use super::{NATS_VERSION, PLUGIN_NATS_ID, interfaces, subscriber};

/// The share of the host's guest-memory budget this plugin will promise to
/// NATS backlogs across every workload it carries.
///
/// Host-side backlog is not guest memory, but it comes out of the same
/// container, and it is the guests the budget is actually named for. A quarter
/// leaves three for them and is still far more than a subscription that is
/// keeping up ever holds.
const NATS_BACKLOG_BUDGET_DIVISOR: u64 = 4;

/// How many granted subjects one bind will ask the server about when looking
/// for a stream that captures them. See
/// [`WasmcloudNats::warn_on_silently_captured_subjects`].
const MAX_CAPTURE_CHECKS: usize = 32;

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
/// `wasmcloud:nats` host plugin — NATS-native capabilities split by interface.
pub struct WasmcloudNats {
    pub(super) tracker: Arc<RwLock<WorkloadTracker<(), ComponentData>>>,
    pub(super) connections: Arc<ConnectionRegistry>,
    pub(super) meters: Arc<RwLock<Meters>>,
    /// Subjects the host itself uses, denied to every workload.
    lattice_prefixes: Vec<String>,
    /// How a subscriber loop tells the host its workload has died out of band —
    /// a server-side permission denial parks a subscription that deployed
    /// cleanly, and nothing else would ever move the workload off running.
    failure_sink: arc_swap::ArcSwapOption<WorkloadFailureSink>,
    /// The host's guest-memory budget, when it told this plugin. Subscription
    /// byte budgets are per subscription and were never compared against it:
    /// seven subscriptions at the 32MiB default is 224MiB of potential backlog
    /// on a 256Mi host, before a single guest instantiates.
    memory_budget: Option<u64>,
    /// The ceiling on what every core subscription on this host may hold
    /// between them, enforced at admission rather than partitioned at bind.
    /// See [`subscriber::HostBacklogBudget`] for why a reservation was the
    /// wrong shape.
    host_backlog: Arc<subscriber::HostBacklogBudget>,
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
            failure_sink: arc_swap::ArcSwapOption::empty(),
            memory_budget: None,
            host_backlog: Arc::new(subscriber::HostBacklogBudget::unbounded()),
        }
    }

    /// Tells the plugin what the host's guest-memory budget is, so the total
    /// core backlog every subscription holds between them can be bounded
    /// against it.
    ///
    /// Without this the per-subscription default stands however many
    /// subscriptions a host ends up carrying, and the first sign of the
    /// mismatch is an OOMKill.
    pub fn with_memory_budget(mut self, max_guest_memory: u64) -> Self {
        self.memory_budget = Some(max_guest_memory);
        self.host_backlog = Arc::new(subscriber::HostBacklogBudget::new(
            max_guest_memory / NATS_BACKLOG_BUDGET_DIVISOR,
        ));
        self
    }

    /// Denies the host's own lattice subject space to every workload.
    pub fn with_lattice_prefixes(mut self, prefixes: Vec<String>) -> Self {
        self.lattice_prefixes = prefixes;
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
        for (binding, merged) in bindings {
            // Already resolved: the host's declaration for this binding, the
            // policy check, and the fold across the manifest's entries all
            // happened in `bind_plugins` before this plugin was called. What
            // arrives is one map per binding — which is what `connection_key`
            // needs, since two bindings that both fall back to the same host
            // configuration must read as one connection, not two.
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
            if super::keys::get(&merged, "servers").is_none() && !merged.is_empty() {
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

            // A stream grant alone reaches nothing: every read of a stored
            // message is checked against the *subject* grant, so a binding
            // granted streams and no subjects returns empty from `scan` and
            // `get-by-sequence` — indistinguishable from an empty stream, and
            // silent. The config can never return a message, so say so once at
            // bind rather than leaving it to be debugged from the outside.
            if !config.policy.stream_allow.is_empty() && config.policy.subject_allow.is_empty() {
                tracing::warn!(
                    workload_id,
                    binding = describe_binding(binding),
                    streams = config.policy.stream_allow.join(","),
                    "wasmcloud:nats binding grants streams but no subjects, so every JetStream \
                     read returns empty: a stored message is checked against `subject-allow` \
                     before it is handed over. Add the subjects those streams store to \
                     `subject-allow`"
                );
            }

            // Scope request-reply to this workload so two workloads on one
            // server cannot observe each other's responses. Named bindings get
            // one prefix each: two connections sharing an inbox prefix would
            // race for each other's replies. A declared prefix is scoped too,
            // never taken as-is — under `Deny` the host's named binding is
            // served to *every* workload that asks for it.
            config.inbox_prefix = Some(match config.inbox_prefix.take() {
                Some(declared) => conn::scope_inbox_prefix(&declared, workload_id),
                None => conn::binding_inbox_prefix(workload_id, binding),
            });

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
            // The grant, which decides every later denial. Never the merged
            // map itself: it carries creds, tokens and passwords.
            debug!(
                workload_id,
                binding = describe_binding(binding),
                subject_allow = config.policy.subject_allow.join(","),
                stream_allow = config.policy.stream_allow.join(","),
                bucket_allow = config.policy.bucket_allow.join(","),
                inbox_prefix = config.inbox_prefix.as_deref().unwrap_or_default(),
                "wasmcloud:nats binding grant"
            );
        }

        Ok(())
    }

    /// Says when this host is carrying more core subscriptions than its
    /// backlog ceiling can hold at once, without narrowing anything.
    ///
    /// The first attempt at bounding the host-wide total *did* narrow it, and
    /// that was the mistake: it partitioned the ceiling at bind, so an
    /// operator who had tuned `subscription-capacity` to 65,536 messages got a
    /// 1MiB buffer — 1.6% of what they asked for — chosen by the order the
    /// bindings happened to resolve in, and the only trace was a line saying
    /// it had "clamped". Configuration the operator wrote must not be
    /// overridden silently; a ceiling that can actually be hit at run time
    /// belongs at admission, where it can be reported with what was queued at
    /// the time. See [`subscriber::HostBacklogBudget`].
    ///
    /// So this is advisory only. It fires when the arithmetic says the
    /// subscriptions *could* collectively exceed the ceiling — not that they
    /// will, since a subscription keeping up holds nothing.
    fn warn_on_oversubscribed_backlog(
        &self,
        workload_id: &str,
        binding: &str,
        handle: &ConnHandle,
        subscriptions: usize,
    ) {
        let Some(budget) = self.memory_budget else {
            return;
        };
        if subscriptions == 0 {
            return;
        }
        let ceiling = self.host_backlog.ceiling();
        let worst_case =
            (handle.limits.subscription_capacity_bytes as u64).saturating_mul(subscriptions as u64);
        if worst_case <= ceiling {
            return;
        }
        tracing::warn!(
            workload_id,
            binding = describe_binding(binding),
            subscriptions,
            capacity_bytes = handle.limits.subscription_capacity_bytes,
            worst_case_bytes = worst_case,
            host_backlog_ceiling_bytes = ceiling,
            host_budget_bytes = budget,
            "wasmcloud:nats: {subscriptions} core subscriptions at \
             `subscription-capacity-bytes` could hold {worst_case} bytes between them, past \
             the {ceiling} this host allows for NATS backlogs across every workload. Nothing \
             is narrowed — each subscription still buffers what it was configured to — but \
             once the host-wide total is reached, further deliveries shed with \
             `reason=\"host memory budget\"` rather than `reason=\"byte budget\"`. Raise the \
             host's memory budget, lower `subscription-capacity-bytes`, or split the \
             subscriptions across hosts."
        );
    }

    /// Says, once per bind, when a subject this binding may publish to is
    /// captured by a stream whose per-message limit is below the connection's.
    ///
    /// A core publish is fire-and-forget: it resolves once written to the
    /// connection, not once accepted. JetStream refusing it above the stream's
    /// own limit is therefore silent at every layer — the publisher reports
    /// OK, the stream stores nothing, consumers see nothing, this host logs
    /// nothing. The overlap is computable before any traffic, so it is said
    /// before any traffic.
    ///
    /// Best effort throughout: a server without JetStream, a stream that does
    /// not exist yet, or a lookup that fails is not a reason to refuse a bind.
    ///
    /// Detached rather than awaited: it is a round trip to the server for a log
    /// line, and every subscription on the binding would otherwise attach that
    /// much later. Nothing downstream reads its result.
    fn warn_on_silently_captured_subjects(
        workload_id: &str,
        binding: &str,
        handle: Arc<ConnHandle>,
    ) {
        let workload_id = workload_id.to_string();
        let binding = binding.to_string();
        tokio::spawn(async move {
            let (workload_id, binding) = (workload_id.as_str(), binding.as_str());
            Self::report_silently_captured_subjects(workload_id, binding, &handle).await;
        });
    }

    async fn report_silently_captured_subjects(
        workload_id: &str,
        binding: &str,
        handle: &ConnHandle,
    ) {
        let max_payload = handle.max_payload();
        // Enumerate the streams and ask which of *them* this grant can reach,
        // rather than asking the server which stream captures each granted
        // subject. `stream_by_subject` answers for one concrete subject, so
        // the subject-driven version silently checked nothing at all on a
        // deployment whose workloads grant `bench.>` or `fan.*` — which is
        // every workload on the rig this was written for. A stream's
        // `config.subjects` are patterns and the grant is patterns, so the
        // question is whether the two sets intersect.
        let mut streams = handle.jetstream.streams();
        let mut examined = 0usize;
        while let Some(info) = streams.next().await {
            let Ok(info) = info else {
                // No JetStream, no permission to list, a transport blip: none
                // of these are a reason to refuse a bind over an advisory.
                break;
            };
            examined += 1;
            if examined > MAX_CAPTURE_CHECKS {
                debug!(
                    workload_id,
                    binding = describe_binding(binding),
                    examined = MAX_CAPTURE_CHECKS,
                    "stopped checking for silently size-capped streams; this server has more \
                     streams than one bind will enumerate"
                );
                break;
            }
            let limit = info.config.max_message_size;
            // `-1` is the stream deferring to the server, which is the same
            // limit the publish already honours: nothing to warn about.
            if limit <= 0 || limit as u64 >= max_payload {
                continue;
            }
            let Some(captured) = info
                .config
                .subjects
                .iter()
                .find(|subject| handle.policy.overlaps_subject_pattern(subject))
            else {
                continue;
            };
            let stream_name = &info.config.name;
            let grant = handle.policy.granted_subject_patterns().join(",");
            tracing::warn!(
                workload_id,
                binding = describe_binding(binding),
                stream = %stream_name,
                captured_subject = %captured,
                subject_allow = %grant,
                stream_max_message_size = limit,
                connection_max_payload = max_payload,
                "this workload may publish into '{captured}', which stream '{stream_name}' \
                 captures and which refuses messages above {limit} bytes — below this \
                 connection's {max_payload}-byte limit. A core (fire-and-forget) publish over \
                 that size is dropped by JetStream silently: neither the publisher nor this \
                 host observes the loss. Use `jetstream.publish` (acked) for JetStream-bound \
                 subjects."
            );
        }
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
        execution_meter: crate::observability::ExecutionTimeMeter,
        warm_set: Arc<subscriber::JetStreamWarmSet>,
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

        debug!(
            workload_id,
            component_id,
            binding = describe_binding(binding),
            jetstream_subscriptions = jetstream_subs.len(),
            core_subscriptions = core_subs.len(),
            kv_watches = kv_watches.len(),
            "starting wasmcloud:nats subscriptions"
        );

        // The per-subscription byte budget is whatever the binding configured;
        // the host-wide ceiling is a second bound applied at admission. This
        // only says when the two are in tension, and never narrows the first.
        self.warn_on_oversubscribed_backlog(workload_id, binding, &handle, core_subs.len());

        // A core publish into a subject a stream captures is refused by
        // JetStream above the stream's own per-message limit, and neither the
        // publisher nor this host observes the loss. Nothing at run time can
        // see it, so it is said here, once, while the coordinates are known.
        Self::warn_on_silently_captured_subjects(workload_id, binding, handle.clone());

        if !jetstream_subs.is_empty() {
            subscriber::spawn_jetstream_subscriptions(
                workload,
                component_id,
                handle.clone(),
                jetstream_subs,
                cancel_token.clone(),
                execution_meter.clone(),
                failure_sink.clone(),
                workload_id,
                warm_set.clone(),
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
                failure_sink.clone(),
                workload_id,
                self.host_backlog.clone(),
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

/// The binding name an interface routes under, exactly as the generic layer
/// reads it.
///
/// A label equal to the plugin's own id is a workload naming the plugin rather
/// than a backend, so it routes to the unnamed binding. Reading it as a name
/// here would let `(implements wasmcloud-nats)` skip the undeclared-name
/// refusal and open a connection the operator never declared.
fn binding_name(interface: &WitInterface) -> &str {
    match interface.name.as_deref() {
        Some(name) if name != PLUGIN_NATS_ID => name,
        _ => UNNAMED_BINDING,
    }
}

/// Groups the bound entries by binding name.
///
/// The fold that used to live here — the union across every entry of one label,
/// with a key two of them disagree about refused — moved into the generic
/// binding layer, because it has to happen before the operator's host layer
/// lands rather than after. It is also not about NATS: "one label is one
/// config" is a property of every plugin that serves named bindings.
///
/// What remains is the read side. By the time this runs, every entry of a
/// label already carries the same resolved map, so taking the first is taking
/// all of them.
fn bindings_by_name<'a>(bound: &[&'a WitInterface]) -> Vec<(&'a str, HashMap<String, String>)> {
    let mut merged: BTreeMap<&str, HashMap<String, String>> = BTreeMap::new();
    for interface in bound {
        merged
            .entry(binding_name(interface))
            .or_insert_with(|| interface.config.clone());
    }
    merged.into_iter().collect()
}

fn serves(interface: &WitInterface, names: &[&str]) -> bool {
    interface
        .interfaces
        .iter()
        .any(|name| names.contains(&name.as_str()))
}

#[async_trait::async_trait]
impl HostPlugin for WasmcloudNats {
    fn binding_schema(&self) -> crate::plugin::BindingSchema {
        super::binding_schema()
    }

    /// Parses the operator's declaration with this plugin's own reader, so a
    /// binding an operator wrote wrong fails the host rather than the first
    /// workload that names it.
    fn validate_bindings(&self, declared: &crate::plugin::PluginBindingSet) -> anyhow::Result<()> {
        // `inbox-prefix` on the base layer is every binding's inbox prefix, on
        // every workload the host runs. `conn::scope_inbox_prefix` gives each
        // workload its own token beneath whatever prefix it is handed, so this
        // is refused for what it says rather than for what it would do: one
        // inbox root for every binding on the host is not a thing an operator
        // can have meant. On a named binding it is kept, and scoped.
        if declared
            .base()
            .keys()
            .any(|k| super::keys::canonical(k) == "inbox-prefix")
        {
            anyhow::bail!(
                "`inbox-prefix` cannot be set on a wasmcloud:nats entry's own `config`: it would \
                 give every workload on this host the same inbox, and two workloads sharing an \
                 inbox consume each other's replies. Set it on a single named binding, or leave \
                 it unset — the per-workload default already isolates replies"
            )
        }

        // The base alone is not required to be complete: a host that sets only
        // grants leaves the servers to a workload under `allow`.
        for (name, layer) in declared.host_layers(&super::binding_schema()) {
            NatsConfig::from_map(&layer).map_err(|e| anyhow::anyhow!("binding `{name}`: {e:#}"))?;
        }
        Ok(())
    }

    /// Whether a workload's grant is inside the one the operator declared.
    ///
    /// The containment function already existed — [`super::policy::NatsSubjectPattern`]
    /// answers exactly this question for subscriptions — so a grant an operator
    /// declares as a ceiling costs the plugin nothing beyond routing each key to
    /// the right comparison.
    fn narrows(&self, key: &str, ceiling: &str, value: &str) -> bool {
        fn split(s: &str) -> impl Iterator<Item = &str> {
            s.split(',').map(str::trim).filter(|s| !s.is_empty())
        }
        match key {
            // Pattern containment: `orders.received` is inside `orders.>`.
            "subject-allow" => {
                let ceiling: Vec<_> = split(ceiling)
                    .map(super::policy::NatsSubjectPattern::parse)
                    .collect();
                split(value)
                    .map(super::policy::NatsSubjectPattern::parse)
                    .all(|v| ceiling.iter().any(|c| c.contains(&v)))
            }
            // Plain names: subset, not pattern containment.
            "stream-allow" | "bucket-allow" => {
                let ceiling: std::collections::HashSet<&str> = split(ceiling).collect();
                split(value).all(|v| ceiling.contains(v))
            }
            // An empty workload list trivially narrows — deny-all is the
            // narrowest thing there is — and falls out of `all()` above with no
            // special case.
            _ => false,
        }
    }

    fn id(&self) -> &'static str {
        PLUGIN_NATS_ID
    }

    fn world(&self) -> WitWorld {
        const IMPORTS: &str = "wasmcloud:nats/types,core,jetstream,kv";
        const EXPORTS: &str = "wasmcloud:nats/jetstream-handler,core-handler,kv-handler";
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                format!("{IMPORTS}@{NATS_VERSION}").as_str(),
            )]),
            exports: HashSet::from([WitInterface::from(
                format!("{EXPORTS}@{NATS_VERSION}").as_str(),
            )]),
        }
    }

    /// Named (`(implements ..)`) bindings route per label: one connection, one
    /// grant, and one set of subscriptions each, so a component can bridge two
    /// clusters by importing `wasmcloud:nats` twice under different names.
    ///
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
        let bindings = bindings_by_name(&bound);

        // A bind that fails partway has already opened every connection ahead
        // of the failure, and the workload it belongs to will never resolve.
        // The engine unbinds the plugin that failed, but the sockets are this
        // plugin's to close: left registered, each keeps a client and its event
        // task alive until the host restarts, and the next deploy under the
        // same workload id is then refused for conflicting with a connection
        // nothing is using.
        debug!(
            workload_id,
            bindings = bindings
                .iter()
                .map(|(binding, _)| describe_binding(binding))
                .collect::<Vec<_>>()
                .join(","),
            "opening wasmcloud:nats bindings"
        );
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
        {
            use interfaces::bindings::wasmcloud::nats as nats_wit;
            // `types` carries record and error definitions only: nothing to
            // route, so it is bound once either way.
            nats_wit::types::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                linker,
                crate::engine::ctx::extract_active_ctx,
            )?;
            if has_plain {
                nats_wit::core::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                    linker,
                    crate::engine::ctx::extract_active_ctx,
                )?;
                nats_wit::jetstream::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                    linker,
                    crate::engine::ctx::extract_active_ctx,
                )?;
                nats_wit::kv::add_to_linker::<_, crate::engine::ctx::SharedCtx>(
                    linker,
                    crate::engine::ctx::extract_active_ctx,
                )?;
            }
            if has_labeled {
                use interfaces::bindings::named_imports::wasmcloud::nats as labeled_nats_wit;
                let route = |label: &str| -> wasmtime::Result<interfaces::NatsId> {
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

        // Before the entries are read, not after: a service has no manifest
        // name, so `component:` could only ever match its per-start UUID, and
        // the advice in every error below would be impossible to follow.
        let WorkloadItem::Component(component) = item else {
            anyhow::bail!("wasmcloud:nats handlers are only supported on components")
        };

        let mut jetstream_subs = Vec::new();
        let mut core_subs = Vec::new();
        let mut kv_watches = Vec::new();
        let mut untargeted_specs: Vec<String> = Vec::new();

        let component_id = component.id().to_string();
        // The manifest's `components[].name`, alongside the runtime id: a
        // manifest is written before the workload exists, so the id — a fresh
        // UUID per start — is not something an author can name.
        let component_name = component.name().to_string();
        let workload_id = component.workload_id().to_string();

        for interface in &bound {
            let binding = binding_name(interface);
            // Every other key this plugin reads accepts either spelling, so
            // these do too.
            let cfg = |key: &str| -> Option<&String> {
                interface
                    .config
                    .iter()
                    .find(|(k, _)| super::keys::canonical(k) == key)
                    .map(|(_, v)| v)
            };
            // The runtime hands an unnamed host-interface entry to every
            // component whose world covers it, which for a subscription means
            // every handler in the workload receives every message. `component`
            // is how an author says which handler a subscription was written
            // for; entries that name another component are simply not this
            // component's.
            let targeted = match cfg("component") {
                Some(target) if *target != component_id && *target != component_name => continue,
                Some(_) => true,
                None => false,
            };
            if serves(interface, &["handler", "jetstream-handler"])
                && let Some(raw) = cfg("jetstream-subscriptions")
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
                        subscription_spec("jetstream-subscriptions", binding, value)
                    }));
                }
                jetstream_subs.extend(subs);
            }
            if serves(interface, &["handler", "core-handler"])
                && let Some(raw) = cfg("core-subscriptions")
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
                && let Some(raw) = cfg("kv-watches")
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
                         would be handled twice. Add `component: <name>` to each \
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

        let execution_meter = self.meters.read().await.execution_time.clone();

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

        // Per component, not per binding: `poolSize` is the component's, and
        // the parked stores are interchangeable across its bindings.
        let warm_set = subscriber::jetstream_warm_set(workload, component_id).await;

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
                execution_meter.clone(),
                warm_set.clone(),
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
        debug!(
            workload_id,
            "released every wasmcloud:nats connection and cancelled its subscriptions"
        );
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
    use crate::plugin::{PluginBindingSet, WorkloadConfigPolicy};

    #[test]
    fn world_advertises_the_package() {
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
        assert_eq!(versions(&world.imports), vec!["0.1.0"]);
        assert_eq!(versions(&world.exports), vec!["0.1.0"]);
        for iface in world.exports.iter() {
            assert!(
                iface.interfaces.contains("kv-handler"),
                "export entry lost an interface: {:?}",
                iface.interfaces
            );
        }
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

    /// Run `entries` through the generic binding layer the way `bind_plugins`
    /// does, with nothing declared, then group them the way this plugin does.
    ///
    /// The fold itself lives in [`crate::plugin::bindings`] now; these tests
    /// stay because what they cover is a `wasmcloud:nats` manifest shape — a
    /// binding split across an import entry and a handler entry — and that
    /// shape has to keep working whoever owns the folding.
    fn merge(entries: &[WitInterface]) -> anyhow::Result<Vec<(String, HashMap<String, String>)>> {
        // `allow`, the `wash dev` posture: these cover the *fold*, and a
        // manifest that describes its own binding is exactly what the fold is
        // for. Under `deny` the same manifest is refused before the fold is
        // reached — which the policy tests below cover.
        let declared = crate::plugin::PluginBindingSet::new(super::super::PLUGIN_NATS_ID)
            .with_workload_config(WorkloadConfigPolicy::Allow);
        let set: std::collections::HashSet<WitInterface> = entries.iter().cloned().collect();
        let resolved = declared.resolve_by_name(
            &set,
            &super::super::binding_schema(),
            &|key: &str, host: &str, workload: &str| {
                WasmcloudNats::default().narrows(key, host, workload)
            },
        )?;
        Ok(resolved.into_iter().collect())
    }

    #[test]
    fn split_entries_fold_into_one_binding() {
        let entries = [
            entry(
                "wasmcloud:nats/types,core,jetstream@0.1.0",
                None,
                &[
                    ("servers", "nats://localhost:4222"),
                    ("subject-allow", "orders.>"),
                    ("stream-allow", "ORDERS"),
                ],
            ),
            entry(
                "wasmcloud:nats/jetstream-handler@0.1.0",
                None,
                &[("jetstream-subscriptions", "ORDERS:orders.eu.>")],
            ),
        ];

        let merged = merge(&entries).unwrap();
        assert_eq!(merged.len(), 1, "{merged:?}");
        let (binding, config) = &merged[0];
        assert_eq!(binding, UNNAMED_BINDING);

        let parsed = NatsConfig::from_map(config).unwrap();
        assert_eq!(parsed.servers, vec!["nats://localhost:4222"]);
        assert_eq!(parsed.policy.subject_allow, vec!["orders.>"]);
        assert_eq!(parsed.policy.stream_allow, vec!["ORDERS"]);
        assert_eq!(
            config.get("jetstream-subscriptions").map(String::as_str),
            Some("ORDERS:orders.eu.>")
        );
    }

    #[test]
    fn entries_may_repeat_a_key_they_agree_on() {
        let entries = [
            entry(
                "wasmcloud:nats/core@0.1.0",
                None,
                &[("servers", "nats://localhost:4222")],
            ),
            entry(
                "wasmcloud:nats/core-handler@0.1.0",
                None,
                &[
                    ("servers", "nats://localhost:4222"),
                    ("core-subscriptions", "orders.eu.new"),
                ],
            ),
        ];
        assert_eq!(merge(&entries).unwrap().len(), 1);
    }

    /// A label equal to the plugin's own id routes to the unnamed binding, so a
    /// manifest pairing a plain entry with `(implements wasmcloud-nats)` gets
    /// one binding. Read as a name it would skip the undeclared-name refusal
    /// and then open a *second* connection to the same servers, under its own
    /// inbox prefix.
    #[test]
    fn a_label_naming_the_plugin_is_the_unnamed_binding() {
        let plain = entry(
            "wasmcloud:nats/core@0.1.0",
            None,
            &[("servers", "nats://localhost:4222")],
        );
        let labeled = entry(
            "wasmcloud:nats/core-handler@0.1.0",
            Some(super::super::PLUGIN_NATS_ID),
            &[("core-subscriptions", "orders.new")],
        );
        assert_eq!(binding_name(&labeled), UNNAMED_BINDING);

        let merged = merge(&[plain, labeled]).unwrap();
        assert_eq!(
            merged.len(),
            1,
            "one binding, so `open_bindings` opens one connection: {merged:?}"
        );
        assert_eq!(merged[0].0, UNNAMED_BINDING);
    }

    #[test]
    fn conflicting_entries_are_refused_by_key_name() {
        let a = entry(
            "wasmcloud:nats/core@0.1.0",
            None,
            &[
                ("servers", "nats://localhost:4222"),
                ("subject-allow", "orders.eu.>"),
            ],
        );
        let b = entry(
            "wasmcloud:nats/core-handler@0.1.0",
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
            "wasmcloud:nats/jetstream-handler@0.1.0",
            None,
            &[("jetstream-subscriptions", "ORDERS:orders.>")],
        )];
        let merged = merge(&entries).unwrap();
        let err = NatsConfig::from_map(&merged[0].1).unwrap_err().to_string();
        assert!(err.contains("`servers`"), "{err}");
    }

    #[test]
    fn named_bindings_fold_separately() {
        let entries = [
            entry(
                "wasmcloud:nats/core@0.1.0",
                Some("hub"),
                &[
                    ("servers", "nats://hub:4222"),
                    ("subject-allow", "orders.>"),
                ],
            ),
            entry(
                "wasmcloud:nats/core@0.1.0",
                Some("leaf"),
                &[
                    ("servers", "nats://leaf:4222"),
                    ("subject-allow", "telemetry.>"),
                ],
            ),
        ];

        let merged = merge(&entries).unwrap();
        assert_eq!(
            merged.iter().map(|(b, _)| b.as_str()).collect::<Vec<_>>(),
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
            .open_bindings("wl", vec![(UNNAMED_BINDING, merged)], &mut opened)
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

    /// The host's declaration reaches the plugin: a binding that names no
    /// servers of its own gets the host's address and reaches a real connect
    /// attempt rather than the missing-`servers` refusal.
    ///
    /// Resolution is the generic layer's now, so the test drives it the way
    /// `bind_plugins` does and hands the plugin what comes out.
    #[tokio::test]
    async fn the_host_declaration_reaches_open_bindings() {
        let declared = PluginBindingSet::new(super::super::PLUGIN_NATS_ID)
            .with_default_bundle("servers", [("servers", "nats://host:4222")])
            .with_base(HashMap::from([(
                "subject-allow".to_string(),
                "orders.>".to_string(),
            )]));
        let plugin = WasmcloudNats::new();

        let merged = declared
            .resolve(
                UNNAMED_BINDING,
                &HashMap::from([("subject-allow".to_string(), "orders.received".to_string())]),
                &super::super::binding_schema(),
                &|key: &str, host: &str, workload: &str| plugin.narrows(key, host, workload),
            )
            .expect("the workload sets only a grant it may narrow into");

        let mut opened = false;
        let err = plugin
            .open_bindings("wl", vec![(UNNAMED_BINDING, merged)], &mut opened)
            .await
            .expect_err("nothing is listening on nats://host:4222");
        let msg = err.to_string();

        assert!(
            !msg.contains("has no `servers`"),
            "the host default satisfied the binding: {msg}"
        );
    }

    /// A workload may take less of a grant than the operator declared, and is
    /// refused for taking more. The predicate is this plugin's; the layer only
    /// asks it.
    #[test]
    fn a_workload_may_narrow_a_grant_but_not_widen_one() {
        let declared = PluginBindingSet::new(super::super::PLUGIN_NATS_ID)
            .with_workload_config(WorkloadConfigPolicy::Deny)
            .with_base(HashMap::from([(
                "servers".to_string(),
                "nats://host:4222".to_string(),
            )]))
            .with_binding(
                "orders",
                HashMap::from([
                    ("subject-allow".to_string(), "orders.>".to_string()),
                    ("stream-allow".to_string(), "ORDERS,PROCESSED".to_string()),
                ]),
            );
        let plugin = WasmcloudNats::new();
        let schema = super::super::binding_schema();
        let narrows = |key: &str, host: &str, workload: &str| plugin.narrows(key, host, workload);

        let resolved = declared
            .resolve(
                "orders",
                &HashMap::from([
                    ("subject-allow".to_string(), "orders.received".to_string()),
                    ("stream-allow".to_string(), "ORDERS".to_string()),
                ]),
                &schema,
                &narrows,
            )
            .expect("asking for less than the ceiling is the point of a ceiling");
        assert_eq!(
            resolved["subject-allow"], "orders.received",
            "the workload runs with what it asked for, not a computed intersection"
        );
        assert_eq!(resolved["stream-allow"], "ORDERS");

        let err = declared
            .resolve(
                "orders",
                &HashMap::from([(
                    "subject-allow".to_string(),
                    "orders.received,billing.>".to_string(),
                )]),
                &schema,
                &narrows,
            )
            .expect_err("a workload may never widen a grant")
            .to_string();
        assert!(err.contains("`billing.>` is outside it"), "{err}");
    }

    /// A grant the operator never declared cannot be narrowed into: an
    /// ungranted allowlist resolves to empty, not to whatever the manifest
    /// wrote.
    #[test]
    fn a_ceiling_nobody_declared_grants_nothing() {
        let declared = PluginBindingSet::new(super::super::PLUGIN_NATS_ID)
            .with_workload_config(WorkloadConfigPolicy::Deny)
            .with_binding("orders", HashMap::new());
        let plugin = WasmcloudNats::new();

        let err = declared
            .resolve(
                "orders",
                &HashMap::from([("subject-allow".to_string(), "orders.>".to_string())]),
                &super::super::binding_schema(),
                &|key: &str, host: &str, workload: &str| plugin.narrows(key, host, workload),
            )
            .expect_err("nothing contains a grant that was never declared")
            .to_string();
        assert!(err.contains("declares no ceiling"), "{err}");
    }

    /// A refusal from the host's declaration fails the bind, and names the
    /// binding it came from.
    #[test]
    fn a_refused_workload_key_fails_the_bind() {
        let declared = PluginBindingSet::new(super::super::PLUGIN_NATS_ID)
            .with_workload_config(WorkloadConfigPolicy::Deny)
            .with_base(HashMap::from([(
                "servers".to_string(),
                "nats://host:4222".to_string(),
            )]))
            .with_binding("orders", HashMap::new());
        let plugin = WasmcloudNats::new();

        let err = declared
            .resolve(
                "orders",
                &HashMap::from([("servers".to_string(), "nats://elsewhere:4222".to_string())]),
                &super::super::binding_schema(),
                &|key: &str, host: &str, workload: &str| plugin.narrows(key, host, workload),
            )
            .expect_err("a manifest may not point itself at another cluster")
            .to_string();
        assert!(err.contains("`servers`"), "names the refused key: {err}");
    }
}

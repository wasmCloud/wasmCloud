//! The optional `wasmcloud:host/workload-lifecycle` export a host component
//! plugin may provide to hear about the workloads calling it: the host delivers
//! `on-workload-bind` (with the workload's identity and per-interface manifest
//! config) before any capability call from that workload, and
//! `on-workload-unbind` when it goes away — so the plugin can provision and
//! reclaim per-workload state eagerly rather than lazily on first call.
//!
//! Both hooks' signatures are shape-checked at registration
//! ([`lifecycle_funcs`]). A bind that overruns the plugin's lifecycle-call
//! timeout fails the deploy at the budget rather than waiting the
//! (uncancellable) hook out; because the hook keeps running, the rollback
//! unbind is *deferred* until it returns ([`spawn_deferred_unbind`]), so state
//! it provisions late is still reclaimed.
//!
//! Because a supervised restart rebuilds the store (losing all guest state)
//! while bound workloads keep running, each fresh incarnation replays
//! `on-workload-bind` for every workload still bound ([`replay_snapshot`]) —
//! completing the replay before serving any queued capability call, which is
//! why the hook must be idempotent. Replay is serial: a guest bind trap
//! short-circuits `run_concurrent` and drops the serving future before its
//! handler runs, so the fault can only be attributed by the marker the serve
//! loop records *before* each replayed bind (`ReplayProgress`, read by
//! [`attribute_replay_fault`]). A bind that crash-loops the shared store is
//! struck on each attributed fault and, past a strike ceiling
//! ([`POISON_EVICT_STRIKES`]), evicted — dropped from the replay set so the
//! store recovers for every other tenant — and reported to the host so the
//! evicted workload's scheduling health becomes failed. Such attributed faults
//! do not consume the plugin-wide restart budget.
//!
//! [`ComponentHostPlugin`](super::ComponentHostPlugin) drives all of this from
//! its [`HostPlugin`](crate::plugin::HostPlugin) impl.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Context as _;
use tracing::{debug, error, warn};
use wasmtime::component::Val;
use wasmtime::component::types::Type;

use crate::engine::abandon::DispatchedCall;
use crate::engine::ctx::{BindGeneration, CallerIdentity, WorkloadCaller};
use crate::engine::store::relocate::Relocated;
use crate::engine::workload::UnresolvedWorkload;
use crate::host::trigger_service::{CapabilityCall, CapabilityJob, LifecycleReplay};
use crate::plugin::{SecretValue, WitInterfaces};
use crate::wit::{ConfigValue, WitInterface};

use super::{CapabilitySender, ComponentHostPluginState, ExportedFunc, ExportedInterface};

/// Interface name of the optional lifecycle export a plugin may provide to
/// hear about the workloads binding to (and unbinding from) it. Bare name,
/// not a full `namespace:package/name@version` string: `wasmcloud:host`
/// versions as one package, so matching by name only means a patch bump for
/// an unrelated addition in the same package can't break a plugin already
/// built against this export.
pub(super) const HOST_LIFECYCLE_EXPORT: &str = "workload-lifecycle";
/// The lifecycle export's bind hook.
const ON_WORKLOAD_BIND: &str = "on-workload-bind";
/// The lifecycle export's unbind hook.
const ON_WORKLOAD_UNBIND: &str = "on-workload-unbind";

/// Consecutive `on-workload-bind` traps a workload may accumulate before it is
/// evicted (dropped from the replay set and reported failed). Kept below
/// [`DEFAULT_MAX_RESTARTS`](super::DEFAULT_MAX_RESTARTS) so a
/// deterministically-poison bind is quarantined — contained to its own workload
/// — before its crash loop can exhaust the whole plugin's restart budget and
/// take every tenant down with it. Strikes reset
/// after a healthy uptime, so only a rapid crash loop escalates to eviction.
pub(super) const POISON_EVICT_STRIKES: u32 = 2;

/// The plugin's `wasmcloud:host/workload-lifecycle` export, introspected at
/// construction: the instance name plus both hook functions' types, ready to
/// drive the dynamic cross-store calls.
pub(super) struct LifecycleFuncs {
    pub(super) interface: Arc<str>,
    pub(super) bind: ExportedFunc,
    pub(super) unbind: ExportedFunc,
    /// Whether the plugin imports `wasmcloud:host/binding-secrets`, the only
    /// way it can read a binding's secrets.
    pub(super) takes_secrets: bool,
}

/// Forget `workload_id` and deliver `on-workload-unbind` for it. The removal
/// and the sender read share one `bound` lock so the unbind lands on the
/// incarnation that will not also replay the removed entry (see
/// [`replay_snapshot`]).
pub(super) async fn remove_and_unbind(
    state: &ComponentHostPluginState,
    lifecycle: &LifecycleFuncs,
    workload_id: &Arc<str>,
) -> anyhow::Result<()> {
    let (was_bound, sender) = {
        let mut bound = state
            .bound
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let removed = bound.remove(workload_id.as_ref());
        if let Some(info) = &removed {
            info.generation.revoke();
        }
        let was_bound = removed.is_some();
        (was_bound, state.sender())
    };
    // Only a workload whose bind we actually delivered gets an unbind:
    // this dedupes the engine's per-item unbind fan-out (a plugin bound to
    // N items is unbound N times) and skips ids that were never bound.
    if !was_bound {
        return Ok(());
    }
    let sender = sender
        .ok_or_else(|| anyhow::anyhow!("host component plugin '{}' is not running", state.id))?;
    call_lifecycle(
        &sender,
        state,
        lifecycle,
        &lifecycle.unbind,
        Relocated::Val(Val::String(workload_id.to_string())),
        workload_id,
    )
    .await
    .map(|_| ())
}

/// Pull the two lifecycle hooks out of the plugin's introspected
/// `wasmcloud:host/workload-lifecycle` export. Presence, arity, and the
/// top-level shape of each signature are checked here so a mismatched plugin
/// fails at registration with a clear message rather than on its first
/// workload deploy. Deep per-field types (every field of `workload-info`)
/// still surface as a per-delivery typecheck error — the fixed shape below
/// catches the mistakes worth catching cheaply.
pub(super) fn lifecycle_funcs(
    id: &str,
    mut export: ExportedInterface,
    takes_secrets: bool,
) -> anyhow::Result<LifecycleFuncs> {
    let mut bind = None;
    let mut unbind = None;
    for func in export.funcs.drain(..) {
        match func.name.as_ref() {
            ON_WORKLOAD_BIND => bind = Some(func),
            ON_WORKLOAD_UNBIND => unbind = Some(func),
            _ => {}
        }
    }
    let bind = bind.with_context(|| {
        format!("host component plugin '{id}' lifecycle export is missing {ON_WORKLOAD_BIND}")
    })?;
    let unbind = unbind.with_context(|| {
        format!("host component plugin '{id}' lifecycle export is missing {ON_WORKLOAD_UNBIND}")
    })?;

    // `on-workload-bind: func(workload-info) -> result<_, string>`
    anyhow::ensure!(
        bind.result_tys.len() == 1 && matches!(bind.param_tys.first(), Some(Type::Record(_))),
        "host component plugin '{id}' {ON_WORKLOAD_BIND} must take one argument, the workload-info \
         record"
    );
    match bind.result_tys.first() {
        Some(Type::Result(rt)) => anyhow::ensure!(
            rt.ok().is_none() && matches!(rt.err(), Some(Type::String)),
            "host component plugin '{id}' {ON_WORKLOAD_BIND} must return result<_, string>"
        ),
        _ => anyhow::bail!(
            "host component plugin '{id}' {ON_WORKLOAD_BIND} must return result<_, string>"
        ),
    }

    // `on-workload-unbind: func(workload-id)` — one string, no result.
    anyhow::ensure!(
        unbind.result_tys.is_empty() && matches!(unbind.param_tys.first(), Some(Type::String)),
        "host component plugin '{id}' {ON_WORKLOAD_UNBIND} must take one argument, the workload-id \
         string, and return nothing"
    );

    Ok(LifecycleFuncs {
        interface: export.name,
        bind,
        unbind,
        takes_secrets,
    })
}

/// Snapshot the bound-workloads map into the replay list for a fresh
/// incarnation. MUST be taken while holding the `bound` lock, atomically with
/// publishing the incarnation's sender: that exclusion is what guarantees a
/// concurrent bind/unbind is either reflected in the snapshot (recorded before
/// the swap, its own send targeting the previous, now-dead channel) or
/// delivered through the new channel — never both, so no double delivery.
pub(super) fn replay_snapshot(
    bound: &BTreeMap<Arc<str>, BindInfo>,
    lifecycle: Option<&LifecycleFuncs>,
) -> Vec<LifecycleReplay> {
    let Some(lifecycle) = lifecycle else {
        return Vec::new();
    };
    bound
        .iter()
        .filter(|(_, info)| info.generation.is_active())
        .map(|(workload_id, info)| LifecycleReplay {
            interface: Arc::clone(&lifecycle.interface),
            func: Arc::clone(&lifecycle.bind.name),
            // A replayed bind is about the workload, not any component of it
            // nor any one of its bindings, which are listed on the
            // `workload-info` the hook is handed.
            caller: WorkloadCaller {
                bind_generation: Some(info.generation.clone()),
                workload_id: Arc::clone(workload_id),
                component_id: None,
                binding: None,
            },
            redact_errors: info.has_secrets(),
            args: vec![info.relocate()],
            result_tys: Arc::clone(&lifecycle.bind.result_tys),
        })
        .collect()
}

/// Attribute the just-observed store fault to a replayed bind, if the faulted
/// incarnation was mid-replay. Returns the culprit workload when the fault
/// happened during replay and the serve loop had marked one in-flight — that is
/// a per-workload poison (its bind crash-loops the shared store), which the
/// supervisor strikes and eventually evicts, and which must NOT consume the
/// plugin-wide restart budget. A fault while serving (replay already completed)
/// returns `None`: it is ordinary instability and charges the budget as before.
///
/// The marker lives in the faulted incarnation's registry, still reachable via
/// `state.registry` until the supervisor publishes the next one — this runs in
/// that window, right after the driver returns.
pub(super) fn attribute_replay_fault(state: &ComponentHostPluginState) -> Option<Arc<str>> {
    let progress = state.registry()?.replay_progress();
    if progress.completed {
        return None;
    }
    let workload_id = progress.current?.workload_id;
    state
        .bind_trap_log
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(Arc::clone(&workload_id));
    Some(workload_id)
}

/// Evict a workload whose `on-workload-bind` crash-looped past the strike
/// ceiling: drop it from the bound set (so the next incarnation stops replaying
/// it and recovers for every other tenant) and forget its strikes. The
/// workload is left provisioned nowhere; the caller reports it failed to the
/// host so its scheduling health reflects the eviction.
pub(super) fn evict_workload(state: &ComponentHostPluginState, workload_id: &Arc<str>) {
    state
        .bound
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(workload_id);
    state
        .poison
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(workload_id);
    error!(
        id = state.id,
        %workload_id,
        strikes = POISON_EVICT_STRIKES,
        "evicting workload after repeated on-workload-bind traps; reporting it failed"
    );
}

/// Report an evicted workload to the host so its scheduling health becomes
/// failed. A no-op if no failure sink was injected (the plugin is running
/// without a host, e.g. driven directly in a test).
pub(super) fn report_workload_failed(state: &ComponentHostPluginState, workload_id: &Arc<str>) {
    if let Some(sink) = state.failure_sink.load_full() {
        sink.report(
            workload_id.to_string(),
            format!(
                "host component plugin '{}' evicted the workload: its on-workload-bind \
                 repeatedly trapped ({POISON_EVICT_STRIKES} strikes)",
                state.id
            ),
        );
    }
}

/// The reply receiver for one in-flight lifecycle call.
pub(super) type LifecycleReplyRx = tokio::sync::oneshot::Receiver<wasmtime::Result<Vec<Relocated>>>;

/// The result of awaiting an `on-workload-bind` reply.
pub(super) enum BindReply {
    /// The hook returned (its relocated results, or a delivery/drop error).
    Completed(anyhow::Result<Vec<Relocated>>),
    /// The reply did not arrive within the budget; the hook may still be
    /// running. Its receiver is handed back so cleanup can be deferred until it
    /// resolves (a guest hook cannot be cancelled).
    TimedOut(LifecycleReplyRx),
}

/// Enqueue one lifecycle call on `sender` and return its pending reply
/// receiver, paired with the [`DispatchedCall`] whose deadline the caller must
/// enforce when awaiting it. Bounded by
/// [`crate::timeouts::plugin_lifecycle_call`] so a full channel cannot park a
/// deploy or a stop; a send that times out (or hits a closed channel) leaves
/// the job un-enqueued — tokio's bounded send only enqueues on completion — so
/// the caller knows no hook is running. The call runs under the workload's
/// identity (empty component id) so `wasmcloud:host/identity` answers
/// consistently inside the hook.
pub(super) async fn send_lifecycle_job(
    sender: &CapabilitySender,
    state: &ComponentHostPluginState,
    lifecycle: &LifecycleFuncs,
    func: &ExportedFunc,
    arg: Relocated,
    workload_id: &Arc<str>,
    generation: Option<BindGeneration>,
) -> anyhow::Result<(LifecycleReplyRx, DispatchedCall)> {
    // Channel backpressure must not consume the bind's secret-access budget.
    let permit = tokio::time::timeout(state.lifecycle_timeout(), sender.reserve())
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "host component plugin '{}' channel send timed out",
                state.id
            )
        })?
        .map_err(|_| anyhow::anyhow!("host component plugin '{}' channel closed", state.id))?;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let call = DispatchedCall::new("workload-lifecycle (plugin)", state.lifecycle_timeout());
    let job = CapabilityJob::Call(CapabilityCall {
        interface: Arc::clone(&lifecycle.interface),
        func: Arc::clone(&func.name),
        caller: CallerIdentity::Workload(WorkloadCaller {
            bind_generation: generation
                .map(|generation| generation.for_call(state.lifecycle_timeout())),
            workload_id: Arc::clone(workload_id),
            component_id: None,
            binding: None,
        }),
        args: vec![arg],
        result_tys: Arc::clone(&func.result_tys),
        reply: reply_tx,
        abandoned: call.flag(),
    });
    permit.send(job);
    Ok((reply_rx, call))
}

/// Interpret a resolved lifecycle reply receiver value into relocated results
/// or a diagnostic error.
fn interpret_reply(
    reply: Result<wasmtime::Result<Vec<Relocated>>, tokio::sync::oneshot::error::RecvError>,
    state: &ComponentHostPluginState,
) -> anyhow::Result<Vec<Relocated>> {
    match reply {
        Ok(Ok(results)) => Ok(results),
        Ok(Err(e)) => Err(anyhow::Error::from(e)),
        Err(_) => Err(anyhow::anyhow!(
            "host component plugin '{}' dropped the lifecycle reply",
            state.id
        )),
    }
}

/// Await a bind reply, keeping the receiver if the budget elapses first so the
/// caller can defer cleanup until the still-running hook returns. A timeout
/// also abandons the call (the [`DispatchedCall`] arms its flag), so a hook
/// wedged in non-yielding guest code is at least detected and logged.
pub(super) async fn await_bind_reply(
    mut reply_rx: LifecycleReplyRx,
    call: DispatchedCall,
    state: &ComponentHostPluginState,
    generation: &BindGeneration,
) -> BindReply {
    match call.await_head(&mut reply_rx).await {
        Some((reply, watch)) => {
            watch.disarm();
            BindReply::Completed(interpret_reply(reply, state))
        }
        None => {
            generation.revoke();
            BindReply::TimedOut(reply_rx)
        }
    }
}

/// Deliver one lifecycle call over `sender` and await its relocated results,
/// bounded by [`crate::timeouts::plugin_lifecycle_call`] on both the send and
/// the reply. Used for unbind (and item-bind rollback) where — unlike bind —
/// there is nothing to defer on a timeout.
async fn call_lifecycle(
    sender: &CapabilitySender,
    state: &ComponentHostPluginState,
    lifecycle: &LifecycleFuncs,
    func: &ExportedFunc,
    arg: Relocated,
    workload_id: &Arc<str>,
) -> anyhow::Result<Vec<Relocated>> {
    let (reply_rx, call) =
        send_lifecycle_job(sender, state, lifecycle, func, arg, workload_id, None).await?;
    match call.await_reply(reply_rx).await {
        Some(reply) => interpret_reply(reply, state),
        None => Err(anyhow::anyhow!(
            "lifecycle call to host component plugin '{}' timed out",
            state.id
        )),
    }
}

pub(super) struct BindLease {
    state: Arc<ComponentHostPluginState>,
    workload_id: Arc<str>,
}

impl BindLease {
    pub(super) fn acquire(
        state: &Arc<ComponentHostPluginState>,
        workload_id: &Arc<str>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            state
                .binding
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(Arc::clone(workload_id)),
            "workload `{workload_id}` still has a bind or cleanup in progress"
        );
        Ok(Self {
            state: Arc::clone(state),
            workload_id: Arc::clone(workload_id),
        })
    }
}

impl Drop for BindLease {
    fn drop(&mut self) {
        self.state
            .binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.workload_id);
    }
}

/// Wait for the old bind, then clean up the current incarnation. The lease
/// prevents a rebind until cleanup returns, including across store restarts.
pub(super) fn spawn_deferred_unbind(
    reply_rx: LifecycleReplyRx,
    workload_id: Arc<str>,
    lifecycle: Arc<LifecycleFuncs>,
    state: Arc<ComponentHostPluginState>,
    lease: BindLease,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _lease = lease;
        // Wait for the runaway hook to finish (or its reply to be dropped by a
        // store teardown).
        let _ = reply_rx.await;
        match state.sender_arc() {
            Some(current) => {
                let cleanup = send_lifecycle_job(
                    &current,
                    &state,
                    &lifecycle,
                    &lifecycle.unbind,
                    Relocated::Val(Val::String(workload_id.to_string())),
                    &workload_id,
                    None,
                )
                .await;
                match cleanup {
                    Ok((reply, call)) => {
                        let _timer = call.arm_on_timer();
                        let _ = reply.await;
                    }
                    Err(e) => {
                        warn!(id = state.id, %workload_id, err = %e, "deferred unbind not delivered")
                    }
                }
            }
            None => {
                debug!(
                    id = state.id,
                    %workload_id,
                    "skipping deferred unbind; the plugin is not running"
                );
            }
        }
    })
}

/// A `workload-info` for `on-workload-bind`, kept to deliver again on every
/// replay, and the secrets `wasmcloud:host/binding-secrets` hands out while
/// it runs.
#[derive(Clone)]
pub(super) struct BindInfo {
    pub(super) generation: BindGeneration,
    pub(super) owner: Arc<crate::engine::workload::WorkloadOwner>,
    info: Val,
    secrets: Vec<BindingSecrets>,
}

/// The secrets of one `interface-binding`, which its `config` leaves out;
/// empty for a binding that holds none.
#[derive(Clone)]
struct BindingSecrets {
    namespace: String,
    package: String,
    version: Option<semver::Version>,
    name: Option<String>,
    secrets: Vec<(String, SecretValue)>,
}

impl BindInfo {
    pub(super) fn has_secrets(&self) -> bool {
        self.secrets
            .iter()
            .any(|binding| !binding.secrets.is_empty())
    }

    /// A guest's rejection text, withheld whole when the bind carried
    /// secrets: the guest may quote one in a form `scrub` cannot match.
    pub(super) fn rejection_message(&self, message: String) -> String {
        if self.has_secrets() {
            "reason withheld because the bind carried secrets".to_string()
        } else {
            message
        }
    }

    /// `e` with each secret this bind can hand out replaced by `<redacted>`,
    /// for a failure the host reports itself, such as a trap.
    pub(super) fn scrub(&self, e: anyhow::Error) -> anyhow::Error {
        let mut values: Vec<&str> = self
            .secrets
            .iter()
            .flat_map(|binding| &binding.secrets)
            .filter_map(|(_, secret)| secret.expose())
            .flat_map(|v| [v, v.trim()])
            .filter(|v| !v.is_empty())
            .collect();
        let msg = format!("{e:#}");
        if !values.iter().any(|v| msg.contains(v)) {
            return e;
        }
        // Longest first, so a value containing another is replaced whole.
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        anyhow::anyhow!(
            values
                .into_iter()
                .fold(msg, |msg, v| msg.replace(v, "<redacted>"))
        )
    }

    pub(super) fn relocate(&self) -> Relocated {
        Relocated::Val(self.info.clone())
    }

    /// The secrets of the binding of `namespace:package` labeled `name`, or
    /// `None` when the workload has no such binding.
    pub(super) fn secrets(
        &self,
        namespace: &str,
        package: &str,
        name: Option<&str>,
    ) -> Option<&[(String, SecretValue)]> {
        self.secrets
            .iter()
            .find(|b| b.namespace == namespace && b.package == package && b.name.as_deref() == name)
            .map(|b| b.secrets.as_slice())
    }
}

/// Build the `workload-info` delivered to `on-workload-bind`: the workload's
/// identity plus every interface instance it matched on this plugin, each with
/// its manifest config, ordered deterministically.
///
/// # Errors
///
/// A binding holds a secret and the plugin does not import
/// `wasmcloud:host/binding-secrets` to read it.
pub(super) fn bind_info(
    id: &str,
    workload: &UnresolvedWorkload,
    interfaces: &WitInterfaces<'_>,
    takes_secrets: bool,
) -> anyhow::Result<BindInfo> {
    let service = Val::Option(
        workload
            .service_id()
            .map(|id| Box::new(Val::String(id.to_string()))),
    );
    let components = Val::List(
        workload
            .component_ids()
            .into_iter()
            .map(|id| Val::String(id.to_string()))
            .collect(),
    );
    let mut matched: Vec<&WitInterface> = interfaces.iter().collect();
    matched.sort_by_key(|i| i.instance());
    let mut bindings = Vec::with_capacity(matched.len());
    let mut secrets = Vec::new();
    for iface in matched {
        let (binding, held) = binding_info(id, iface, takes_secrets)?;
        anyhow::ensure!(
            !secrets.iter().any(|previous: &BindingSecrets| {
                previous.namespace == held.namespace
                    && previous.package == held.package
                    && previous.name == held.name
                    && previous.version != held.version
                    && (!previous.secrets.is_empty() || !held.secrets.is_empty())
            }),
            "host component plugin '{id}' has ambiguous secret bindings for {}:{}; use distinct binding names across versions",
            held.namespace,
            held.package
        );
        bindings.push(binding);
        secrets.push(held);
    }
    let info = Val::Record(vec![
        ("id".to_string(), Val::String(workload.id().to_string())),
        ("name".to_string(), Val::String(workload.name().to_string())),
        (
            "namespace".to_string(),
            Val::String(workload.namespace().to_string()),
        ),
        ("service".to_string(), service),
        ("components".to_string(), components),
        ("interfaces".to_string(), Val::List(bindings)),
    ]);
    Ok(BindInfo {
        generation: BindGeneration::default(),
        owner: Arc::clone(&workload.owner),
        info,
        secrets,
    })
}

/// One matched [`WitInterface`] as a lifecycle `interface-binding`, and its
/// secrets.
fn binding_info(
    id: &str,
    iface: &WitInterface,
    takes_secrets: bool,
) -> anyhow::Result<(Val, BindingSecrets)> {
    let mut names: Vec<&String> = iface.interfaces.iter().collect();
    names.sort();
    let mut config = Vec::new();
    let mut secrets = Vec::new();
    for (key, value) in &iface.config {
        match value {
            ConfigValue::Text(text) => config.push(Val::Tuple(vec![
                Val::String(key.clone()),
                Val::String(text.clone()),
            ])),
            ConfigValue::Secret(secret) => secrets.push((key.clone(), secret.clone())),
        }
    }
    if !takes_secrets && let Some((key, _)) = secrets.first() {
        anyhow::bail!(
            "host component plugin '{id}' does not import wasmcloud:host/binding-secrets, so it \
             cannot read the secret in `{key}`"
        );
    }
    let binding = Val::Record(vec![
        (
            "namespace".to_string(),
            Val::String(iface.namespace.clone()),
        ),
        ("package".to_string(), Val::String(iface.package.clone())),
        (
            "interfaces".to_string(),
            Val::List(names.into_iter().map(|n| Val::String(n.clone())).collect()),
        ),
        (
            "version".to_string(),
            Val::Option(iface.version.as_ref().map(|v| Box::new(version_val(v)))),
        ),
        (
            "name".to_string(),
            Val::Option(
                iface
                    .name
                    .as_ref()
                    .map(|n| Box::new(Val::String(n.clone()))),
            ),
        ),
        // Stubbed until component-model external-id lands (wasmCloud#5393):
        // `WitInterface` grows an `external_id` there, and this becomes
        // `iface.external_id.as_ref().map(..)` like `name` above. The WIT field
        // exists now so that merge does not change the record shape again —
        // adding a field to a WIT record is breaking for compiled plugins.
        ("external-id".to_string(), Val::Option(None)),
        ("config".to_string(), Val::List(config)),
    ]);
    let secrets = BindingSecrets {
        namespace: iface.namespace.clone(),
        package: iface.package.clone(),
        version: iface.version.clone(),
        name: iface.name.clone(),
        secrets,
    };
    Ok((binding, secrets))
}

/// A [`semver::Version`] as a lifecycle `version` record.
fn version_val(v: &semver::Version) -> Val {
    let pre = (!v.pre.is_empty()).then(|| Box::new(Val::String(v.pre.as_str().to_string())));
    let build = (!v.build.is_empty()).then(|| Box::new(Val::String(v.build.as_str().to_string())));
    Val::Record(vec![
        ("major".to_string(), Val::U64(v.major)),
        ("minor".to_string(), Val::U64(v.minor)),
        ("patch".to_string(), Val::U64(v.patch)),
        ("pre".to_string(), Val::Option(pre)),
        ("build".to_string(), Val::Option(build)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_binding_versions_require_distinct_names() {
        let workload = UnresolvedWorkload::new(
            "test",
            "test",
            "test",
            None,
            std::iter::empty::<crate::engine::workload::WorkloadComponent>(),
            Vec::new(),
        );
        let first = iface();
        let mut second = iface();
        second.version = Some(semver::Version::new(0, 2, 0));
        let interfaces = std::collections::HashSet::from([first.clone(), second.clone()]);
        assert!(bind_info("kv", &workload, &WitInterfaces::new(&interfaces), true).is_err());
        second.name = Some("second".to_string());
        let interfaces = std::collections::HashSet::from([first, second]);
        assert!(bind_info("kv", &workload, &WitInterfaces::new(&interfaces), true).is_ok());
    }

    fn iface() -> WitInterface {
        let mut iface = WitInterface::from("acme:kv/store@0.1.0");
        iface.config.insert("url", "redis://cache");
        iface
            .config
            .insert("password", crate::plugin::SecretValue::new("hunter2"));
        iface
    }

    /// A secret is kept apart for `binding-secrets`; text stays in `config`.
    #[test]
    fn a_secret_is_kept_apart_from_text_config() {
        let (binding, secrets) = binding_info("kv", &iface(), true).unwrap();
        let Val::Record(fields) = binding else {
            panic!("an interface-binding is a record");
        };
        let config = fields
            .iter()
            .find(|(name, _)| name == "config")
            .map(|(_, v)| v.clone());
        assert_eq!(
            config,
            Some(Val::List(vec![Val::Tuple(vec![
                Val::String("url".to_string()),
                Val::String("redis://cache".to_string()),
            ])]))
        );
        let info = BindInfo {
            generation: BindGeneration::default(),
            owner: Arc::new(crate::engine::workload::WorkloadOwner::new(
                "test",
                crate::engine::workload::OwnerKind::Workload,
                "workload",
            )),
            info: Val::Bool(true),
            secrets: vec![secrets],
        };
        let held = info.secrets("acme", "kv", None).unwrap();
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].0, "password");
        assert_eq!(held[0].1.expose(), Some("hunter2"));
        assert!(info.secrets("acme", "kv", Some("cache")).is_none());
    }

    /// A plugin that cannot read a secret is refused one, never handed it as
    /// text.
    #[test]
    fn a_plugin_without_binding_secrets_is_refused_one() {
        let err = binding_info("kv", &iface(), false)
            .err()
            .expect("a secret needs binding-secrets")
            .to_string();
        assert!(err.contains("`password`"), "{err}");
        assert!(!err.contains("hunter2"), "{err}");

        let mut text_only = iface();
        text_only.config.remove("password");
        let (_, held) = binding_info("kv", &text_only, false).unwrap();
        assert!(held.secrets.is_empty());
    }
}

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
use crate::engine::ctx::CallerIdentity;
use crate::engine::store::relocate::Relocated;
use crate::engine::workload::UnresolvedWorkload;
use crate::host::trigger_service::{CapabilityCall, CapabilityJob, LifecycleReplay};
use crate::plugin::WitInterfaces;
use crate::wit::WitInterface;

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
        let was_bound = bound.remove(workload_id.as_ref()).is_some();
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
        Val::String(workload_id.to_string()),
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
    })
}

/// Snapshot the bound-workloads map into the replay list for a fresh
/// incarnation. MUST be taken while holding the `bound` lock, atomically with
/// publishing the incarnation's sender: that exclusion is what guarantees a
/// concurrent bind/unbind is either reflected in the snapshot (recorded before
/// the swap, its own send targeting the previous, now-dead channel) or
/// delivered through the new channel — never both, so no double delivery.
pub(super) fn replay_snapshot(
    bound: &BTreeMap<Arc<str>, Val>,
    lifecycle: Option<&LifecycleFuncs>,
) -> Vec<LifecycleReplay> {
    let Some(lifecycle) = lifecycle else {
        return Vec::new();
    };
    bound
        .iter()
        .map(|(workload_id, info)| LifecycleReplay {
            interface: Arc::clone(&lifecycle.interface),
            func: Arc::clone(&lifecycle.bind.name),
            // A replayed bind is about the workload, not any component of it
            // nor any one of its bindings, which are listed on the
            // `workload-info` the hook is handed.
            caller: CallerIdentity {
                workload_id: Arc::clone(workload_id),
                component_id: None,
                binding: None,
            },
            args: vec![Relocated::Val(info.clone())],
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
    arg: Val,
    workload_id: &Arc<str>,
) -> anyhow::Result<(LifecycleReplyRx, DispatchedCall)> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let call = DispatchedCall::new("workload-lifecycle (plugin)", state.lifecycle_timeout());
    let job = CapabilityJob::Call(CapabilityCall {
        interface: Arc::clone(&lifecycle.interface),
        func: Arc::clone(&func.name),
        caller: CallerIdentity {
            workload_id: Arc::clone(workload_id),
            component_id: None,
            binding: None,
        },
        args: vec![Relocated::Val(arg)],
        result_tys: Arc::clone(&func.result_tys),
        reply: reply_tx,
        abandoned: call.flag(),
    });
    tokio::time::timeout(state.lifecycle_timeout(), sender.send(job))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "host component plugin '{}' channel send timed out",
                state.id
            )
        })?
        .map_err(|_| anyhow::anyhow!("host component plugin '{}' channel closed", state.id))?;
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
) -> BindReply {
    match call.await_head(&mut reply_rx).await {
        Some((reply, watch)) => {
            watch.disarm();
            BindReply::Completed(interpret_reply(reply, state))
        }
        None => BindReply::TimedOut(reply_rx),
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
    arg: Val,
    workload_id: &Arc<str>,
) -> anyhow::Result<Vec<Relocated>> {
    let (reply_rx, call) =
        send_lifecycle_job(sender, state, lifecycle, func, arg, workload_id).await?;
    match call.await_reply(reply_rx).await {
        Some(reply) => interpret_reply(reply, state),
        None => Err(anyhow::anyhow!(
            "lifecycle call to host component plugin '{}' timed out",
            state.id
        )),
    }
}

/// Reclaim whatever a timed-out `on-workload-bind` provisions, once it actually
/// returns. The deploy has already failed and the workload is out of the bound
/// map; this waits for the runaway hook's reply (guaranteeing bind-then-unbind
/// order), then delivers the unbind only if the incarnation the bind ran on is
/// still live — pointer-comparing the captured sender against the current one.
/// If a restart intervened, the hook's state died with that store, so there is
/// nothing to reclaim and no unbind is sent.
pub(super) fn spawn_deferred_unbind(
    incarnation: Arc<CapabilitySender>,
    reply_rx: LifecycleReplyRx,
    workload_id: Arc<str>,
    lifecycle: Arc<LifecycleFuncs>,
    state: Arc<ComponentHostPluginState>,
) {
    tokio::spawn(async move {
        // Wait for the runaway hook to finish (or its reply to be dropped by a
        // store teardown).
        let _ = reply_rx.await;
        match state.sender_arc() {
            Some(current) if Arc::ptr_eq(&current, &incarnation) => {
                if let Err(e) = call_lifecycle(
                    &incarnation,
                    &state,
                    &lifecycle,
                    &lifecycle.unbind,
                    Val::String(workload_id.to_string()),
                    &workload_id,
                )
                .await
                {
                    warn!(id = state.id, %workload_id, err = %e, "deferred unbind not delivered");
                }
            }
            _ => {
                debug!(
                    id = state.id,
                    %workload_id,
                    "skipping deferred unbind; the bind's incarnation is gone"
                );
            }
        }
    });
}

/// Build the `workload-info` record delivered to `on-workload-bind`: the
/// workload's identity plus every interface instance it matched on this plugin,
/// each with its manifest config — ordered deterministically.
pub(super) fn workload_info_val(
    workload: &UnresolvedWorkload,
    interfaces: &WitInterfaces<'_>,
) -> Val {
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
    let interfaces = Val::List(matched.into_iter().map(interface_binding_val).collect());
    Val::Record(vec![
        ("id".to_string(), Val::String(workload.id().to_string())),
        ("name".to_string(), Val::String(workload.name().to_string())),
        (
            "namespace".to_string(),
            Val::String(workload.namespace().to_string()),
        ),
        ("service".to_string(), service),
        ("components".to_string(), components),
        ("interfaces".to_string(), interfaces),
    ])
}

/// One matched [`WitInterface`] as a lifecycle `interface-binding` record.
fn interface_binding_val(iface: &WitInterface) -> Val {
    let mut names: Vec<&String> = iface.interfaces.iter().collect();
    names.sort();
    let mut config: Vec<(&String, &String)> = iface.config.iter().collect();
    config.sort();
    Val::Record(vec![
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
        (
            "config".to_string(),
            Val::List(
                config
                    .into_iter()
                    .map(|(k, v)| Val::Tuple(vec![Val::String(k.clone()), Val::String(v.clone())]))
                    .collect(),
            ),
        ),
    ])
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

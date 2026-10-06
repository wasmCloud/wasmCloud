//! Linked components that run beside their caller, each in a store of its own.
//!
//! A sync-typed import is served by a host function, and a host function cannot
//! call back into the store it was called from. So a linked component reached
//! by a sync call is not instantiated into its caller's store: it gets a
//! *companion* — a store holding that one instance, driven by its own task,
//! built on first use and dropped with the store that first reached it.
//!
//! The stores serving one root store form a [`LinkGroup`]. Every member reaches
//! the same companions, so a resource one member hands out is found again when
//! another member calls a method on it. A resource never leaves the store of
//! the component that defines it: every other member holds a proxy, and method
//! calls and drops are routed to the owner (see
//! [`crate::engine::store::resource_bridge`]).
//!
//! What cannot cross to a companion is a handle into the caller's own store: a
//! host resource such as a WASI stream, or an `error-context`. And a component
//! that calls back into one already blocked in a sync call to it waits until
//! that call's deadline.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use tokio::sync::{Notify, OnceCell, mpsc, oneshot};
use tokio_util::task::AbortOnDropHandle;
use tracing::{trace, warn};
use wasmtime::component::{
    Accessor, AccessorTask, ComponentExportIndex, Instance, Resource, Val, types::Type,
};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut, Store, StoreContextMut};

use crate::engine::abandon::{AbandonFlag, DispatchedCall, rearm_for_call, watch_until_abandoned};
use crate::engine::ctx::SharedCtx;
use crate::engine::instance_driver::InvocationSample;
use crate::engine::linked_call::{
    EphemeralLinkedCall, LinkedExportInvocation, Signature, extract_all, inject_results,
    linked_attributes, new_ephemeral_store_with,
};
use crate::engine::store::relocate::{self, Relocated};
use crate::engine::store::resource_bridge::{
    Owner, ProxyResource, ResourceRegistry, flush_drops, release_lent, take_lent,
};

/// A store's place in its [`LinkGroup`].
#[derive(Default)]
pub(crate) struct LinkState {
    group: GroupRef,
    /// The companions this store has already reached, so a call to one does
    /// not go back to the group for it.
    routes: BTreeMap<Arc<str>, Route>,
}

#[derive(Default)]
enum GroupRef {
    #[default]
    Unlinked,
    /// The store that first reached a companion; owns them all.
    Root(Arc<LinkGroup>),
    Member(Weak<LinkGroup>),
}

impl LinkState {
    fn for_member(group: &Arc<LinkGroup>) -> Self {
        Self {
            group: GroupRef::Member(Arc::downgrade(group)),
            routes: BTreeMap::new(),
        }
    }

    /// This store's group, founding one if it has not reached a companion yet.
    fn group(&mut self) -> wasmtime::Result<Arc<LinkGroup>> {
        match &self.group {
            GroupRef::Root(group) => Ok(Arc::clone(group)),
            GroupRef::Member(group) => group
                .upgrade()
                .context("the store this linked component serves is gone"),
            GroupRef::Unlinked => {
                let group = Arc::new(LinkGroup::default());
                self.group = GroupRef::Root(Arc::clone(&group));
                Ok(group)
            }
        }
    }

    fn joined(&self) -> Option<Arc<LinkGroup>> {
        match &self.group {
            GroupRef::Root(group) => Some(Arc::clone(group)),
            GroupRef::Member(group) => group.upgrade(),
            GroupRef::Unlinked => None,
        }
    }
}

/// The companions serving one root store, by component id.
///
/// Each has a cell of its own, so one being built does not hold up a call to
/// another — including a call its own start-up makes.
#[derive(Default)]
pub(crate) struct LinkGroup {
    members: Mutex<BTreeMap<Arc<str>, Arc<OnceCell<Member>>>>,
}

struct Member {
    route: Route,
    /// Dropping the group stops every companion and drops its store.
    _driver: AbortOnDropHandle<()>,
}

/// How to reach one companion, and what became of it if it has stopped.
#[derive(Clone)]
struct Route {
    component_id: Arc<str>,
    jobs: mpsc::UnboundedSender<Job>,
    fault: Arc<OnceLock<String>>,
    /// Weak, so a call in flight does not keep the group alive past its root.
    group: Weak<LinkGroup>,
}

impl Route {
    fn gone(&self) -> wasmtime::Error {
        match self.fault.get() {
            Some(fault) => wasmtime::format_err!(
                "linked component '{}' has stopped: {fault}",
                self.component_id
            ),
            None => wasmtime::format_err!(
                "linked component '{}' stopped before the call completed",
                self.component_id
            ),
        }
    }

    /// Release what `relocated` owns, for values that will never be injected.
    fn dispose(&self, relocated: Vec<Relocated>) {
        if let Some(group) = self.group.upgrade() {
            group.dispose(relocated);
        }
    }

    /// Queue a call. The returned future waits for its results.
    fn send(
        self,
        inv: &LinkedExportInvocation,
        args: Vec<Relocated>,
        result_tys: &Arc<[Type]>,
        attributes: Arc<[opentelemetry::KeyValue]>,
        deadline: Duration,
    ) -> impl Future<Output = wasmtime::Result<Vec<Relocated>>> + use<> {
        let dispatched = DispatchedCall::new("linked (companion store)", deadline);
        let (reply, reply_rx) = oneshot::channel();
        let job = Job::Call(Box::new(CallJob {
            func_idx: inv.func_idx,
            import_name: inv.import_name.clone(),
            export_name: inv.export_name.clone(),
            args,
            result_tys: Arc::clone(result_tys),
            attributes,
            abandoned: dispatched.flag(),
            reply,
        }));
        trace!(name = %inv.import_name, fn_name = %inv.export_name, "invoking companion export");
        let sent = self.jobs.send(job).map_err(|unsent| {
            if let Job::Call(job) = unsent.0 {
                self.dispose(job.args);
            }
        });
        let (import_name, export_name) = (inv.import_name.clone(), inv.export_name.clone());
        async move {
            sent.map_err(|()| self.gone())?;
            dispatched
                .await_reply(reply_rx)
                .await
                .ok_or_else(|| {
                    wasmtime::format_err!("{import_name}.{export_name} produced no result in time")
                })?
                .map_err(|_| self.gone())?
        }
    }
}

enum Job {
    Call(Box<CallJob>),
    /// The last proxy for one of this companion's resources was dropped.
    Drop(u64),
}

struct CallJob {
    func_idx: ComponentExportIndex,
    import_name: Arc<str>,
    export_name: Arc<str>,
    args: Vec<Relocated>,
    result_tys: Arc<[Type]>,
    attributes: Arc<[opentelemetry::KeyValue]>,
    abandoned: Arc<AbandonFlag>,
    reply: oneshot::Sender<wasmtime::Result<Vec<Relocated>>>,
}

/// The sync calls in flight on one companion. A resource destructor has to
/// enter the instance, which a sync call in progress forbids, so staged drops
/// wait for this to empty.
#[derive(Default)]
struct SyncCalls {
    in_flight: AtomicUsize,
    settled: Notify,
}

impl SyncCalls {
    fn enter(self: &Arc<Self>) -> SyncCall {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        SyncCall(Arc::clone(self))
    }

    fn idle(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst) == 0
    }
}

/// One sync call's place in [`SyncCalls`], given up however the call ends.
struct SyncCall(Arc<SyncCalls>);

impl Drop for SyncCall {
    fn drop(&mut self) {
        if self.0.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.settled.notify_one();
        }
    }
}

impl LinkGroup {
    fn route(&self, component_id: &str) -> Option<Route> {
        let members = self.members.lock().unwrap_or_else(|e| e.into_inner());
        let member = members.get(component_id)?.get()?;
        Some(member.route.clone())
    }

    /// The route to `callee`'s companion, building the companion first if this
    /// group has not reached that component yet.
    async fn companion(self: &Arc<Self>, callee: &EphemeralLinkedCall) -> wasmtime::Result<Route> {
        let cell = {
            let mut members = self.members.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(
                members
                    .entry(Arc::clone(&callee.active_component_id))
                    .or_default(),
            )
        };
        let member = cell.get_or_try_init(|| self.build(callee)).await?;
        Ok(member.route.clone())
    }

    async fn build(self: &Arc<Self>, callee: &EphemeralLinkedCall) -> wasmtime::Result<Member> {
        let component_id = Arc::clone(&callee.active_component_id);
        // Set before anything is instantiated, so a sync call made while the
        // store is being built already reaches this group.
        let mut store = new_ephemeral_store_with(callee, |ctx| {
            ctx.links = LinkState::for_member(self);
            ctx.resource_registry = Some(ResourceRegistry::new(Owner::Component(Arc::clone(
                &component_id,
            ))));
        })
        .await
        .map_err(|e| wasmtime::format_err!("linked component store creation failed: {e:#}"))?;
        let instance = callee.pre.instantiate_async(&mut store).await?;

        let (jobs, queue) = mpsc::unbounded_channel();
        let route = Route {
            component_id: Arc::clone(&component_id),
            jobs,
            fault: Arc::default(),
            group: Arc::downgrade(self),
        };
        let driver = Driver {
            component_id,
            instance,
            queue,
            route: route.clone(),
            stop: Arc::default(),
            sync_calls: Arc::default(),
            drops_staged: false,
        };
        Ok(Member {
            route,
            _driver: AbortOnDropHandle::new(tokio::spawn(driver.run(store))),
        })
    }

    /// Release what `relocated` owns, for values that will never be injected
    /// anywhere: a companion is told to drop each of its resources they
    /// carried. (A host component plugin's is not reachable from here, and is
    /// released when the plugin's store is.)
    fn dispose(&self, relocated: Vec<Relocated>) {
        for value in relocated {
            self.dispose_one(value);
        }
    }

    fn dispose_one(&self, value: Relocated) {
        match value {
            Relocated::Resource {
                owner: Owner::Component(owner),
                proxy_id,
                owned: true,
            } => {
                if let Some(route) = self.route(&owner) {
                    let _ = route.jobs.send(Job::Drop(proxy_id));
                }
            }
            Relocated::List(values)
            | Relocated::FixedLengthList(values)
            | Relocated::Tuple(values) => self.dispose(values),
            Relocated::Record(fields) => {
                for (_, value) in fields {
                    self.dispose_one(value);
                }
            }
            Relocated::Variant(_, value) | Relocated::Option(value) => self.dispose_one(*value),
            Relocated::Result(Ok(value) | Err(value)) => self.dispose_one(*value),
            Relocated::Map(entries) => {
                for (key, value) in entries {
                    self.dispose_one(key);
                    self.dispose_one(value);
                }
            }
            // Dropping a pump's far end is what stops it.
            Relocated::Val(_)
            | Relocated::Stream(_)
            | Relocated::Future(_)
            | Relocated::Resource { .. } => {}
        }
    }
}

/// What ended one pass of a companion's event loop.
enum Served {
    /// Its group is gone; nothing will call it again.
    Closed,
    /// A resource was staged for dropping, which needs the store itself.
    Drops,
    /// A call failed in a way that leaves the guest's state unknown.
    Faulted,
}

struct Driver {
    component_id: Arc<str>,
    instance: Instance,
    queue: mpsc::UnboundedReceiver<Job>,
    /// This companion's own route: where a call records what stopped it, and
    /// how it reaches the group without keeping it alive.
    route: Route,
    /// Signalled by a call that faulted the companion, to end the loop.
    stop: Arc<Notify>,
    sync_calls: Arc<SyncCalls>,
    /// Whether a resource is staged for dropping and waiting on `sync_calls`.
    drops_staged: bool,
}

impl Driver {
    /// Serve the companion's calls until its group is dropped or it faults.
    ///
    /// One event loop serves every call, so calls overlap. It is left only to
    /// run resource destructors, which need the store itself, and re-entered
    /// with the calls in flight where they were.
    async fn run(mut self, mut store: Store<SharedCtx>) {
        loop {
            let served = store
                .run_concurrent(async |accessor| self.serve(accessor).await)
                .await;
            match served {
                Ok(Served::Closed) => return,
                Ok(Served::Faulted) => break,
                Ok(Served::Drops) => {
                    flush_drops(&mut store).await;
                    self.drops_staged = false;
                }
                Err(e) => {
                    let _ = self.route.fault.set(format!("{e:#}"));
                    break;
                }
            }
        }
        warn!(
            component_id = %self.component_id,
            fault = self.route.fault.get().map(String::as_str),
            "linked component stopped; calls to it fail from here on"
        );
    }

    async fn serve(&mut self, accessor: &Accessor<SharedCtx>) -> Served {
        loop {
            let job = tokio::select! {
                job = self.queue.recv() => match job {
                    Some(job) => job,
                    None => return Served::Closed,
                },
                () = self.stop.notified() => return Served::Faulted,
                () = self.sync_calls.settled.notified(), if self.drops_staged => {
                    if self.sync_calls.idle() {
                        return Served::Drops;
                    }
                    continue;
                }
            };
            match job {
                Job::Drop(id) => {
                    accessor.with(|mut access| {
                        if let Some(registry) = access.data_mut().resource_registry.as_mut() {
                            registry.stage_drop(id);
                        }
                    });
                    if self.sync_calls.idle() {
                        return Served::Drops;
                    }
                    self.drops_staged = true;
                }
                // Its caller has gone, and nothing can be cancelled once it
                // starts.
                Job::Call(job) if job.reply.is_closed() => self.route.dispose(job.args),
                Job::Call(job) => {
                    // The callee's type controls reentry, even if its importer is async.
                    let sync_call = accessor.with(|mut access| {
                        self.instance
                            .get_func(&mut access, job.func_idx)
                            .filter(|func| !func.ty(&access).async_())
                            .map(|_| self.sync_calls.enter())
                    });
                    let task = CallTask {
                        instance: self.instance,
                        sync_call,
                        job,
                        route: self.route.clone(),
                        stop: Arc::clone(&self.stop),
                    };
                    if let Err(e) = accessor.spawn(task) {
                        tracing::error!(err = %e, "failed to spawn linked call task");
                    }
                }
            }
        }
    }
}

/// Serves one call on a companion's instance.
struct CallTask {
    instance: Instance,
    /// Held for the life of the task when the callee is sync-typed.
    sync_call: Option<SyncCall>,
    job: Box<CallJob>,
    route: Route,
    stop: Arc<Notify>,
}

impl AccessorTask<SharedCtx> for CallTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        let CallJob {
            func_idx,
            import_name,
            export_name,
            args,
            result_tys,
            attributes,
            abandoned,
            reply,
        } = *self.job;
        let instance = self.instance;
        let _sync_call = self.sync_call;

        let prepared = accessor.with(|mut access| -> wasmtime::Result<_> {
            // The epoch deadline measures this call's own execution.
            rearm_for_call(&mut access);
            let func = instance.get_func(&mut access, func_idx).with_context(|| {
                format!("function not found for linked import {import_name}.{export_name}")
            })?;
            let mut vals = Vec::with_capacity(args.len());
            for arg in args {
                vals.push(relocate::inject(access.as_context_mut(), arg)?);
            }
            Ok((func, vals, take_lent(access.data_mut())))
        });
        let (func, args, lent) = match prepared {
            Ok(prepared) => prepared,
            Err(e) => {
                let _ = reply.send(Err(e));
                return Ok(());
            }
        };
        let (calls, executed) = accessor.with(|mut access| {
            let data = access.get();
            (Arc::clone(&data.abandoned), Arc::clone(&data.executed))
        });
        let _sample = InvocationSample::start(&executed, attributes);

        let mut results = vec![Val::Bool(false); result_tys.len()];
        let outcome = watch_until_abandoned(
            &calls,
            abandoned,
            func.call_concurrent(accessor, &args, &mut results),
        )
        .await
        .and_then(|()| {
            accessor.with(|mut access| {
                release_lent(access.as_context_mut(), lent)?;
                // Result pumps run under this store, which outlives the call.
                extract_all(
                    access.as_context_mut(),
                    &results,
                    &result_tys,
                    &mut Vec::new(),
                )
            })
        });
        match outcome {
            Ok(relocated) => {
                if let Err(Ok(undelivered)) = reply.send(Ok(relocated)) {
                    self.route.dispose(undelivered);
                }
            }
            // The guest's state is unknown past a failed call, and a guest
            // call cannot be cancelled from the host: the companion ends here.
            Err(e) => {
                let _ = self
                    .route
                    .fault
                    .set(format!("{import_name}.{export_name} failed: {e:#}"));
                let _ = reply.send(Err(e));
                self.stop.notify_one();
            }
        }
        Ok(())
    }
}

/// The route to `callee`'s companion from a store whose links are `links`,
/// when that store has already reached it.
fn known_route(links: &LinkState, callee: &EphemeralLinkedCall) -> Option<Route> {
    links.routes.get(&callee.active_component_id).cloned()
}

/// Serve a sync-typed import from its companion.
///
/// The caller's store is blocked for the whole call, so nothing in it can feed
/// a `stream` or `future` argument; the linker refuses such a signature.
pub(crate) async fn invoke_sync(
    mut store: StoreContextMut<'_, SharedCtx>,
    params: &[Val],
    results: &mut [Val],
    inv: &LinkedExportInvocation,
    callee: &EphemeralLinkedCall,
    signature: &Signature,
) -> wasmtime::Result<()> {
    let deadline = crate::timeouts::shared_store_call();
    let route = match known_route(&store.data().links, callee) {
        Some(route) => route,
        None => {
            let group = store.data_mut().links.group()?;
            let route = tokio::time::timeout(deadline, group.companion(callee))
                .await
                .map_err(|_| {
                    wasmtime::format_err!(
                        "linked component '{}' was not ready in time",
                        callee.active_component_id
                    )
                })??;
            let links = &mut store.data_mut().links;
            links
                .routes
                .insert(Arc::clone(&callee.active_component_id), route.clone());
            route
        }
    };
    let attributes = linked_attributes(callee, inv).await;
    // Everything awaited is behind us: the arguments leave this store and are
    // queued without a point between at which the call could be dropped.
    let args = extract_all(
        store.as_context_mut(),
        params,
        &signature.params,
        &mut Vec::new(),
    )?;
    let relocated = route
        .send(inv, args, &signature.results, attributes, deadline)
        .await?;
    inject_results(store.as_context_mut(), relocated, results)
}

/// Serve an async-typed import from its companion.
///
/// It carries no deadline of its own, as it had none while the callee shared
/// the caller's store: the caller's own bounds it.
pub(crate) async fn invoke_async(
    accessor: &Accessor<SharedCtx>,
    params: &[Val],
    results: &mut [Val],
    inv: &LinkedExportInvocation,
    callee: &EphemeralLinkedCall,
    signature: &Signature,
) -> wasmtime::Result<()> {
    let known = accessor.with(|mut access| known_route(&access.get().links, callee));
    let route = match known {
        Some(route) => route,
        None => {
            let group = accessor.with(|mut access| access.data_mut().links.group())?;
            let route = group.companion(callee).await?;
            accessor.with(|mut access| {
                let links = &mut access.data_mut().links;
                links
                    .routes
                    .insert(Arc::clone(&callee.active_component_id), route.clone());
            });
            route
        }
    };
    let attributes = linked_attributes(callee, inv).await;
    // Argument pumps run under this store, which outlives the call.
    let args = accessor.with(|mut access| {
        extract_all(
            access.as_context_mut(),
            params,
            &signature.params,
            &mut Vec::new(),
        )
    })?;
    let relocated = route
        .send(inv, args, &signature.results, attributes, Duration::MAX)
        .await?;
    accessor.with(|mut access| inject_results(access.as_context_mut(), relocated, results))
}

/// The destructor of a proxy for a linked component's resource: tells the
/// owner its resource is done.
pub(crate) fn drop_proxy(
    mut store: StoreContextMut<'_, SharedCtx>,
    rep: u32,
) -> wasmtime::Result<()> {
    let proxy = store
        .data_mut()
        .table
        .delete(Resource::<ProxyResource>::new_own(rep))?;
    let Owner::Component(owner) = proxy.owner else {
        return Ok(());
    };
    // No companion to tell means its store, and the resource with it, is gone.
    if let Some(route) = store
        .data()
        .links
        .joined()
        .and_then(|group| group.route(&owner))
    {
        let _ = route.jobs.send(Job::Drop(proxy.proxy_id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ctx::Ctx;
    use wasmtime::component::{Component, Linker};

    #[tokio::test]
    async fn sync_callee_defers_drops_until_it_returns() -> anyhow::Result<()> {
        let mut config = wasmtime::Config::new();
        config.wasm_component_model_async(true);
        let engine = wasmtime::Engine::new(&config)?;
        let component = Component::new(
            &engine,
            wat::parse_str(
                r#"(component
                    (import "wait" (func $wait))
                    (core func $wait (canon lower (func $wait)))
                    (core module $m
                        (import "" "wait" (func $wait))
                        (func (export "run") call $wait))
                    (core instance $i (instantiate $m
                        (with "" (instance (export "wait" (func $wait))))))
                    (func (export "run") (canon lift (core func $i "run"))))"#,
            )?,
        )?;
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let mut linker = Linker::<SharedCtx>::new(&engine);
        linker.root().func_new_async("wait", {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            move |_, _, _, _| {
                let entered = Arc::clone(&entered);
                let release = Arc::clone(&release);
                Box::new(async move {
                    entered.notify_one();
                    release.notified().await;
                    Ok(())
                })
            }
        })?;
        let mut store = Store::new(
            &engine,
            SharedCtx::new(Ctx::builder("test", "callee").build()),
        );
        let instance = linker.instantiate_async(&mut store, &component).await?;
        let (_, func_idx) = component.get_export(None, "run").unwrap();
        let (jobs, queue) = mpsc::unbounded_channel();
        let (reply, reply_rx) = oneshot::channel();
        let dispatched = DispatchedCall::new("test", Duration::from_secs(5));
        let sync_calls = Arc::new(SyncCalls::default());
        let driver = Driver {
            component_id: Arc::from("callee"),
            instance,
            queue,
            sync_calls: Arc::clone(&sync_calls),
            drops_staged: false,
            route: Route {
                component_id: Arc::from("callee"),
                jobs: jobs.clone(),
                fault: Arc::default(),
                group: Weak::new(),
            },
            stop: Arc::default(),
        };
        let _driver = AbortOnDropHandle::new(tokio::spawn(driver.run(store)));
        assert!(
            jobs.send(Job::Call(Box::new(CallJob {
                func_idx,
                import_name: Arc::from("test"),
                export_name: Arc::from("run"),
                args: Vec::new(),
                result_tys: Arc::from([]),
                attributes: Arc::from([]),
                abandoned: dispatched.flag(),
                reply,
            })))
            .is_ok()
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            entered.notified().await;
            assert!(!sync_calls.idle());
            release.notify_one();
            reply_rx.await??;
            anyhow::Ok(())
        })
        .await??;
        assert!(sync_calls.idle());
        Ok(())
    }
}

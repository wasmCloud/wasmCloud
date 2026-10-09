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
use tracing::{Instrument as _, Span, trace, warn};
use wasmtime::component::{
    Accessor, AccessorTask, ComponentExportIndex, Instance, Resource, Val, types::Type,
};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut, Store, StoreContextMut};

use crate::engine::abandon::{
    AbandonFlag, AbandonedCallPolicy, DispatchedCall, rearm_for_call, watch_until_abandoned,
};
use crate::engine::ctx::SharedCtx;
use crate::engine::instance_driver::InvocationSample;
use crate::engine::linked_call::{
    EphemeralLinkedCall, LinkedExportInvocation, Signature, extract_all, linked_attributes,
    new_ephemeral_store_with, write_results,
};
use crate::engine::store::relocate::{self, Relocated};
use crate::engine::store::resource_bridge::{Owner, ProxyResource, ResourceRegistry, flush_drops};

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

/// What a companion's own tasks and its callers both know it by. It holds no
/// way to queue work, so the companion's queue closes once its callers are
/// gone.
#[derive(Clone)]
struct Home {
    component_id: Arc<str>,
    /// What stopped the companion, once something has.
    fault: Arc<OnceLock<String>>,
    /// Weak, so a call in flight does not keep the group alive past its root.
    group: Weak<LinkGroup>,
}

impl Home {
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

    fn pack(&self, values: Vec<Relocated>) -> Parcel {
        Parcel {
            values,
            group: Weak::clone(&self.group),
        }
    }
}

/// Relocated values on their way from one store to another.
///
/// Dropped unopened — the call was cancelled, or the store they were bound for
/// stopped — it has the owner of each resource they carried drop it, so
/// nothing in transit is stranded in a registry.
struct Parcel {
    values: Vec<Relocated>,
    group: Weak<LinkGroup>,
}

impl Parcel {
    fn open(mut self) -> Vec<Relocated> {
        std::mem::take(&mut self.values)
    }
}

impl Drop for Parcel {
    fn drop(&mut self) {
        if !self.values.is_empty()
            && let Some(group) = self.group.upgrade()
        {
            group.dispose(std::mem::take(&mut self.values));
        }
    }
}

/// How to reach one companion.
#[derive(Clone)]
struct Route {
    home: Home,
    jobs: mpsc::UnboundedSender<Job>,
}

impl Route {
    /// Queue a call. The returned future waits for its results.
    fn send(
        self,
        inv: &LinkedExportInvocation,
        args: Vec<Relocated>,
        result_tys: &Arc<[Type]>,
        attributes: Arc<[opentelemetry::KeyValue]>,
        deadline: Duration,
    ) -> impl Future<Output = wasmtime::Result<Parcel>> + use<> {
        let dispatched = DispatchedCall::new("linked (companion store)", deadline);
        let (reply, reply_rx) = oneshot::channel();
        let job = Job::Call(Box::new(CallJob {
            func_idx: inv.func_idx,
            import_name: inv.import_name.clone(),
            export_name: inv.export_name.clone(),
            args: self.home.pack(args),
            result_tys: Arc::clone(result_tys),
            attributes,
            abandoned: dispatched.flag(),
            span: Span::current(),
            reply,
        }));
        trace!(name = %inv.import_name, fn_name = %inv.export_name, "invoking companion export");
        // A job that cannot be queued is dropped here, arguments and all.
        let sent = self.jobs.send(job).is_ok();
        let (import_name, export_name) = (inv.import_name.clone(), inv.export_name.clone());
        let home = self.home;
        async move {
            if !sent {
                return Err(home.gone());
            }
            dispatched
                .await_reply(reply_rx)
                .await
                .ok_or_else(|| {
                    wasmtime::format_err!("{import_name}.{export_name} produced no result in time")
                })?
                .map_err(|_| home.gone())?
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
    args: Parcel,
    result_tys: Arc<[Type]>,
    attributes: Arc<[opentelemetry::KeyValue]>,
    abandoned: Arc<AbandonFlag>,
    /// The caller's span, which the call runs under in the companion's task.
    span: Span,
    reply: oneshot::Sender<wasmtime::Result<Parcel>>,
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
        // A companion is never rebuilt, and everything its group has handed
        // out goes with it, so an abandoned call gets the same patience a
        // plugin's does before the store is trapped under it.
        let policy = AbandonedCallPolicy::WarnThenTrap;
        // Set before anything is instantiated, so a sync call made while the
        // store is being built already reaches this group.
        let mut store = new_ephemeral_store_with(callee, policy, |ctx| {
            ctx.links = LinkState::for_member(self);
            ctx.resource_registry = Some(ResourceRegistry::new(Owner::Component(Arc::clone(
                &component_id,
            ))));
        })
        .await
        .map_err(|e| wasmtime::format_err!("linked component store creation failed: {e:#}"))?;
        let instance = callee.pre.instantiate_async(&mut store).await?;
        let late_drops = store
            .data()
            .resource_registry
            .as_ref()
            .map(ResourceRegistry::late_drops)
            .unwrap_or_default();

        let (jobs, queue) = mpsc::unbounded_channel();
        let home = Home {
            component_id,
            fault: Arc::default(),
            group: Arc::downgrade(self),
        };
        let driver = Driver {
            home: home.clone(),
            instance,
            queue,
            stop: Arc::default(),
            late_drops,
            sync_calls: Arc::default(),
            drops_staged: false,
        };
        Ok(Member {
            route: Route { home, jobs },
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
    /// Every route to it is gone; nothing will call it again.
    Closed,
    /// Resources are staged for dropping, which needs the store itself.
    Drops,
    /// A call failed in a way that leaves the guest's state unknown.
    Faulted,
}

struct Driver {
    home: Home,
    instance: Instance,
    queue: mpsc::UnboundedReceiver<Job>,
    /// Signalled by a call that faulted the companion, to end the loop.
    stop: Arc<Notify>,
    /// Signalled when a call ending staged a drop; see
    /// [`ResourceRegistry::late_drops`].
    late_drops: Arc<Notify>,
    sync_calls: Arc<SyncCalls>,
    /// Whether a resource is staged for dropping.
    drops_staged: bool,
}

impl Driver {
    /// Serve the companion's calls until nothing can reach it or it faults.
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
                    let _ = self.home.fault.set(format!("{e:#}"));
                    break;
                }
            }
        }
        warn!(
            component_id = %self.home.component_id,
            fault = self.home.fault.get().map(String::as_str),
            "linked component stopped; calls to it fail from here on"
        );
    }

    async fn serve(&mut self, accessor: &Accessor<SharedCtx>) -> Served {
        loop {
            if self.drops_staged && self.sync_calls.idle() {
                return Served::Drops;
            }
            let job = tokio::select! {
                biased;
                () = self.stop.notified() => return Served::Faulted,
                () = self.late_drops.notified() => {
                    self.drops_staged = true;
                    continue;
                }
                () = self.sync_calls.settled.notified(), if self.drops_staged => continue,
                // Nothing new is taken while a drop waits on the sync calls in
                // flight, or a steady run of them would hold it back for good.
                job = self.queue.recv(), if !self.drops_staged => match job {
                    Some(job) => job,
                    None => return Served::Closed,
                },
            };
            self.admit(accessor, job);
            // Whatever else is already queued goes with it, so a run of drops
            // costs one trip out of the event loop.
            while let Ok(job) = self.queue.try_recv() {
                self.admit(accessor, job);
            }
        }
    }

    fn admit(&mut self, accessor: &Accessor<SharedCtx>, job: Job) {
        match job {
            Job::Drop(id) => {
                self.drops_staged |= accessor.with(|mut access| {
                    let registry = access.data_mut().resource_registry.as_mut();
                    registry.is_some_and(|registry| registry.stage_drop(id))
                });
            }
            // Its caller has gone, and nothing can be cancelled once it starts.
            Job::Call(job) if job.reply.is_closed() => {}
            Job::Call(job) => {
                let task = CallTask {
                    instance: self.instance,
                    job,
                    home: self.home.clone(),
                    stop: Arc::clone(&self.stop),
                    sync_calls: Arc::clone(&self.sync_calls),
                };
                if let Err(e) = accessor.spawn(task) {
                    tracing::error!(err = %e, "failed to spawn linked call task");
                }
            }
        }
    }
}

/// Serves one call on a companion's instance.
struct CallTask {
    instance: Instance,
    job: Box<CallJob>,
    home: Home,
    stop: Arc<Notify>,
    sync_calls: Arc<SyncCalls>,
}

impl AccessorTask<SharedCtx> for CallTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        let span = self.job.span.clone();
        self.call(accessor).instrument(span).await;
        Ok(())
    }
}

impl CallTask {
    async fn call(self, accessor: &Accessor<SharedCtx>) {
        let CallJob {
            func_idx,
            import_name,
            export_name,
            args,
            result_tys,
            attributes,
            abandoned,
            span: _,
            reply,
        } = *self.job;
        let instance = self.instance;
        let home = self.home;

        let prepared = accessor.with(|mut access| -> wasmtime::Result<_> {
            // The epoch deadline measures this call's own execution.
            rearm_for_call(&mut access);
            let func = instance.get_func(&mut access, func_idx).with_context(|| {
                format!("function not found for linked import {import_name}.{export_name}")
            })?;
            let sync = !func.ty(&access).async_();
            match relocate::inject_all(access.as_context_mut(), args.open()) {
                Ok((args, lent)) => Ok((func, sync, args, lent)),
                Err(failed) => {
                    drop(home.pack(failed.stranded));
                    Err(failed.error)
                }
            }
        });
        let (func, sync, args, lent) = match prepared {
            Ok(prepared) => prepared,
            Err(e) => {
                let _ = reply.send(Err(e));
                return;
            }
        };
        // A sync function's instance cannot be entered again until it returns.
        let _sync_call = sync.then(|| self.sync_calls.enter());
        let (calls, executed) = accessor.with(|mut access| {
            let data = access.data_mut();
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
                relocate::finish_call(access.as_context_mut(), lent, &results, &result_tys)
            })
        });
        match outcome {
            Ok(relocated) => {
                // Undelivered, the parcel comes back and is dropped here.
                let _ = reply.send(Ok(home.pack(relocated)));
            }
            // The guest's state is unknown past a failed call, and a guest
            // call cannot be cancelled from the host: the companion ends here.
            Err(e) => {
                let _ = home
                    .fault
                    .set(format!("{import_name}.{export_name} failed: {e:#}"));
                let _ = reply.send(Err(e));
                self.stop.notify_one();
            }
        }
    }
}

/// The route to `callee`'s companion from a store whose links are `links`,
/// when that store has already reached it.
fn known_route(links: &LinkState, callee: &EphemeralLinkedCall) -> Option<Route> {
    links.routes.get(&callee.active_component_id).cloned()
}

/// Rebuild a call's results in the caller's store and write them into its
/// result slots. Results that cannot all be rebuilt are given back to their
/// owners.
fn deliver(
    store: StoreContextMut<'_, SharedCtx>,
    home: &Home,
    parcel: Parcel,
    results: &mut [Val],
) -> wasmtime::Result<()> {
    match relocate::inject_all(store, parcel.open()) {
        // A result is never a `borrow`, so nothing is lent.
        Ok((vals, _lent)) => write_results(vals, results),
        Err(failed) => {
            drop(home.pack(failed.stranded));
            Err(failed.error)
        }
    }
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
    let home = route.home.clone();
    // Everything awaited is behind us: the arguments leave this store and are
    // queued without a point between at which the call could be dropped.
    let args = extract_all(
        store.as_context_mut(),
        params,
        &signature.params,
        &mut Vec::new(),
    )?;
    let parcel = route
        .send(inv, args, &signature.results, attributes, deadline)
        .await?;
    deliver(store.as_context_mut(), &home, parcel, results)
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
    let known = accessor.with(|mut access| known_route(&access.data_mut().links, callee));
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
    let home = route.home.clone();
    // Argument pumps run under this store, which outlives the call.
    let args = accessor.with(|mut access| {
        extract_all(
            access.as_context_mut(),
            params,
            &signature.params,
            &mut Vec::new(),
        )
    })?;
    let parcel = route
        .send(inv, args, &signature.results, attributes, Duration::MAX)
        .await?;
    accessor.with(|mut access| deliver(access.as_context_mut(), &home, parcel, results))
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

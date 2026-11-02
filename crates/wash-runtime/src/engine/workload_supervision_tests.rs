use super::*;
use crate::engine::dispatch::{GuestCall, GuestCallFuture};
use crate::host::http::{HostHandler, IngressRoute, ServiceHttpJob};
use crate::host::trigger_service::MessagingJob;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc};

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug)]
enum Outcome {
    Success,
    Error,
    Trap,
}

#[derive(Clone, Copy, Debug)]
struct Ingresses {
    http: bool,
    messaging: bool,
    dispatch: bool,
}

const CONFIGURATIONS: [Ingresses; 7] = [
    Ingresses {
        http: true,
        messaging: false,
        dispatch: false,
    },
    Ingresses {
        http: false,
        messaging: true,
        dispatch: false,
    },
    Ingresses {
        http: false,
        messaging: false,
        dispatch: true,
    },
    Ingresses {
        http: true,
        messaging: true,
        dispatch: false,
    },
    Ingresses {
        http: true,
        messaging: false,
        dispatch: true,
    },
    Ingresses {
        http: false,
        messaging: true,
        dispatch: true,
    },
    Ingresses {
        http: true,
        messaging: true,
        dispatch: true,
    },
];

fn engine() -> &'static wasmtime::Engine {
    static ENGINE: std::sync::OnceLock<wasmtime::Engine> = std::sync::OnceLock::new();
    ENGINE.get_or_init(|| {
        crate::init_crypto();
        let mut config = wasmtime::Config::new();
        config
            .wasm_component_model(true)
            .wasm_component_model_async(true)
            .wasm_component_model_error_context(true);
        wasmtime::Engine::new(&config).unwrap()
    })
}

#[derive(Debug)]
struct Run {
    store: String,
    service_count: u32,
    linked_count: u32,
}

struct Probe {
    runs: mpsc::UnboundedReceiver<Run>,
    proceed: Arc<Semaphore>,
    active: Arc<AtomicUsize>,
}

impl Probe {
    async fn next(&mut self) -> anyhow::Result<Run> {
        let run = tokio::time::timeout(TIMEOUT, self.runs.recv())
            .await?
            .context("service did not run")?;
        assert_eq!((run.service_count, run.linked_count), (1, 1));
        Ok(run)
    }

    fn resume(&self) {
        self.proceed.add_permits(1);
    }
}

struct ActiveProbe(Arc<AtomicUsize>);

impl Drop for ActiveProbe {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn workload(
    outcome: Outcome,
    budget: u64,
    ingress: Ingresses,
    host: &crate::host::HostRef,
) -> anyhow::Result<(ResolvedWorkload, Probe)> {
    let linked = WorkloadComponent::new(
        "supervision",
        "supervision",
        "test",
        "linked",
        Component::new(
            engine(),
            wat::parse_str(
                r#"(component
          (core module $m
            (global $count (mut i32) i32.const 0)
            (func (export "next") (result i32)
              global.get $count i32.const 1 i32.add global.set $count global.get $count))
          (core instance $m (instantiate $m))
          (func $next (result u32) (canon lift (core func $m "next")))
          (instance $linked (export "next" (func $next)))
          (export "test:supervision/linked@0.1.0" (instance $linked)))"#,
            )?,
        )?,
        Linker::new(engine()),
        vec![],
        LocalResources::default(),
        Arc::default(),
        InstancePolicy::Ephemeral,
    );
    let wat = include_str!("workload_supervision_fixture.wat")
        .replace(
            "HTTP_EXPORT",
            if ingress.http {
                r#"(export "wasi:http/handler@0.3.0" (instance $http))"#
            } else {
                ""
            },
        )
        .replace(
            "MESSAGING_EXPORT",
            if ingress.messaging {
                r#"(export "wasmcloud:messaging/handler@0.3.0" (instance $messaging))"#
            } else {
                ""
            },
        );
    let wat = wat.replace(
        "AFTER_PROBE",
        if matches!(outcome, Outcome::Trap) {
            "drop unreachable"
        } else {
            ""
        },
    );
    let mut linker: Linker<SharedCtx> = Linker::new(engine());
    wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
    let (tx, runs) = mpsc::unbounded_channel();
    let proceed = Arc::new(Semaphore::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    linker
        .instance("test:supervision/probe@0.1.0")?
        .func_wrap_concurrent("probe", {
            let proceed = Arc::clone(&proceed);
            let active = Arc::clone(&active);
            move |ctx, (service_count, linked_count): (u32, u32)| {
                let proceed = Arc::clone(&proceed);
                let active = Arc::clone(&active);
                let tx = tx.clone();
                Box::pin(async move {
                    let run = ctx.with(|mut access| {
                        let data = access.get();
                        assert_eq!(data.exporter_instances.len(), 1);
                        assert_eq!(data.contexts.len(), 1);
                        assert!(
                            data.contexts
                                .values()
                                .all(|c| c.store_id == data.active_ctx.store_id)
                        );
                        Run {
                            store: data.active_ctx.store_id.to_string(),
                            service_count,
                            linked_count,
                        }
                    });
                    active.fetch_add(1, Ordering::SeqCst);
                    let _active = ActiveProbe(active);
                    tx.send(run).unwrap();
                    proceed.acquire().await.unwrap().forget();
                    match outcome {
                        Outcome::Success | Outcome::Trap => Ok((0_u32,)),
                        Outcome::Error => Ok((1_u32,)),
                    }
                })
            }
        })?;
    let service = WorkloadService::new(
        "supervision",
        "supervision",
        "test",
        Component::new(engine(), wat::parse_str(wat)?)?,
        linker,
        vec![],
        LocalResources::default(),
        budget,
        Arc::default(),
    );
    let mut interface = WitInterface::from("wasi:http/handler@0.3.0");
    interface
        .config
        .insert("host".into(), "service.test".into());
    let resolved = UnresolvedWorkload::new(
        "supervision",
        "supervision",
        "test",
        Some(service),
        [linked],
        vec![interface],
    )
    .resolve(None, &Default::default(), host, &Default::default())
    .await?;
    Ok((
        resolved,
        Probe {
            runs,
            proceed,
            active,
        },
    ))
}

async fn join(
    resolved: &mut ResolvedWorkload,
    handle: Arc<JoinHandle<()>>,
) -> anyhow::Result<Result<(), tokio::task::JoinError>> {
    resolved.service_mut().unwrap().handle = None;
    let handle = Arc::try_unwrap(handle).map_err(|_| anyhow::anyhow!("service task is shared"))?;
    Ok(tokio::time::timeout(TIMEOUT, handle).await?)
}

#[derive(Default)]
struct Registrations {
    http: Vec<Option<mpsc::Sender<ServiceHttpJob>>>,
    messaging: Vec<Option<mpsc::Sender<MessagingJob>>>,
    routes: Vec<Vec<IngressRoute>>,
    order: Vec<&'static str>,
}

#[derive(Default)]
struct RecordingHost {
    registrations: Arc<Mutex<Registrations>>,
    fail_http: Option<usize>,
    fail_messaging: Option<usize>,
}

#[async_trait::async_trait]
impl HostHandler for RecordingHost {
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }
    fn port(&self) -> u16 {
        0
    }
    async fn on_workload_resolved(&self, _: &ResolvedWorkload, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn on_workload_unbind(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn on_service_http_resolved(
        &self,
        workload: &str,
        routes: &[IngressRoute],
        sender: mpsc::Sender<ServiceHttpJob>,
    ) -> anyhow::Result<()> {
        assert_eq!(workload, "supervision");
        let mut registrations = self.registrations.lock().unwrap();
        let fail = self.fail_http == Some(registrations.http.len() + 1);
        registrations.order.push("http");
        registrations.routes.push(routes.to_vec());
        registrations.http.push((!fail).then_some(sender));
        anyhow::ensure!(!fail, "HTTP registration refused");
        Ok(())
    }

    async fn on_trigger_service_messaging_resolved(
        &self,
        workload: &str,
        sender: mpsc::Sender<MessagingJob>,
    ) -> anyhow::Result<()> {
        assert_eq!(workload, "supervision");
        let mut registrations = self.registrations.lock().unwrap();
        let fail = self.fail_messaging == Some(registrations.messaging.len() + 1);
        registrations.order.push("messaging");
        registrations.messaging.push((!fail).then_some(sender));
        anyhow::ensure!(!fail, "messaging registration refused");
        Ok(())
    }

    fn outgoing_request(
        &self,
        workload: &str,
        request: hyper::Request<wasmtime_wasi_http::WasiBody>,
        options: Option<wasmtime_wasi_http::RequestOptions>,
        future: crate::host::http::RequestIoFuture,
        allowed_hosts: &[crate::host::allowed_hosts::AllowedHost],
    ) -> crate::host::http::SendFuture {
        HostHandler::outgoing_request(
            &crate::host::http::NullServer::default(),
            workload,
            request,
            options,
            future,
            allowed_hosts,
        )
    }
}

fn host_ref(host: &Arc<RecordingHost>) -> crate::host::HostRef {
    let handler: Arc<dyn HostHandler> = host.clone();
    crate::host::HostRef::from_handler(&handler)
}

struct Inspect {
    store: String,
    read_guest: bool,
}

impl GuestCall for Inspect {
    fn describe(&self) -> &str {
        "test:supervision/state#count"
    }

    fn call<'a>(
        self: Box<Self>,
        accessor: &'a wasmtime::component::Accessor<SharedCtx>,
        instance: wasmtime::component::Instance,
    ) -> GuestCallFuture<'a> {
        Box::pin(async move {
            accessor.with(|mut access| {
                assert_eq!(access.get().active_ctx.store_id.to_string(), self.store);
            });
            if self.read_guest {
                let function = accessor.with(|mut access| {
                    let (_, interface) = instance
                        .get_export(&mut access, None, "test:supervision/state@0.1.0")
                        .unwrap();
                    let (_, function) = instance
                        .get_export(&mut access, Some(&interface), "count")
                        .unwrap();
                    instance.get_func(&mut access, function).unwrap()
                });
                let mut values = [wasmtime::component::Val::U32(0)];
                function.call_concurrent(accessor, &[], &mut values).await?;
                assert!(matches!(values, [wasmtime::component::Val::U32(1)]));
            }
            Ok(None)
        })
    }
}

#[tokio::test]
async fn p3_supervision_uses_the_exact_budget_and_fresh_linked_instances() -> anyhow::Result<()> {
    for dispatch in [false, true] {
        for outcome in [Outcome::Success, Outcome::Error, Outcome::Trap] {
            if dispatch && matches!(outcome, Outcome::Success) {
                continue;
            }
            for budget in [0, 1, 3] {
                let ingress = Ingresses {
                    http: false,
                    messaging: false,
                    dispatch,
                };
                let (mut resolved, mut probe) =
                    workload(outcome, budget, ingress, &crate::host::HostRef::detached()).await?;
                let _claim = dispatch
                    .then(|| resolved.claim_service_dispatch())
                    .transpose()?;
                let handle = resolved
                    .execute_service()
                    .await?
                    .context("service task missing")?;
                let attempts = if matches!(outcome, Outcome::Success) {
                    1
                } else {
                    budget + 1
                };
                let mut stores = HashSet::new();
                for _ in 0..attempts {
                    assert!(stores.insert(probe.next().await?.store));
                    probe.resume();
                }
                join(&mut resolved, handle).await??;
                assert!(matches!(
                    probe.runs.try_recv(),
                    Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)
                ));
                assert_eq!(probe.active.load(Ordering::SeqCst), 0);
            }
        }
    }

    Ok(())
}

#[tokio::test]
async fn teardown_cancels_active_p3_and_trigger_execution() -> anyhow::Result<()> {
    for ingress in std::iter::once(Ingresses {
        http: false,
        messaging: false,
        dispatch: false,
    })
    .chain(CONFIGURATIONS)
    {
        let host = Arc::new(RecordingHost::default());
        let (mut resolved, mut probe) =
            workload(Outcome::Trap, 3, ingress, &host_ref(&host)).await?;
        let _claim = ingress
            .dispatch
            .then(|| resolved.claim_service_dispatch())
            .transpose()?;
        let handle = resolved
            .execute_service()
            .await?
            .context("service task missing")?;
        probe.next().await?;
        assert_eq!(probe.active.load(Ordering::SeqCst), 1);
        resolved.begin_teardown();
        assert!(
            join(&mut resolved, handle)
                .await?
                .unwrap_err()
                .is_cancelled()
        );
        assert_eq!(probe.active.load(Ordering::SeqCst), 0);
        assert!(matches!(
            probe.runs.try_recv(),
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)
        ));
        let registrations = host.registrations.lock().unwrap();
        assert!(
            registrations
                .http
                .iter()
                .flatten()
                .all(mpsc::Sender::is_closed)
        );
        assert!(
            registrations
                .messaging
                .iter()
                .flatten()
                .all(mpsc::Sender::is_closed)
        );
    }
    Ok(())
}

#[tokio::test]
async fn trigger_restarts_replace_all_registered_channels() -> anyhow::Result<()> {
    for ingress in CONFIGURATIONS {
        let host = Arc::new(RecordingHost::default());
        let (mut resolved, mut probe) =
            workload(Outcome::Trap, 2, ingress, &host_ref(&host)).await?;
        let target = if ingress.dispatch {
            let id = resolved.service.as_ref().unwrap().id().to_string();
            Some(resolved.dispatch_target(&id, "supervision-test").await?)
        } else {
            None
        };
        let handle = resolved
            .execute_service()
            .await?
            .context("service task missing")?;
        let mut stores = HashSet::new();
        for attempt in 0..3 {
            let run = probe.next().await?;
            assert!(stores.insert(run.store.clone()));
            {
                let registrations = host.registrations.lock().unwrap();
                assert_eq!(
                    registrations.http.len(),
                    usize::from(ingress.http) * (attempt + 1)
                );
                assert_eq!(
                    registrations.messaging.len(),
                    usize::from(ingress.messaging) * (attempt + 1)
                );
                for channels in [
                    registrations
                        .http
                        .iter()
                        .map(|s| s.as_ref().unwrap().is_closed())
                        .collect::<Vec<_>>(),
                    registrations
                        .messaging
                        .iter()
                        .map(|s| s.as_ref().unwrap().is_closed())
                        .collect(),
                ] {
                    assert!(channels.iter().take(attempt).all(|closed| *closed));
                    assert!(channels.last().is_none_or(|closed| !closed));
                }
                if ingress.http {
                    let current = registrations.http[attempt].as_ref().unwrap();
                    assert_eq!(current.capacity(), INGRESS_BACKLOG);
                    for previous in registrations.http[..attempt].iter().flatten() {
                        assert!(!current.same_channel(previous));
                    }
                    assert!(
                        registrations
                            .routes
                            .iter()
                            .all(|r| r == &registrations.routes[0])
                    );
                    assert!(!registrations.routes[0].is_empty());
                }
                if ingress.messaging {
                    let current = registrations.messaging[attempt].as_ref().unwrap();
                    assert_eq!(current.capacity(), INGRESS_BACKLOG);
                    for previous in registrations.messaging[..attempt].iter().flatten() {
                        assert!(!current.same_channel(previous));
                    }
                }
            }
            if let Some(target) = &target {
                tokio::time::timeout(
                    TIMEOUT,
                    target.dispatch(Inspect {
                        store: run.store,
                        read_guest: false,
                    }),
                )
                .await??;
            }
            probe.resume();
        }
        drop(target);
        join(&mut resolved, handle).await??;
        assert!(matches!(
            probe.runs.try_recv(),
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)
        ));
    }
    Ok(())
}

#[tokio::test]
async fn initial_registration_failures_do_not_spawn_a_supervisor() -> anyhow::Result<()> {
    for (fail_http, fail_messaging) in [(Some(1), None), (None, Some(1))] {
        for dispatch in [false, true] {
            let ingress = Ingresses {
                http: true,
                messaging: true,
                dispatch,
            };
            let host = Arc::new(RecordingHost {
                fail_http,
                fail_messaging,
                ..Default::default()
            });
            let (mut resolved, mut probe) =
                workload(Outcome::Trap, 3, ingress, &host_ref(&host)).await?;
            let _claim = dispatch
                .then(|| resolved.claim_service_dispatch())
                .transpose()?;
            let err = resolved.execute_service().await.unwrap_err();
            assert!(
                format!("{err:#}").contains(if fail_http.is_some() {
                    "failed to register service HTTP handler"
                } else {
                    "failed to register trigger service messaging handler"
                }),
                "{err:#}"
            );
            assert!(resolved.service.as_ref().unwrap().handle.is_none());
            assert!(matches!(
                probe.runs.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ));
            let registrations = host.registrations.lock().unwrap();
            assert_eq!(
                registrations.order,
                if fail_http.is_some() {
                    vec!["http"]
                } else {
                    vec!["http", "messaging"]
                }
            );
            assert!(
                registrations
                    .http
                    .iter()
                    .flatten()
                    .all(mpsc::Sender::is_closed)
            );
            assert!(
                registrations
                    .messaging
                    .iter()
                    .flatten()
                    .all(mpsc::Sender::is_closed)
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn losing_the_host_stops_ingress_restarts_but_dispatch_only_can_restart() -> anyhow::Result<()>
{
    for ingress in CONFIGURATIONS {
        let host = Arc::new(RecordingHost::default());
        let registrations = Arc::clone(&host.registrations);
        let (mut resolved, mut probe) =
            workload(Outcome::Trap, 1, ingress, &host_ref(&host)).await?;
        let _claim = ingress
            .dispatch
            .then(|| resolved.claim_service_dispatch())
            .transpose()?;
        let handle = resolved
            .execute_service()
            .await?
            .context("service task missing")?;
        let first = probe.next().await?;
        drop(host);
        assert!(resolved.try_http_handler().is_none());
        probe.resume();
        if !ingress.http && !ingress.messaging {
            assert_ne!(probe.next().await?.store, first.store);
            probe.resume();
        }
        join(&mut resolved, handle).await??;
        assert!(matches!(
            probe.runs.try_recv(),
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)
        ));
        let registrations = registrations.lock().unwrap();
        assert_eq!(registrations.http.len(), usize::from(ingress.http));
        assert_eq!(
            registrations.messaging.len(),
            usize::from(ingress.messaging)
        );
    }
    Ok(())
}

#[tokio::test]
async fn restart_registration_failures_keep_other_ingresses_and_retry_next_restart()
-> anyhow::Result<()> {
    for (fail_http, fail_messaging) in [(Some(2), None), (None, Some(2))] {
        let host = Arc::new(RecordingHost {
            fail_http,
            fail_messaging,
            ..Default::default()
        });
        let ingress = Ingresses {
            http: true,
            messaging: true,
            dispatch: true,
        };
        let (mut resolved, mut probe) =
            workload(Outcome::Trap, 2, ingress, &host_ref(&host)).await?;
        let id = resolved.service.as_ref().unwrap().id().to_string();
        let target = resolved.dispatch_target(&id, "supervision-test").await?;
        let handle = resolved
            .execute_service()
            .await?
            .context("service task missing")?;
        let mut stores = HashSet::new();
        for attempt in 0..3 {
            let run = probe.next().await?;
            assert!(stores.insert(run.store.clone()));
            {
                let registrations = host.registrations.lock().unwrap();
                assert_eq!(registrations.http.len(), attempt + 1);
                assert_eq!(registrations.messaging.len(), attempt + 1);
                assert_eq!(
                    registrations.http[attempt].is_none(),
                    fail_http == Some(attempt + 1)
                );
                assert_eq!(
                    registrations.messaging[attempt].is_none(),
                    fail_messaging == Some(attempt + 1)
                );
                assert!(
                    registrations.http[attempt]
                        .as_ref()
                        .is_none_or(|s| !s.is_closed())
                );
                assert!(
                    registrations.messaging[attempt]
                        .as_ref()
                        .is_none_or(|s| !s.is_closed())
                );
            }
            tokio::time::timeout(
                TIMEOUT,
                target.dispatch(Inspect {
                    store: run.store,
                    read_guest: false,
                }),
            )
            .await??;
            probe.resume();
        }
        drop(target);
        join(&mut resolved, handle).await??;
        assert!(matches!(
            probe.runs.try_recv(),
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)
        ));
    }
    Ok(())
}

#[tokio::test]
async fn successful_and_handler_error_commands_keep_serving_until_teardown() -> anyhow::Result<()> {
    for outcome in [Outcome::Success, Outcome::Error] {
        for ingress in CONFIGURATIONS {
            if matches!(outcome, Outcome::Error) && !ingress.http && !ingress.messaging {
                continue;
            }
            let host = Arc::new(RecordingHost::default());
            let (mut resolved, mut probe) = workload(outcome, 3, ingress, &host_ref(&host)).await?;
            let id = resolved.service.as_ref().unwrap().id().to_string();
            let target = resolved.dispatch_target(&id, "supervision-test").await?;
            let handle = resolved
                .execute_service()
                .await?
                .context("service task missing")?;
            let run = probe.next().await?;
            probe.resume();
            for _ in 0..2 {
                tokio::time::timeout(
                    TIMEOUT,
                    target.dispatch(Inspect {
                        store: run.store.clone(),
                        read_guest: true,
                    }),
                )
                .await??;
            }
            assert!(!handle.is_finished());
            assert_eq!(probe.active.load(Ordering::SeqCst), 0);
            resolved.begin_teardown();
            drop(target);
            assert!(
                join(&mut resolved, handle)
                    .await?
                    .unwrap_err()
                    .is_cancelled()
            );
            assert!(matches!(
                probe.runs.try_recv(),
                Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)
            ));
            let registrations = host.registrations.lock().unwrap();
            assert_eq!(registrations.http.len(), usize::from(ingress.http));
            assert_eq!(
                registrations.messaging.len(),
                usize::from(ingress.messaging)
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn losing_the_only_registration_ends_the_driver_without_spending_more_restarts()
-> anyhow::Result<()> {
    for http in [false, true] {
        let host = Arc::new(RecordingHost {
            fail_http: http.then_some(2),
            fail_messaging: (!http).then_some(2),
            ..Default::default()
        });
        let ingress = Ingresses {
            http,
            messaging: !http,
            dispatch: false,
        };
        let (mut resolved, mut probe) =
            workload(Outcome::Trap, 3, ingress, &host_ref(&host)).await?;
        let handle = resolved
            .execute_service()
            .await?
            .context("service task missing")?;
        probe.next().await?;
        probe.resume();
        join(&mut resolved, handle).await??;
        assert_eq!(probe.active.load(Ordering::SeqCst), 0);
        let registrations = host.registrations.lock().unwrap();
        assert_eq!(registrations.http.len(), if http { 2 } else { 0 });
        assert_eq!(registrations.messaging.len(), if http { 0 } else { 2 });
        assert!(
            registrations
                .http
                .iter()
                .flatten()
                .all(mpsc::Sender::is_closed)
        );
        assert!(
            registrations
                .messaging
                .iter()
                .flatten()
                .all(mpsc::Sender::is_closed)
        );
    }
    Ok(())
}

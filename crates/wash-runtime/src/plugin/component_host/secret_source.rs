//! The optional `wasmcloud:host/secret-source` or `owned-secret-source`
//! export: a host component plugin answering the host's own request for
//! secrets that feed another plugin's bindings (a plugin-backed `secrets:`
//! catalog entry).
//!
//! The host calls it, never a workload, so it is reserved like the lifecycle
//! export and kept apart from any workload-facing secrets interface the plugin
//! serves. The call runs as [`CallerIdentity::Host`], and the `request` record
//! names the consuming binding and the workload whose bind needs the values.
//! `owned-secret-source` also names that workload's verified owner.
//!
//! [`CallerIdentity::Host`]: crate::engine::ctx::CallerIdentity::Host

use std::collections::{BTreeMap, btree_map};
use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use wasmtime::component::types::Type;
use wasmtime::component::{Accessor, ComponentNamedList, Instance, Lift, Lower, TypedFunc};
use wasmtime::{AsContextMut as _, Store};

use crate::engine::abandon::{AbandonFlag, DispatchedCall};
use crate::engine::ctx::{CallerIdentity, SharedCtx};
use crate::host::job_registry::JobGuard;
use crate::host::trigger_service::{CapabilityJob, SecretSourceCall, SecretSourceReply};
use crate::plugin::{HostSecretSource, SecretKey, SecretRequest, SecretSourceError, SecretValue};

use super::{ComponentHostPluginState, ExportedInterface};

mod named {
    crate::wasmtime::component::bindgen!({
        world: "wasmcloud:host/secret-source-provider",
        exports: { default: async },
    });
    pub(super) use exports::wasmcloud::host::secret_source::*;
}

mod owned {
    crate::wasmtime::component::bindgen!({
        world: "wasmcloud:host/owned-secret-source-provider",
        exports: { default: async },
    });
    pub(super) use exports::wasmcloud::host::owned_secret_source::*;
}

type Resolve<Request, Value, Error> = TypedFunc<(Request,), (Result<Vec<(String, Value)>, Error>,)>;

/// Interface names of the exports, matched by name for the same reason as
/// [`super::HOST_LIFECYCLE_EXPORT`].
pub(super) const HOST_SECRET_SOURCE_EXPORT: &str = "secret-source";
pub(super) const HOST_OWNED_SECRET_SOURCE_EXPORT: &str = "owned-secret-source";
const RESOLVE: &str = "resolve";

/// A plugin's secret-source export, by instance name and shape.
#[derive(Clone, Debug)]
pub struct SecretSourceExport {
    pub(crate) name: Arc<str>,
    /// `owned-secret-source`, whose `workload` record carries the owner.
    pub(crate) owned: bool,
}

/// The export the host calls, of `held` and a later `found`: the owned one
/// when a plugin exports both, whichever it lists first.
pub(super) fn prefer_owned(
    held: Option<SecretSourceExport>,
    found: SecretSourceExport,
) -> SecretSourceExport {
    match held {
        Some(held) if held.owned && !found.owned => held,
        _ => found,
    }
}

/// The typed `resolve` of a plugin's secret-source export.
#[derive(Clone, Copy)]
pub(crate) enum SecretSourceResolve {
    Named(Resolve<named::Request, named::SecretValue, named::SourceError>),
    Owned(Resolve<owned::Request, owned::SecretValue, owned::SourceError>),
}

/// Check `resolve` against the published WIT at registration, so a plugin
/// built against another `request` or `source-error` shape fails host start
/// rather than every incarnation's prepare. Mirrors the record field and
/// variant case names and order that [`secret_source_resolve`] typechecks.
pub(super) fn check_secret_source_export(
    id: &str,
    export: &ExportedInterface,
    owned: bool,
) -> anyhow::Result<()> {
    let interface = if owned {
        HOST_OWNED_SECRET_SOURCE_EXPORT
    } else {
        HOST_SECRET_SOURCE_EXPORT
    };
    let resolve = export
        .funcs
        .iter()
        .find(|f| f.name.as_ref() == RESOLVE)
        .with_context(|| {
            format!("host component plugin '{id}' {interface} export is missing {RESOLVE}")
        })?;
    anyhow::ensure!(
        resolve.is_async
            && matches!(resolve.param_tys.as_ref(), [request] if is_request(request, owned))
            && matches!(resolve.result_tys.as_ref(), [result] if is_resolve_result(result)),
        "host component plugin '{id}' {RESOLVE} does not match wasmcloud:host/{interface}: \
         expected `async func(request: request) -> result<list<tuple<secret-key, \
         secret-value>>, source-error>`"
    );
    Ok(())
}

/// `record request { consumer: consumer, workload: workload, keys: list<string> }`
fn is_request(ty: &Type, owned: bool) -> bool {
    let Type::Record(record) = ty else {
        return false;
    };
    let fields: Vec<_> = record.fields().map(|f| (f.name, f.ty)).collect();
    matches!(
        fields.as_slice(),
        [("consumer", consumer), ("workload", workload), ("keys", Type::List(keys))]
            if is_consumer(consumer) && is_workload(workload, owned) && matches!(keys.ty(), Type::String)
    )
}

/// `record consumer { plugin: string, binding: option<string> }`
fn is_consumer(ty: &Type) -> bool {
    let Type::Record(record) = ty else {
        return false;
    };
    let fields: Vec<_> = record.fields().map(|f| (f.name, f.ty)).collect();
    matches!(
        fields.as_slice(),
        [("plugin", Type::String), ("binding", Type::Option(binding))]
            if matches!(binding.ty(), Type::String)
    )
}

/// `record workload { id: string, name: string, namespace: string }`, and
/// `owner: option<workload-owner>` after them when `owned`.
fn is_workload(ty: &Type, owned: bool) -> bool {
    let Type::Record(record) = ty else {
        return false;
    };
    let fields: Vec<_> = record.fields().map(|f| (f.name, f.ty)).collect();
    match fields.as_slice() {
        [
            ("id", Type::String),
            ("name", Type::String),
            ("namespace", Type::String),
        ] => !owned,
        [
            ("id", Type::String),
            ("name", Type::String),
            ("namespace", Type::String),
            ("owner", Type::Option(owner)),
        ] => owned && is_owner(&owner.ty()),
        _ => false,
    }
}

/// `record workload-owner { kind: string, name: string, uid: string }`
fn is_owner(ty: &Type) -> bool {
    let Type::Record(record) = ty else {
        return false;
    };
    let fields: Vec<_> = record.fields().map(|f| (f.name, f.ty)).collect();
    matches!(
        fields.as_slice(),
        [
            ("kind", Type::String),
            ("name", Type::String),
            ("uid", Type::String)
        ]
    )
}

/// `variant secret-value { string(string), bytes(list<u8>) }`
fn is_secret_value(ty: &Type) -> bool {
    let Type::Variant(variant) = ty else {
        return false;
    };
    let cases: Vec<_> = variant.cases().map(|c| (c.name, c.ty)).collect();
    matches!(
        cases.as_slice(),
        [("string", Some(Type::String)), ("bytes", Some(Type::List(bytes)))]
            if matches!(bytes.ty(), Type::U8)
    )
}

/// `result<list<tuple<string, secret-value>>, source-error>`
fn is_resolve_result(ty: &Type) -> bool {
    let Type::Result(result) = ty else {
        return false;
    };
    let ok = match result.ok() {
        Some(Type::List(list)) => match list.ty() {
            Type::Tuple(pair) => {
                let types: Vec<_> = pair.types().collect();
                matches!(types.as_slice(), [Type::String, value] if is_secret_value(value))
            }
            _ => false,
        },
        _ => false,
    };
    let err = match result.err() {
        Some(Type::Variant(variant)) => {
            let cases: Vec<_> = variant.cases().map(|c| (c.name, c.ty)).collect();
            let optional_text = |ty: &Option<Type>| matches!(ty, Some(Type::Option(o)) if matches!(o.ty(), Type::String));
            match cases.as_slice() {
                [
                    ("not-found", Some(Type::List(keys))),
                    ("access-denied", denied),
                    ("unavailable", unavailable),
                    ("other", other),
                ] => {
                    matches!(keys.ty(), Type::String)
                        && optional_text(denied)
                        && optional_text(unavailable)
                        && optional_text(other)
                }
                _ => false,
            }
        }
        _ => false,
    };
    ok && err
}

/// The typed `resolve` of the plugin's secret-source export.
///
/// # Errors
///
/// The export is missing, or its types differ from the published WIT.
pub(crate) fn secret_source_resolve(
    store: &mut Store<SharedCtx>,
    instance: &Instance,
    export: &SecretSourceExport,
) -> anyhow::Result<SecretSourceResolve> {
    let interface = export.name.as_ref();
    let iface = instance
        .get_export_index(&mut *store, None, interface)
        .with_context(|| format!("plugin is missing {interface} export"))?;
    let func = instance
        .get_export_index(&mut *store, Some(&iface), RESOLVE)
        .with_context(|| format!("{interface} is missing {RESOLVE}"))?;
    let mismatch = || format!("{interface}#{RESOLVE} does not match the published WIT");
    Ok(if export.owned {
        SecretSourceResolve::Owned(
            instance
                .get_typed_func(&mut *store, func)
                .map_err(anyhow::Error::from)
                .with_context(mismatch)?,
        )
    } else {
        SecretSourceResolve::Named(
            instance
                .get_typed_func(&mut *store, func)
                .map_err(anyhow::Error::from)
                .with_context(mismatch)?,
        )
    })
}

macro_rules! answer {
    ($module:ident, $answer:expr) => {
        $answer
            .map(|pairs| {
                pairs
                    .into_iter()
                    .map(|(key, value)| {
                        let value = match value {
                            $module::SecretValue::String(text) => SecretValue::new(text),
                            $module::SecretValue::Bytes(bytes) => SecretValue::from_bytes(bytes),
                        };
                        (key, value)
                    })
                    .collect()
            })
            .map_err(|error| match error {
                $module::SourceError::NotFound(keys) => {
                    SecretSourceError::NotFound(keys.into_iter().map(SecretKey::from).collect())
                }
                $module::SourceError::AccessDenied(detail) => {
                    SecretSourceError::AccessDenied(detail)
                }
                $module::SourceError::Unavailable(detail) => SecretSourceError::Unavailable(detail),
                $module::SourceError::Other(detail) => SecretSourceError::Other(detail),
            })
    };
}

impl SecretSourceResolve {
    /// Call `resolve` with `request` on the shared plugin instance, as the
    /// host, until it answers or `abandoned` gives up on it.
    pub(crate) async fn call(
        self,
        accessor: &Accessor<SharedCtx>,
        request: SecretRequest,
        job_guard: &mut JobGuard,
        abandoned: Arc<AbandonFlag>,
    ) -> wasmtime::Result<SecretSourceReply> {
        let SecretRequest {
            consumer,
            workload,
            keys,
        } = request;
        let plugin = consumer.plugin.into_string();
        let binding = consumer.binding.map(|b| b.into_string());
        let keys: Vec<String> = keys.into_iter().map(|k| k.into_string()).collect();
        Ok(match self {
            Self::Named(resolve) => {
                let request = named::Request {
                    consumer: named::Consumer { plugin, binding },
                    workload: named::Workload {
                        id: workload.id,
                        name: workload.name,
                        namespace: workload.namespace,
                    },
                    keys,
                };
                answer!(
                    named,
                    call_typed(resolve, accessor, request, job_guard, abandoned).await?
                )
            }
            Self::Owned(resolve) => {
                let request = owned::Request {
                    consumer: owned::Consumer { plugin, binding },
                    workload: owned::Workload {
                        id: workload.id,
                        name: workload.name,
                        namespace: workload.namespace,
                        owner: workload.owner.map(|owner| owned::WorkloadOwner {
                            kind: owner.kind().to_string(),
                            name: owner.name().to_string(),
                            uid: owner.uid().to_string(),
                        }),
                    },
                    keys,
                };
                answer!(
                    owned,
                    call_typed(resolve, accessor, request, job_guard, abandoned).await?
                )
            }
        })
    }
}

async fn call_typed<Params, Return>(
    resolve: TypedFunc<(Params,), (Return,)>,
    accessor: &Accessor<SharedCtx>,
    request: Params,
    job_guard: &mut JobGuard,
    abandoned: Arc<AbandonFlag>,
) -> wasmtime::Result<Return>
where
    (Params,): ComponentNamedList + Lower + Send + Sync + 'static,
    (Return,): ComponentNamedList + Lift + Send + Sync + 'static,
{
    let calls = accessor.with(|mut access| {
        crate::engine::abandon::rearm_for_call(&mut access);
        Arc::clone(&access.get().abandoned)
    });
    let call = accessor
        .with(|mut access| resolve.start_call_concurrent(access.as_context_mut(), (request,)))?;
    // Bound as the host, so no call it makes is attributed to a workload.
    job_guard.set_task(call.task(), CallerIdentity::Host);
    let (answer,) = crate::engine::abandon::watch_until_abandoned(
        &calls,
        abandoned,
        resolve.finish_call_concurrent(accessor, call),
    )
    .await
    .map_err(|e| e.context("secret-source resolve trapped"))?;
    Ok(answer)
}

/// [`HostSecretSource`] over a component plugin's secret-source export.
pub(super) struct ComponentSecretSource {
    pub(super) state: Arc<ComponentHostPluginState>,
}

#[async_trait]
impl HostSecretSource for ComponentSecretSource {
    async fn resolve(
        &self,
        request: SecretRequest,
    ) -> Result<BTreeMap<SecretKey, SecretValue>, SecretSourceError> {
        let state = &self.state;
        let failed = |msg: String| SecretSourceError::Call(anyhow::anyhow!(msg));
        let sender = state.sender().ok_or_else(|| {
            failed(format!(
                "host component plugin '{}' is not running",
                state.id
            ))
        })?;
        let (reply, reply_rx) = tokio::sync::oneshot::channel();
        let call = DispatchedCall::new("secret-source (plugin)", state.lifecycle_timeout());
        let job = CapabilityJob::SecretSource(SecretSourceCall {
            request,
            reply,
            abandoned: call.flag(),
        });
        tokio::time::timeout(state.lifecycle_timeout(), sender.send(job))
            .await
            .map_err(|_| {
                failed(format!(
                    "host component plugin '{}' channel send timed out",
                    state.id
                ))
            })?
            .map_err(|_| {
                failed(format!(
                    "host component plugin '{}' channel closed",
                    state.id
                ))
            })?;
        let pairs = match call.await_reply(reply_rx).await {
            Some(Ok(reply)) => reply.map_err(|e| SecretSourceError::Call(e.into()))??,
            Some(Err(_)) => {
                return Err(failed(format!(
                    "host component plugin '{}' dropped the secret-source reply",
                    state.id
                )));
            }
            None => {
                return Err(failed(format!(
                    "secret-source call to host component plugin '{}' timed out",
                    state.id
                )));
            }
        };
        let mut answer = BTreeMap::new();
        for (key, value) in pairs {
            match answer.entry(SecretKey::from(key)) {
                btree_map::Entry::Vacant(slot) => {
                    slot.insert(value);
                }
                btree_map::Entry::Occupied(slot) => {
                    return Err(SecretSourceError::InvalidAnswer(format!(
                        "answered `{}` more than once",
                        slot.key()
                    )));
                }
            }
        }
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::engine::Engine;

    /// Each secret-source export of the fixture `wasm`, by whether it is
    /// `owned-secret-source`.
    fn exports(wasm: &str) -> Vec<(bool, ExportedInterface)> {
        let path = format!("{}/tests/wasm/{wasm}", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {path}: {e}"));
        let engine = Engine::builder().build().unwrap();
        let (component, _) = engine.prepare_host_component(&bytes).unwrap();
        super::super::introspect_exports(&component)
            .unwrap()
            .into_iter()
            .filter_map(|export| {
                let owned = export
                    .wit
                    .interfaces
                    .contains(HOST_OWNED_SECRET_SOURCE_EXPORT);
                (owned || export.wit.interfaces.contains(HOST_SECRET_SOURCE_EXPORT))
                    .then_some((owned, export))
            })
            .collect()
    }

    /// Each export passes the check for its own shape and fails the other's,
    /// including `secret-source@0.1.6` as released.
    #[test]
    fn an_export_matches_only_its_own_shape() {
        let named = exports("secret_source_plugin.wasm");
        assert!(matches!(named.as_slice(), [(false, e)] if e.name.ends_with("@0.1.6")));
        let both = exports("owned_secret_source_plugin.wasm");
        assert_eq!(both.len(), 2);
        for (owned, export) in named.iter().chain(&both) {
            check_secret_source_export("test", export, *owned)
                .unwrap_or_else(|e| panic!("{}: {e:#}", export.name));
            let err = check_secret_source_export("test", export, !*owned).unwrap_err();
            assert!(format!("{err:#}").contains("does not match"), "{err:#}");
        }
    }

    #[test]
    fn the_owned_export_wins_in_either_order() {
        let export = |owned| SecretSourceExport {
            name: Arc::from(if owned { "owned" } else { "named" }),
            owned,
        };
        assert!(prefer_owned(Some(export(true)), export(false)).owned);
        assert!(prefer_owned(Some(export(false)), export(true)).owned);
        assert!(!prefer_owned(None, export(false)).owned);
    }
}

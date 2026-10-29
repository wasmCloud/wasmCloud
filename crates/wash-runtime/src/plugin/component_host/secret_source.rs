//! The optional `wasmcloud:host/secret-source` export: a host component plugin
//! answering the host's own request for secrets that feed another plugin's
//! bindings (a plugin-backed `secrets:` catalog entry).
//!
//! The host calls it, never a workload, so it is reserved like the lifecycle
//! export and kept apart from any workload-facing secrets interface the plugin
//! serves. The call runs as [`CallerIdentity::Host`], and the `request` record
//! names the consuming binding and the workload whose bind needs the values.
//!
//! [`CallerIdentity::Host`]: crate::engine::ctx::CallerIdentity::Host

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use wasmtime::Store;
use wasmtime::component::types::Type;
use wasmtime::component::{Instance, TypedFunc};

use crate::engine::abandon::DispatchedCall;
use crate::engine::ctx::SharedCtx;
use crate::host::trigger_service::{CapabilityJob, SecretSourceCall, SecretSourceReply};
use crate::plugin::{HostSecretSource, SecretKey, SecretRequest, SecretSourceError};

use super::{ComponentHostPluginState, ExportedInterface};

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "wasmcloud:host/secret-source-provider",
        exports: { default: async },
    });
}

use bindings::exports::wasmcloud::host::secret_source::{Consumer, Workload};
pub(crate) use bindings::exports::wasmcloud::host::secret_source::{
    Request as SecretSourceRequest, SecretValue as WitSecretValue, SourceError,
};

/// `resolve`, typed against the published `secret-source-provider` world.
pub(crate) type SecretSourceResolve =
    TypedFunc<(SecretSourceRequest,), (Result<Vec<(String, WitSecretValue)>, SourceError>,)>;

/// A `secret-value` answered by the plugin, as the host keeps it.
pub(crate) fn secret_value(value: WitSecretValue) -> crate::plugin::SecretValue {
    match value {
        WitSecretValue::String(text) => crate::plugin::SecretValue::new(text),
        WitSecretValue::Bytes(bytes) => crate::plugin::SecretValue::from_bytes(bytes),
    }
}

/// Interface name of the export, matched by name for the same reason as
/// [`super::HOST_LIFECYCLE_EXPORT`].
pub(super) const HOST_SECRET_SOURCE_EXPORT: &str = "secret-source";
const RESOLVE: &str = "resolve";

/// Check `resolve` against the published WIT at registration, so a plugin
/// built against another `request` or `source-error` shape fails host start
/// rather than every incarnation's prepare. Mirrors the record field and
/// variant case names and order that [`secret_source_resolve`] typechecks.
pub(super) fn check_secret_source_export(
    id: &str,
    export: &ExportedInterface,
) -> anyhow::Result<()> {
    let resolve = export
        .funcs
        .iter()
        .find(|f| f.name.as_ref() == RESOLVE)
        .with_context(|| {
            format!("host component plugin '{id}' secret-source export is missing {RESOLVE}")
        })?;
    anyhow::ensure!(
        resolve.is_async
            && matches!(resolve.param_tys.as_ref(), [request] if is_request(request))
            && matches!(resolve.result_tys.as_ref(), [result] if is_resolve_result(result)),
        "host component plugin '{id}' {RESOLVE} does not match wasmcloud:host/secret-source: \
         expected `async func(request: request) -> result<list<tuple<secret-key, \
         secret-value>>, source-error>`"
    );
    Ok(())
}

/// `record request { consumer: consumer, workload: workload, keys: list<string> }`
fn is_request(ty: &Type) -> bool {
    let Type::Record(record) = ty else {
        return false;
    };
    let fields: Vec<_> = record.fields().map(|f| (f.name, f.ty)).collect();
    matches!(
        fields.as_slice(),
        [("consumer", consumer), ("workload", workload), ("keys", Type::List(keys))]
            if is_consumer(consumer) && is_workload(workload) && matches!(keys.ty(), Type::String)
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

/// `record workload { id: string, name: string, namespace: string }`
fn is_workload(ty: &Type) -> bool {
    let Type::Record(record) = ty else {
        return false;
    };
    let fields: Vec<_> = record.fields().map(|f| (f.name, f.ty)).collect();
    matches!(
        fields.as_slice(),
        [
            ("id", Type::String),
            ("name", Type::String),
            ("namespace", Type::String)
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

/// The typed `resolve` of the plugin's `interface` export.
///
/// # Errors
///
/// The export is missing, or its types differ from the published WIT.
pub(crate) fn secret_source_resolve(
    store: &mut Store<SharedCtx>,
    instance: &Instance,
    interface: &str,
) -> anyhow::Result<SecretSourceResolve> {
    let iface = instance
        .get_export_index(&mut *store, None, interface)
        .with_context(|| format!("plugin is missing {interface} export"))?;
    let func = instance
        .get_export_index(&mut *store, Some(&iface), RESOLVE)
        .with_context(|| format!("{interface} is missing {RESOLVE}"))?;
    instance
        .get_typed_func(&mut *store, func)
        .map_err(anyhow::Error::from)
        .with_context(|| {
            format!("{interface}#{RESOLVE} does not match wasmcloud:host/secret-source")
        })
}

/// [`HostSecretSource`] over a component plugin's `secret-source` export.
pub(super) struct ComponentSecretSource {
    pub(super) state: Arc<ComponentHostPluginState>,
}

#[async_trait]
impl HostSecretSource for ComponentSecretSource {
    async fn resolve(
        &self,
        request: SecretRequest,
    ) -> Result<BTreeMap<SecretKey, crate::plugin::SecretValue>, SecretSourceError> {
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
            request: SecretSourceRequest {
                consumer: Consumer {
                    plugin: request.consumer.plugin.into_string(),
                    binding: request.consumer.binding.map(|b| b.into_string()),
                },
                workload: Workload {
                    id: request.workload.id,
                    name: request.workload.name,
                    namespace: request.workload.namespace,
                },
                keys: request.keys.into_iter().map(|k| k.into_string()).collect(),
            },
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
        let reply: SecretSourceReply = match call.await_reply(reply_rx).await {
            Some(Ok(reply)) => reply.map_err(|e| SecretSourceError::Call(e.into()))?,
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
        match reply {
            Ok(pairs) => Ok(pairs
                .into_iter()
                .map(|(key, value)| (SecretKey::from(key), value))
                .collect()),
            Err(SourceError::NotFound(keys)) => Err(SecretSourceError::NotFound(
                keys.into_iter().map(SecretKey::from).collect(),
            )),
            Err(SourceError::AccessDenied(detail)) => Err(SecretSourceError::AccessDenied(detail)),
            Err(SourceError::Unavailable(detail)) => Err(SecretSourceError::Unavailable(detail)),
            Err(SourceError::Other(detail)) => Err(SecretSourceError::Other(detail)),
        }
    }
}

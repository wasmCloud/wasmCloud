//! The optional `wasmcloud:host/secret-source` export: a host component plugin
//! answering the host's own request for secrets that feed another plugin's
//! bindings (a plugin-backed `secrets:` catalog entry).
//!
//! The host calls it, never a workload, so it is reserved like the lifecycle
//! export and kept apart from any workload-facing secrets interface the plugin
//! serves. The call runs as [`CallerIdentity::Host`], and the `request` record
//! names the consuming plugin and binding.
//!
//! [`CallerIdentity::Host`]: crate::engine::ctx::CallerIdentity::Host

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use wasmtime::Store;
use wasmtime::component::types::Type;
use wasmtime::component::{Instance, TypedFunc};

use crate::engine::abandon::DispatchedCall;
use crate::engine::ctx::SharedCtx;
use crate::host::trigger_service::{CapabilityJob, SecretSourceCall, SecretSourceReply};
use crate::plugin::{HostSecretSource, SecretRequest, SecretSourceError, SecretValue};

use super::{ComponentHostPluginState, ExportedInterface};

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "wasmcloud:host/secret-source-provider",
        exports: { default: async },
    });
}

pub(crate) use bindings::exports::wasmcloud::host::secret_source::{
    Request as SecretSourceRequest, SourceError,
};

/// `resolve`, typed against the published `secret-source-provider` world.
pub(crate) type SecretSourceResolve =
    TypedFunc<(SecretSourceRequest,), (Result<Vec<(String, String)>, SourceError>,)>;

/// Interface name of the export, matched by name for the same reason as
/// [`super::HOST_LIFECYCLE_EXPORT`].
pub(super) const HOST_SECRET_SOURCE_EXPORT: &str = "secret-source";
const RESOLVE: &str = "resolve";

/// Check the export's top-level shape at registration, so a grossly
/// mismatched plugin fails host start. The exact types are checked by
/// [`secret_source_resolve`] when each incarnation instantiates.
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
        matches!(resolve.param_tys.as_ref(), [Type::Record(_)])
            && matches!(resolve.result_tys.as_ref(), [Type::Result(_)]),
        "host component plugin '{id}' {RESOLVE} must take the request record and return a result"
    );
    Ok(())
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
    ) -> Result<HashMap<String, SecretValue>, SecretSourceError> {
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
                consumer: request.consumer,
                binding: request.binding,
                keys: request.keys,
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
            Ok(pairs) => Ok(pairs.into_iter().collect()),
            Err(SourceError::AccessDenied(detail)) => Err(SecretSourceError::AccessDenied(detail)),
            Err(SourceError::Unavailable(detail)) => Err(SecretSourceError::Unavailable(detail)),
            Err(SourceError::Other(detail)) => Err(SecretSourceError::Other(detail)),
        }
    }
}

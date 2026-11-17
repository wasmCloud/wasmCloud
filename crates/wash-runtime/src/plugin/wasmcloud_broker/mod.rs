//! # `wasmcloud:broker@0.1.0`
//!
//! Effects performed with a credential the guest is never handed. A component
//! imports `client`, `signer` or `verifier` once per credential, under a label
//! (`import github: wasmcloud:broker/client@0.1.0;`), and a [`BrokerPolicy`]
//! decides at bind which credential, if any, stands behind each label. A label
//! with nothing behind it fails the bind, so there is no name a guest can
//! forge at run time.
//!
//! This is the managed class: a component that imports the broker has no
//! network of its own. Its `wasi:http` requests are all refused and its
//! sockets get a policy that allows nothing, so a library that imports either
//! still links. `wasi:tls` and `reveal` are refused at bind, since neither has
//! a useful deny-all form. The only way out with a credential is an operation
//! the binding allows. All of this is per component: a sibling in the same
//! workload is not covered, so a managed component linked to one that has
//! network access can reach out through it.

mod client;
mod keys;

use std::collections::HashSet;
use std::sync::Arc;

use wasmtime::component::Accessor;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};

pub use client::{ClientBinding, Presenter};
pub use keys::{SignerKey, VerifierKey};

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit/broker",
        world: "broker",
        imports: { default: async | store | trappable | tracing },
        named_imports: {
            "wasmcloud:broker/client": crate::plugin::wasmcloud_broker::BoundClient,
            "wasmcloud:broker/signer": crate::plugin::wasmcloud_broker::BoundSigner,
            "wasmcloud:broker/verifier": crate::plugin::wasmcloud_broker::BoundVerifier,
        },
        with: {
            "wasi:http/types@0.3.0": wasmtime_wasi_http::p3::bindings::http::types,
            "wasi:clocks/types@0.3.0": wasmtime_wasi::p3::bindings::clocks::types,
        },
    });
}

use bindings::wasmcloud::broker::types::{DeniedReason, ErrorCode};

pub const WASMCLOUD_BROKER_ID: &str = "wasmcloud-broker";

/// Response header carrying the identifier of the operation's audit record.
pub const AUDIT_ID_HEADER: &str = "wasmcloud-audit-id";

/// The workload component a broker import is bound for, as the host attests
/// it. Nothing here comes from the guest.
#[derive(Clone, Debug)]
pub struct BrokerCaller {
    pub workload_id: Arc<str>,
    pub namespace: Arc<str>,
    pub workload_name: Arc<str>,
    pub component_id: Arc<str>,
    /// The component's name in the manifest, or `None` for the workload's
    /// service.
    pub component_name: Option<Arc<str>>,
}

/// The authority behind broker imports.
///
/// Asked once per labeled import when a component binds. `None` refuses the
/// bind. A binding is fixed for the life of the instance, so revocation lives
/// in what a binding holds: a leased credential that stops being served.
pub trait BrokerPolicy: Send + Sync + 'static {
    fn client(&self, caller: &BrokerCaller, label: &str) -> Option<Arc<ClientBinding>>;

    fn signer(&self, _caller: &BrokerCaller, _label: &str) -> Option<Arc<SignerKey>> {
        None
    }

    fn verifier(&self, _caller: &BrokerCaller, _label: &str) -> Option<Arc<VerifierKey>> {
        None
    }

    /// Every operation, completed or not, for audit. Never carries a
    /// credential, a message or a body. A send is recorded once its request
    /// body has been sent, which may be after the guest has the response.
    fn on_use(&self, _event: &BrokerUse<'_>) {}
}

/// One broker operation, for audit.
#[derive(Debug)]
pub struct BrokerUse<'a> {
    pub caller: &'a BrokerCaller,
    pub label: &'a str,
    pub audit_id: &'a str,
    pub operation: BrokerOperation<'a>,
    pub outcome: BrokerOutcome<'a>,
}

#[derive(Debug)]
pub enum BrokerOperation<'a> {
    Send {
        method: &'a http::Method,
        authority: &'a str,
        /// Without the query.
        path: &'a str,
    },
    Sign,
    KeyInfo,
    Verify,
}

#[derive(Clone, Copy, Debug)]
pub enum BrokerOutcome<'a> {
    /// The signature or check was produced, or the response head arrived and
    /// the request body was sent in full.
    Completed,
    /// Refused before anything was sent, for this reason.
    Denied(&'a str),
    /// Sent, then failed or was canceled, the request body included; whether
    /// it took effect is unknown.
    Indeterminate(&'a str),
    /// Failed with nothing sent, for a reason that is not a refusal, such as
    /// a connection that could not be made.
    Failed(&'a str),
}

/// What a resolved label carries into its calls.
pub struct Bound<T: ?Sized> {
    caller: Arc<BrokerCaller>,
    label: Arc<str>,
    inner: Arc<T>,
    policy: Arc<dyn BrokerPolicy>,
    /// This binding's own count against the key's limits.
    limits: keys::Limits,
}

impl<T: ?Sized> Bound<T> {
    fn audit(&self, audit_id: &str, operation: BrokerOperation<'_>, outcome: BrokerOutcome<'_>) {
        self.policy.on_use(&BrokerUse {
            caller: &self.caller,
            label: &self.label,
            audit_id,
            operation,
            outcome,
        });
    }
}

type Record<'a> = Box<dyn for<'o> FnOnce(BrokerOutcome<'o>) + Send + 'a>;

/// One operation's audit record, written once: by [`Audit::record`], or as
/// `Indeterminate("canceled")` if the guest drops the call first.
pub(super) struct Audit<'a>(Option<Record<'a>>);

impl<'a> Audit<'a> {
    pub(super) fn new(record: impl for<'o> FnOnce(BrokerOutcome<'o>) + Send + 'a) -> Self {
        Self(Some(Box::new(record)))
    }

    pub(super) fn record(mut self, outcome: BrokerOutcome<'_>) {
        if let Some(record) = self.0.take() {
            record(outcome);
        }
    }
}

impl Drop for Audit<'_> {
    fn drop(&mut self) {
        if let Some(record) = self.0.take() {
            record(BrokerOutcome::Indeterminate("canceled"));
        }
    }
}

pub type BoundClient = Arc<Bound<ClientBinding>>;
pub type BoundSigner = Arc<Bound<SignerKey>>;
pub type BoundVerifier = Arc<Bound<VerifierKey>>;

fn reason_name(reason: &DeniedReason) -> &'static str {
    match reason {
        DeniedReason::NotEntitled => "not-entitled",
        DeniedReason::DestinationNotAllowed => "destination-not-allowed",
        DeniedReason::ResolvedAddressNotAllowed => "resolved-address-not-allowed",
        DeniedReason::OperationNotAllowed => "operation-not-allowed",
        DeniedReason::ReservedHeader => "reserved-header",
        DeniedReason::CredentialUnavailable => "credential-unavailable",
        DeniedReason::BudgetExceeded => "budget-exceeded",
        DeniedReason::NotSupported => "not-supported",
        DeniedReason::Other(_) => "other",
    }
}

/// Imports a managed component is refused outright, since a deny-all version
/// would be no use to it. `None` is the whole package. Its `wasi:http` and
/// `wasi:sockets` imports link to deny-all versions instead.
const BYPASSES: &[(&str, &str, Option<&str>)] = &[
    ("wasi", "tls", None),
    ("wasmcloud", "secrets", Some("reveal")),
];

/// An unlabeled operation import names no credential. Only `types` may be
/// imported plainly.
fn refuse_unlabeled(world: &WitWorld) -> anyhow::Result<()> {
    let unlabeled = world.imports.iter().any(|import| {
        import.namespace == "wasmcloud"
            && import.package == "broker"
            && import.name.is_none()
            && ["client", "signer", "verifier"]
                .iter()
                .any(|i| import.interfaces.contains(*i))
    });
    anyhow::ensure!(
        !unlabeled,
        "a `wasmcloud:broker` import must be labeled with the credential it uses, \
         as in `import github: wasmcloud:broker/client@0.1.0;`"
    );
    Ok(())
}

fn refuse_bypasses(world: &WitWorld) -> anyhow::Result<()> {
    for import in &world.imports {
        for (namespace, package, interface) in BYPASSES {
            if import.namespace == *namespace
                && import.package == *package
                && interface.is_none_or(|i| import.interfaces.contains(i))
            {
                anyhow::bail!(
                    "a component importing `wasmcloud:broker` cannot also import \
                     `{namespace}:{package}{}`, which would reach out without its binding",
                    interface.map(|i| format!("/{i}")).unwrap_or_default()
                );
            }
        }
    }
    Ok(())
}

/// Serves `wasmcloud:broker` imports with credentials a [`BrokerPolicy`]
/// binds per label.
pub struct WasmcloudBroker {
    policy: Arc<dyn BrokerPolicy>,
}

impl WasmcloudBroker {
    pub fn new(policy: Arc<dyn BrokerPolicy>) -> Self {
        Self { policy }
    }
}

#[async_trait::async_trait]
impl HostPlugin for WasmcloudBroker {
    fn id(&self) -> &'static str {
        WASMCLOUD_BROKER_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasmcloud:broker/types,client,signer,verifier@0.1.0",
            )]),
            ..Default::default()
        }
    }

    fn supports_named_instances(&self) -> bool {
        true
    }

    /// An unlabeled import names no credential, so it is refused here rather
    /// than offered to another plugin.
    fn defers_unnamed_instances(&self) -> bool {
        false
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        if !interfaces.contains("wasmcloud", "broker", &[]) {
            return Ok(());
        }
        let world = item.world();
        refuse_unlabeled(&world)?;
        refuse_bypasses(&world)?;
        item.brokered_egress = true;

        let caller = Arc::new(BrokerCaller {
            workload_id: item.workload_id().into(),
            namespace: item.workload_namespace().into(),
            workload_name: item.workload_name().into(),
            component_id: item.id().into(),
            component_name: match &*item {
                WorkloadItem::Component(component) => Some(component.name().into()),
                WorkloadItem::Service(_) => None,
            },
        });
        let component = item.component().clone();
        let linker = item.linker()?;
        let bound = |label: &str| Bound {
            caller: Arc::clone(&caller),
            label: label.into(),
            inner: Arc::new(()),
            policy: Arc::clone(&self.policy),
            limits: keys::Limits::default(),
        };
        let unbound = |label: &str| {
            wasmtime::format_err!(
                "no broker credential is bound to `{label}` for workload {}/{}",
                caller.namespace,
                caller.workload_name
            )
        };

        bindings::wasmcloud::broker::types::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        bindings::named_imports::wasmcloud::broker::client::add_to_linker::<_, SharedCtx>(
            linker,
            &component,
            |label| {
                let inner = self
                    .policy
                    .client(&caller, label)
                    .ok_or_else(|| unbound(label))?;
                Ok(Arc::new(with_inner(bound(label), inner)))
            },
            extract_active_ctx,
        )?;
        bindings::named_imports::wasmcloud::broker::signer::add_to_linker::<_, SharedCtx>(
            linker,
            &component,
            |label| {
                let inner = self
                    .policy
                    .signer(&caller, label)
                    .ok_or_else(|| unbound(label))?;
                let limits = inner.limits();
                Ok(Arc::new(Bound {
                    limits,
                    ..with_inner(bound(label), inner)
                }))
            },
            extract_active_ctx,
        )?;
        bindings::named_imports::wasmcloud::broker::verifier::add_to_linker::<_, SharedCtx>(
            linker,
            &component,
            |label| {
                let inner = self
                    .policy
                    .verifier(&caller, label)
                    .ok_or_else(|| unbound(label))?;
                let limits = inner.limits();
                Ok(Arc::new(Bound {
                    limits,
                    ..with_inner(bound(label), inner)
                }))
            },
            extract_active_ctx,
        )?;
        Ok(())
    }
}

fn with_inner<T: ?Sized>(bound: Bound<()>, inner: Arc<T>) -> Bound<T> {
    Bound {
        caller: bound.caller,
        label: bound.label,
        inner,
        policy: bound.policy,
        limits: bound.limits,
    }
}

impl bindings::wasmcloud::broker::types::Host for ActiveCtx<'_> {}
impl bindings::named_imports::wasmcloud::broker::client::Host for ActiveCtx<'_> {}
impl bindings::named_imports::wasmcloud::broker::signer::Host for ActiveCtx<'_> {}
impl bindings::named_imports::wasmcloud::broker::verifier::Host for ActiveCtx<'_> {}

impl bindings::named_imports::wasmcloud::broker::signer::HostWithStore<SharedCtx> for SharedCtx {
    async fn sign(
        accessor: &Accessor<SharedCtx, Self>,
        bound: BoundSigner,
        message: wasmtime::component::StreamReader<u8>,
    ) -> wasmtime::Result<Result<Vec<u8>, ErrorCode>> {
        let audit_id = crate::ids::now_v7().to_string();
        let audit = Audit::new(|outcome| bound.audit(&audit_id, BrokerOperation::Sign, outcome));
        let result = keys::sign(accessor, &bound.inner, &bound.limits, message).await?;
        audit.record(match &result {
            Ok(_) => BrokerOutcome::Completed,
            Err(ErrorCode::Denied(reason)) => BrokerOutcome::Denied(reason_name(reason)),
            Err(_) => BrokerOutcome::Indeterminate("signing failed"),
        });
        Ok(result)
    }

    async fn get_key_info(
        _accessor: &Accessor<SharedCtx, Self>,
        bound: BoundSigner,
    ) -> wasmtime::Result<bindings::wasmcloud::broker::signer::KeyInfo> {
        let audit_id = crate::ids::now_v7().to_string();
        bound.audit(
            &audit_id,
            BrokerOperation::KeyInfo,
            BrokerOutcome::Completed,
        );
        Ok(bound.inner.info())
    }
}

impl bindings::named_imports::wasmcloud::broker::verifier::HostWithStore<SharedCtx> for SharedCtx {
    async fn verify(
        accessor: &Accessor<SharedCtx, Self>,
        bound: BoundVerifier,
        message: wasmtime::component::StreamReader<u8>,
        tag: Vec<u8>,
    ) -> wasmtime::Result<Result<bool, ErrorCode>> {
        let audit_id = crate::ids::now_v7().to_string();
        let audit = Audit::new(|outcome| bound.audit(&audit_id, BrokerOperation::Verify, outcome));
        let result = keys::verify(accessor, &bound.inner, &bound.limits, message, &tag).await?;
        audit.record(match &result {
            Ok(_) => BrokerOutcome::Completed,
            Err(ErrorCode::Denied(reason)) => BrokerOutcome::Denied(reason_name(reason)),
            Err(_) => BrokerOutcome::Indeterminate("verification failed"),
        });
        Ok(result)
    }
}

impl bindings::named_imports::wasmcloud::broker::client::HostWithStore<SharedCtx> for SharedCtx {
    async fn send(
        accessor: &Accessor<SharedCtx, Self>,
        bound: BoundClient,
        request: wasmtime::component::Resource<wasmtime_wasi_http::p3::Request>,
    ) -> wasmtime::Result<
        Result<
            wasmtime::component::Resource<wasmtime_wasi_http::p3::Response>,
            bindings::wasmcloud::broker::client::ErrorCode,
        >,
    > {
        client::send(accessor, &bound, request).await
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn a_managed_component_is_refused_only_imports_with_no_deny_all_form() {
        let world = |iface: &str| WitWorld {
            imports: HashSet::from([WitInterface::from(iface)]),
            ..Default::default()
        };
        assert!(refuse_bypasses(&world("wasmcloud:secrets/reveal@2.1.0")).is_err());
        assert!(refuse_bypasses(&world("wasi:tls/client@0.3.0-draft")).is_err());
        assert!(refuse_bypasses(&world("wasi:http/client@0.3.0")).is_ok());
        assert!(refuse_bypasses(&world("wasi:sockets/tcp@0.2.0")).is_ok());
        assert!(refuse_bypasses(&world("wasi:http/types@0.3.0")).is_ok());
        assert!(refuse_bypasses(&world("wasmcloud:secrets/store@2.1.0")).is_ok());
    }

    #[test]
    fn an_audit_dropped_before_its_outcome_records_a_cancel() {
        let seen = std::sync::Mutex::new(Vec::new());
        let audit = || {
            Audit::new(|outcome: BrokerOutcome<'_>| {
                seen.lock().unwrap().push(format!("{outcome:?}"));
            })
        };
        drop(audit());
        audit().record(BrokerOutcome::Completed);
        assert_eq!(
            *seen.lock().unwrap(),
            [r#"Indeterminate("canceled")"#, "Completed"]
        );
    }

    #[test]
    fn an_unlabeled_operation_import_is_refused_but_types_is_not() {
        let world = |name: Option<&str>, iface: &str| {
            let mut import = WitInterface::from(iface);
            import.name = name.map(str::to_string);
            WitWorld {
                imports: HashSet::from([import]),
                ..Default::default()
            }
        };
        assert!(refuse_unlabeled(&world(None, "wasmcloud:broker/client@0.1.0")).is_err());
        assert!(refuse_unlabeled(&world(None, "wasmcloud:broker/signer@0.1.0")).is_err());
        assert!(refuse_unlabeled(&world(None, "wasmcloud:broker/types@0.1.0")).is_ok());
        assert!(refuse_unlabeled(&world(Some("gh"), "wasmcloud:broker/client@0.1.0")).is_ok());
    }
}

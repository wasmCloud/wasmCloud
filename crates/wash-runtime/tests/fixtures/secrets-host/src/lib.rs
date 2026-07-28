//! Host component plugin that serves `wasmcloud:secrets` from its own supervised
//! store. It captures each consuming workload's secret configuration at
//! `on-workload-bind` (the operator sources those values from `secretFrom`) and
//! serves them back through `store`, correlating every call to the calling
//! workload via the host identity import. Values leave only as opaque `secret`
//! handles the caller unwraps with `reveal`.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "secrets-host", generate_all });
}

use std::collections::BTreeMap;
use std::sync::Mutex;

use bindings::exports::wasmcloud::host::workload_lifecycle::{
    Guest as LifecycleGuest, WorkloadInfo,
};
use bindings::exports::wasmcloud::secrets::reveal::{Guest as RevealGuest, SecretBorrow};
use bindings::exports::wasmcloud::secrets::store::{
    Guest as StoreGuest, GuestSecret, Secret, SecretValue, SecretsError,
};
use bindings::wasmcloud::host::identity;

/// Per-workload secret configuration captured at bind, keyed by workload id then
/// by config key. `on-workload-bind` fills it from the workload's matched
/// interface config; `on-workload-unbind` reclaims it.
static BINDS: Mutex<BTreeMap<String, BTreeMap<String, String>>> = Mutex::new(BTreeMap::new());

/// Backing state for the exported `secret` resource: the resolved value, kept
/// opaque to callers until they `reveal` it.
struct SecretState {
    value: SecretValue,
}

impl GuestSecret for SecretState {}

struct Component;

impl StoreGuest for Component {
    type Secret = SecretState;

    /// Resolve `key` from the calling workload's bind-time secret config. The
    /// caller is identified via the host identity import, exact under
    /// concurrency (resolved from this call's task). A missing key is `not-found`.
    async fn get(key: String) -> Result<Secret, SecretsError> {
        let caller = identity::get_workload_id();
        let value = BINDS
            .lock()
            .unwrap()
            .get(&caller)
            .and_then(|config| config.get(&key).cloned());
        match value {
            Some(value) => Ok(Secret::new(SecretState {
                value: SecretValue::String(value),
            })),
            None => Err(SecretsError::NotFound),
        }
    }
}

impl RevealGuest for Component {
    async fn reveal(secret: SecretBorrow<'_>) -> SecretValue {
        clone_value(&secret.get::<SecretState>().value)
    }
}

impl LifecycleGuest for Component {
    async fn on_workload_bind(workload: WorkloadInfo) -> Result<(), String> {
        // Flatten every matched interface's config into one per-workload map;
        // the `wasmcloud:secrets` binding's config carries the credentials the
        // operator sourced from `secretFrom`.
        let mut config = BTreeMap::new();
        for binding in &workload.interfaces {
            for (key, value) in &binding.config {
                config.insert(key.clone(), value.clone());
            }
        }
        BINDS.lock().unwrap().insert(workload.id, config);
        Ok(())
    }

    async fn on_workload_unbind(id: String) {
        BINDS.lock().unwrap().remove(&id);
    }
}

/// `secret-value` does not derive `Clone` in the generated bindings, so rebuild
/// it by variant to return an owned copy.
fn clone_value(value: &SecretValue) -> SecretValue {
    match value {
        SecretValue::String(s) => SecretValue::String(s.clone()),
        SecretValue::Bytes(b) => SecretValue::Bytes(b.clone()),
    }
}

mod export {
    #![allow(unsafe_code)]
    use super::{bindings, Component};
    bindings::export!(Component with_types_in bindings);
}

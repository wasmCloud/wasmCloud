//! Host component plugin exporting `wasmcloud:host/owned-secret-source` and
//! `secret-source`, so a test sees that the host calls the owned one and what
//! it was told.
//!
//! `owned-secret-source` answers:
//! - `db-password`: a fixed value.
//! - `workload`: `<namespace>/<name>` of the workload whose bind asked.
//! - `owner`: `<kind>/<name>/<uid>` of its owner, or `-` with none.
//!
//! Any other key is `not-found`. `secret-source` refuses every request.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "owned-secret-source-plugin", generate_all });
}

use bindings::exports::wasmcloud::host::{owned_secret_source, secret_source};

struct Component;

impl owned_secret_source::Guest for Component {
    async fn resolve(
        request: owned_secret_source::Request,
    ) -> Result<Vec<(String, owned_secret_source::SecretValue)>, owned_secret_source::SourceError>
    {
        let workload = request.workload;
        let mut values = Vec::new();
        let mut missing = Vec::new();
        for key in request.keys {
            let value = match key.as_str() {
                "db-password" => "hunter2".to_string(),
                "workload" => format!("{}/{}", workload.namespace, workload.name),
                "owner" => workload.owner.as_ref().map_or_else(
                    || "-".to_string(),
                    |owner| format!("{}/{}/{}", owner.kind, owner.name, owner.uid),
                ),
                _ => {
                    missing.push(key);
                    continue;
                }
            };
            values.push((key, owned_secret_source::SecretValue::String(value)));
        }
        if !missing.is_empty() {
            return Err(owned_secret_source::SourceError::NotFound(missing));
        }
        Ok(values)
    }
}

impl secret_source::Guest for Component {
    async fn resolve(
        _: secret_source::Request,
    ) -> Result<Vec<(String, secret_source::SecretValue)>, secret_source::SourceError> {
        Err(secret_source::SourceError::Other(Some(
            "called through secret-source".to_string(),
        )))
    }
}

bindings::export!(Component with_types_in bindings);

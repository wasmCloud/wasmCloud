//! Host component plugin exporting only `wasmcloud:host/secret-source`, so the
//! host's real call path into a plugin secret is testable end to end.
//!
//! Answers from a fixed table, plus probes:
//! - `who`: `<consumer>/<binding or "-">`, so a test sees what the host sent.
//! - `workload`: `<namespace>/<name>` of the workload whose bind asked.
//! - `binary`: bytes that are not UTF-8.
//! - `denied`: refused with `access-denied`.
//! - `twice`: answered twice.
//!
//! Any other key is `not-found`.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "secret-source-plugin", generate_all });
}

use bindings::exports::wasmcloud::host::secret_source::{
    Guest, Request, SecretValue, SourceError,
};

struct Component;

impl Guest for Component {
    async fn resolve(request: Request) -> Result<Vec<(String, SecretValue)>, SourceError> {
        let mut values = Vec::new();
        let mut missing = Vec::new();
        for key in request.keys {
            let value = match key.as_str() {
                "denied" => {
                    return Err(SourceError::AccessDenied(Some(format!(
                        "{} may not read `denied`",
                        request.consumer.plugin
                    ))));
                }
                "who" => SecretValue::String(format!(
                    "{}/{}",
                    request.consumer.plugin,
                    request.consumer.binding.as_deref().unwrap_or("-")
                )),
                "workload" => SecretValue::String(format!(
                    "{}/{}",
                    request.workload.namespace, request.workload.name
                )),
                "binary" => SecretValue::Bytes(vec![0xff, 0xfe]),
                "tier" => SecretValue::String("platinum".to_string()),
                "db-password" => SecretValue::String("hunter2".to_string()),
                "twice" => {
                    values.push((key.clone(), SecretValue::String("first".to_string())));
                    SecretValue::String("second".to_string())
                }
                _ => {
                    missing.push(key);
                    continue;
                }
            };
            values.push((key, value));
        }
        if !missing.is_empty() {
            return Err(SourceError::NotFound(missing));
        }
        Ok(values)
    }
}

bindings::export!(Component with_types_in bindings);

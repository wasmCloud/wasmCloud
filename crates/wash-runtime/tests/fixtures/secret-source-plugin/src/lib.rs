//! Host component plugin exporting only `wasmcloud:host/secret-source`, so the
//! host's real call path into a plugin secret is testable end to end.
//!
//! Answers from a fixed table, plus two probes:
//! - `who`: `<consumer>/<binding or "-">`, so a test sees what the host sent.
//! - `caller`: what `identity.get-caller` reports inside `resolve`.
//! - `denied`: refused with `access-denied`.
//!
//! Any other key is left out of the answer, which the host reads as not found.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "secret-source-plugin", generate_all });
}

use bindings::exports::wasmcloud::host::secret_source::{Guest, Request, SourceError};
use bindings::wasmcloud::host::identity::{Caller, get_caller};

struct Component;

impl Guest for Component {
    async fn resolve(request: Request) -> Result<Vec<(String, String)>, SourceError> {
        let mut values = Vec::new();
        for key in request.keys {
            let value = match key.as_str() {
                "denied" => {
                    return Err(SourceError::AccessDenied(Some(format!(
                        "{} may not read `denied`",
                        request.consumer
                    ))));
                }
                "who" => format!(
                    "{}/{}",
                    request.consumer,
                    request.binding.as_deref().unwrap_or("-")
                ),
                "caller" => match get_caller() {
                    Some(Caller::Host) => "host".to_string(),
                    Some(Caller::Workload(id)) => format!("workload:{id}"),
                    None => "none".to_string(),
                },
                "tier" => "platinum".to_string(),
                _ => continue,
            };
            values.push((key, value));
        }
        Ok(values)
    }
}

bindings::export!(Component with_types_in bindings);

use wasmcloud_component::http;

mod api;
use api::{
    error_resp_json, ErrorInfo, PasswordCheckRequest, PasswordCheckResponse, PasswordStrength,
    ResponseEnvelope, SecretQuery,
};

mod bindings {
    use crate::Component;

    // While normally we'd be able to use only `wasmcloud_component`,
    // we must do some generation for other WIT interfaces.
    wit_bindgen::generate!({
        with: {
            "wasmcloud:secrets/reveal@0.1.0-draft": generate,
            "wasmcloud:secrets/store@0.1.0-draft": generate,
        },
        generate_all,
    });

    wasmcloud_component::http::export!(Component);
}

/// Implementation of the `component` world (see `wit/world.wit`)
struct Component;

impl http::Server for Component {
    fn handle(
        mut request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        // Ensure we only get post requests
        if request.method() != http::Method::POST {
            return error_resp_json(
                http::StatusCode::BAD_REQUEST,
                ResponseEnvelope::<()>::Error {
                    error: ErrorInfo {
                        code: "invalid-request".into(),
                        msg: "invalid request: all requests must be POST requests".into(),
                    },
                },
            );
        }

        // Handle paths
        match request.uri().path() {
            //
            // POST /api/v1/check
            //
            "/api/v1/check" => {
                // Convert the request bytes into a password check request
                let check_req: PasswordCheckRequest =
                    match serde_json::from_reader(request.body_mut()) {
                        Ok(v) => v,
                        Err(e) => {
                            return error_resp_json(
                                http::StatusCode::BAD_REQUEST,
                                ResponseEnvelope::<()>::Error {
                                    error: ErrorInfo {
                                        code: "something".into(),
                                        msg: format!("invalid password check request: {e}").into(),
                                    },
                                },
                            )
                        }
                    };

                // Retrieve the password, possibly from secret storage
                let password = match check_req {
                    PasswordCheckRequest::RawText { value } => value,
                    PasswordCheckRequest::SecretQuery {
                        secret: SecretQuery { key, .. },
                    } => {
                        let secret = match bindings::wasmcloud::secrets::store::get(&key) {
                            Ok(s) => s,
                            Err(e) => {
                                return error_resp_json(
                                    http::StatusCode::BAD_REQUEST,
                                    ResponseEnvelope::<()>::Error {
                                        error: ErrorInfo {
                                            code: "invalid-request".into(),
                                            msg: format!("invalid secret: {e}").into(),
                                        },
                                    },
                                );
                            }
                        };

                        match bindings::wasmcloud::secrets::reveal::reveal(&secret) {
                            bindings::wasmcloud::secrets::store::SecretValue::String(s) => s,
                            bindings::wasmcloud::secrets::store::SecretValue::Bytes(_) => {
                                return error_resp_json(
                                    http::StatusCode::INTERNAL_SERVER_ERROR,
                                    ResponseEnvelope::<()>::Error {
                                        error: ErrorInfo {
                                            code: "server-error".into(),
                                            msg: "binary secrets not supported".into(),
                                        },
                                    },
                                );
                            }
                        }
                    }
                };

                // Process the password
                let analyzed = passwords::analyzer::analyze(&password);

                // Return the result
                ResponseEnvelope::Success {
                    body: PasswordCheckResponse {
                        length: password.len(),
                        strength: calculate_score(&analyzed), // estimate.score().into(),
                        contains: calculate_contains(&analyzed),
                    },
                }
                .into_http_resp(http::StatusCode::OK)
            }

            // For all other requests, we return an error
            r => ResponseEnvelope::<()>::Error {
                error: ErrorInfo {
                    code: "unknown".into(),
                    msg: format!("unrecognized endpoint [{r}]"),
                },
            }
            .into_http_resp(http::StatusCode::NOT_FOUND),
        }
    }
}

/// Calculate whether a given string contains various character classes
fn calculate_contains(p: &passwords::AnalyzedPassword) -> Vec<String> {
    let mut contains = Vec::with_capacity(4);
    if p.numbers_count() > 0 {
        contains.push("number".into());
    }
    if p.lowercase_letters_count() > 0 {
        contains.push("lowercase".into());
    }
    if p.uppercase_letters_count() > 0 {
        contains.push("uppercase".into());
    }
    if p.symbols_count() > 0 {
        contains.push("symbol".into());
    }
    contains
}

/// Calculate whether a given string contains various character classes
fn calculate_score(p: &passwords::AnalyzedPassword) -> PasswordStrength {
    let score = passwords::scorer::score(p);
    if score < 40.0 {
        return PasswordStrength::VeryWeak;
    }
    if score < 80.0 {
        return PasswordStrength::Weak;
    }
    if score < 90.0 {
        return PasswordStrength::Medium;
    }
    PasswordStrength::Strong
}

wit_bindgen::generate!({ generate_all });

use crate::wasi::http::outgoing_handler;
use crate::wasi::http::types::*;
use serde_json::Value;
use wasmcloud_component::wasi::logging::logging::{log, Level};

use crate::exports::wasmcloud::cron::scheduler::Guest as CronDemoGuest;

#[derive(Debug)]
struct CronDemo {}

impl CronDemoGuest for CronDemo {
    fn invoke(payload: Vec<u8>) -> Result<(), String> {
        // Unmarshall the Byte payload into JSON
        let json_result: Result<Value, _> = serde_json::from_slice(&payload);

        match json_result {
            Ok(json_data) => {
                log(
                    Level::Info,
                    "cronjob-scheduler",
                    &format!("Received JSON payload: {}", json_data),
                );

                let x1 = json_data.get("x1").and_then(|v| v.as_str()).unwrap_or("default_x1");

                let x2 = json_data.get("x2").and_then(|v| v.as_str()).unwrap_or("default_x2");

                let req = outgoing_handler::OutgoingRequest::new(Fields::new());
                req.set_scheme(Some(&Scheme::Http)).unwrap();
                req.set_authority(Some("localhost:3002")).unwrap();
                req.set_path_with_query(Some(format!("/payload?{}={}", x1, x2).as_ref()))
                    .unwrap();

                match outgoing_handler::handle(req, None) {
                    Ok(resp) => {
                        resp.subscribe().block();

                        match resp.get() {
                            Some(Ok(Ok(response))) => {
                                log(
                                    Level::Info,
                                    "cronjob-scheduler",
                                    format!(
                                        "HTTP request completed with status {}",
                                        response.status()
                                    )
                                    .as_str(),
                                );
                            }
                            Some(Ok(Err(code))) => {
                                log(
                                    Level::Error,
                                    "cronjob-scheduler",
                                    format!("HTTP request failed with code {}", code).as_str(),
                                );
                            }
                            _ => {
                                log(
                                    Level::Error,
                                    "cronjob-scheduler",
                                    "Failed to get HTTP response",
                                );
                            }
                        }
                    }
                    Err(e) => {
                        log(
                            Level::Error,
                            "cronjob-scheduler",
                            format!("Failed to send HTTP request: {:?}", e).as_str(),
                        );
                    }
                }
            }
            Err(e) => {
                log(
                    Level::Error,
                    "cronjob-scheduler",
                    &format!("Failed to parse payload as JSON: {}", e),
                );
                return Err(format!("JSON parsing error: {}", e));
            }
        }

        Ok(())
    }
}

export!(CronDemo);

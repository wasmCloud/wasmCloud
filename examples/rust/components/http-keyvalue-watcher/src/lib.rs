wit_bindgen::generate!({ generate_all });

use crate::wasi::http::outgoing_handler;
use crate::wasi::http::types::*;
use wasmcloud_component::wasi::logging::logging::{log, Level};

use crate::exports::wasi::keyvalue::watcher::Guest as KvWatcherDemoGuest;

#[derive(Debug)]
struct KvWatcher {}

impl KvWatcherDemoGuest for KvWatcher {
    fn on_set(bucket: wasi::keyvalue::store::Bucket, key: String, value: Vec<u8>) {
        let val = String::from_utf8(value).unwrap();
        log(
            Level::Info,
            "kv-watch-export",
            format!(
                "{} was set with {} value in {:?} bucket",
                &key, val, &bucket
            )
            .as_str(),
        );

        let req = outgoing_handler::OutgoingRequest::new(Fields::new());
        req.set_scheme(Some(&Scheme::Http)).unwrap();
        req.set_authority(Some("localhost:3001")).unwrap();
        req.set_path_with_query(Some(format!("/alert?{}={}", key, val).as_ref()))
            .unwrap();

        match outgoing_handler::handle(req, None) {
            Ok(resp) => {
                resp.subscribe().block();

                match resp.get() {
                    Some(Ok(Ok(response))) => {
                        log(
                            Level::Info,
                            "kv-watch-export",
                            format!("HTTP request completed with status {}", response.status())
                                .as_str(),
                        );
                    }
                    Some(Ok(Err(code))) => {
                        log(
                            Level::Error,
                            "kv-watch-export",
                            format!("HTTP request failed with code {}", code).as_str(),
                        );
                    }
                    _ => {
                        log(
                            Level::Error,
                            "kv-watch-export",
                            "Failed to get HTTP response",
                        );
                    }
                }
            }
            Err(e) => {
                log(
                    Level::Error,
                    "kv-watch-export",
                    format!("Failed to send HTTP request: {:?}", e).as_str(),
                );
            }
        }

        std::mem::forget(bucket);
    }
    fn on_delete(bucket: wasi::keyvalue::store::Bucket, key: String) {
        log(
            Level::Info,
            "kv-watch-export",
            format!(
                "the value of key : {} was deleted in {:?} bucket",
                &key, &bucket
            )
            .as_str(),
        );

        let req = outgoing_handler::OutgoingRequest::new(Fields::new());
        req.set_scheme(Some(&Scheme::Http)).unwrap();
        req.set_authority(Some("localhost:3001")).unwrap();
        req.set_path_with_query(Some(format!("/alert?{}=nil", key).as_ref()))
            .unwrap();

        match outgoing_handler::handle(req, None) {
            Ok(resp) => {
                resp.subscribe().block();

                match resp.get() {
                    Some(Ok(Ok(response))) => {
                        log(
                            Level::Info,
                            "kv-watch-export",
                            format!("HTTP request completed with status {}", response.status())
                                .as_str(),
                        );
                    }
                    Some(Ok(Err(code))) => {
                        log(
                            Level::Error,
                            "kv-watch-export",
                            format!("HTTP request failed with code {}", code).as_str(),
                        );
                    }
                    _ => {
                        log(
                            Level::Error,
                            "kv-watch-export",
                            "Failed to get HTTP response",
                        );
                    }
                }
            }
            Err(e) => {
                log(
                    Level::Error,
                    "kv-watch-export",
                    format!("Failed to send HTTP request: {:?}", e).as_str(),
                );
            }
        }

        std::mem::forget(bucket);
    }
}
export!(KvWatcher);

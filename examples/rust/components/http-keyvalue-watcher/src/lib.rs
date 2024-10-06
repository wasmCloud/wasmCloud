use crate::bindings::wasi::http::outgoing_handler;
use crate::bindings::wasi::http::types::*;
use std::collections::HashMap;
use wasmcloud_component::http::ErrorCode;
use wasmcloud_component::wasi::keyvalue::{store, watcher};
use wasmcloud_component::wasi::logging::logging::{log, Level};
use wasmcloud_component::wasmcloud::bus::lattice;
mod bindings {
    use super::KvWatcher;
    wit_bindgen::generate!({ generate_all });
    export!(KvWatcher);
}

use crate::bindings::exports::wasi::http::incoming_handler::Guest as HttpKvInvoker;
use crate::bindings::exports::wasi::keyvalue::watcher::Guest as KvWatcherDemoGuest;

#[derive(Debug)]
struct KvWatcher {
    store: HashMap<String, store::Bucket>,
}

impl KvWatcherDemoGuest for KvWatcher {
    fn on_set(bucket: bindings::wasi::keyvalue::store::Bucket, key: String, value: Vec<u8>) {
        let val = String::from_utf8(value.clone()).unwrap();
        log(
            Level::Info,
            "kv-watch-export",
            format!(
                "{} was set with {} value in {:?} bucket",
                &key,
                val,
                &bucket
            )
            .as_str(),
        );
        
        let req = outgoing_handler::OutgoingRequest::new(Fields::new());
        req.set_scheme(Some(&Scheme::Http)).unwrap();
        req.set_authority(Some("localhost:3001")).unwrap();
        req.set_path_with_query(Some(format!("/alert?{}={}",key,val).as_ref())).unwrap();
        
        match outgoing_handler::handle(req, None) {
            Ok(resp) => {
                resp.subscribe().block();
                
                match resp.get() {
                    Some(Ok(Ok(response))) => {
                        log(
                            Level::Info,
                            "kv-watch-export",
                            format!("HTTP request completed with status {}", response.status())
                                .as_str()
                        );
                    }
                    Some(Ok(Err(code))) => {
                        log(
                            Level::Error,
                            "kv-watch-export",
                            format!("HTTP request failed with code {}", code)
                                .as_str()
                        );
                    }
                    _ => {
                        log(
                            Level::Error,
                            "kv-watch-export",
                            "Failed to get HTTP response"
                        );
                    }
                }
            }
            Err(e) => {
                log(
                    Level::Error,
                    "kv-watch-export",
                    format!("Failed to send HTTP request: {:?}", e)
                        .as_str()
                );
            }
        }
        
        std::mem::forget(bucket);
    }
    fn on_delete(bucket: bindings::wasi::keyvalue::store::Bucket, key: String) {
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
        req.set_path_with_query(Some(format!("/alert?{}=nil",key).as_ref())).unwrap();
        
        match outgoing_handler::handle(req, None) {
            Ok(resp) => {
                resp.subscribe().block();
                
                match resp.get() {
                    Some(Ok(Ok(response))) => {
                        log(
                            Level::Info,
                            "kv-watch-export",
                            format!("HTTP request completed with status {}", response.status())
                                .as_str()
                        );
                    }
                    Some(Ok(Err(code))) => {
                        log(
                            Level::Error,
                            "kv-watch-export",
                            format!("HTTP request failed with code {}", code)
                                .as_str()
                        );
                    }
                    _ => {
                        log(
                            Level::Error,
                            "kv-watch-export",
                            "Failed to get HTTP response"
                        );
                    }
                }
            }
            Err(e) => {
                log(
                    Level::Error,
                    "kv-watch-export",
                    format!("Failed to send HTTP request: {:?}", e)
                        .as_str()
                );
            }
        }
        
        std::mem::forget(bucket);
    }
}

impl HttpKvInvoker for KvWatcher {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let response = OutgoingResponse::new(Fields::new());

        // Extract query parameters
        let path_with_query = match request.path_with_query() {
            Some(pq) => pq,
            None => {
                let error_response = OutgoingResponse::new(Fields::new());
                error_response.set_status_code(400).unwrap();
                let error_body = error_response.body().unwrap();
                error_body
                    .write()
                    .unwrap()
                    .blocking_write_and_flush(b"Bad request, did not contain path and query")
                    .unwrap();
                OutgoingBody::finish(error_body, None).expect("failed to finish response body");
                ResponseOutparam::set(response_out, Ok(error_response));
                return;
            }
        };
        // Bucket cannot be String : Letters
        let link_name = "default";
        let mut key = None;
        let mut value = None;
        let mut action = None;

        // Parse parameters from path - removing the leading '/' if present
        let params_str = path_with_query.trim_start_matches('/');
        let query_params = params_str
            .split('&')
            .filter_map(|v| v.split_once('='))
            .collect::<Vec<(&str, &str)>>();

        for (k, v) in query_params {
            match k.to_lowercase().as_str() {
                "key" => key = Some(v.to_string()),
                "value" => value = Some(v.to_string()),
                "action" => action = Some(v.to_string()),
                _ => {}
            }
        }

        // Check for required key parameter
        let key = match key {
            Some(k) => k,
            None => {
                let error_response = OutgoingResponse::new(Fields::new());
                error_response.set_status_code(400).unwrap();
                let error_body = error_response.body().unwrap();
                error_body
                    .write()
                    .unwrap()
                    .blocking_write_and_flush(b"Missing 'key' parameter")
                    .unwrap();
                OutgoingBody::finish(error_body, None).expect("failed to finish response body");
                ResponseOutparam::set(response_out, Ok(error_response));
                return;
            }
        };

        // Set up link name
        if let Err(e) = lattice::set_link_name(
            link_name,
            vec![
                lattice::CallTargetInterface::new("wasi", "keyvalue", "store"),
                lattice::CallTargetInterface::new("wasi", "keyvalue", "watcher"),
            ],
        ) {
            let error_response = OutgoingResponse::new(Fields::new());
            error_response.set_status_code(500).unwrap();
            let error_body = error_response.body().unwrap();
            error_body
                .write()
                .unwrap()
                .blocking_write_and_flush(format!("failed to set link name {:?}", e).as_bytes())
                .unwrap();
            OutgoingBody::finish(error_body, None).expect("failed to finish response body");
            ResponseOutparam::set(response_out, Ok(error_response));
            return;
        }

        // Initialize KV watcher
        let link_names = vec![Some(link_name.to_string())];
        let mut kv_watcher = KvWatcher {
            store: {
                let mut tmp_store: HashMap<String, store::Bucket> = HashMap::new();

                for link_name_option in link_names.clone() {
                    if let Some(link_name) = link_name_option {
                        match store::open(&link_name) {
                            Ok(bucket) => {
                                tmp_store.insert(link_name, bucket);
                            }
                            Err(e) => {
                                let error_response = OutgoingResponse::new(Fields::new());
                                error_response.set_status_code(500).unwrap();
                                let error_body = error_response.body().unwrap();
                                error_body
                                    .write()
                                    .unwrap()
                                    .blocking_write_and_flush(
                                        format!("failed to open bucket {:?}", e).as_bytes(),
                                    )
                                    .unwrap();
                                OutgoingBody::finish(error_body, None)
                                    .expect("failed to finish response body");
                                ResponseOutparam::set(response_out, Ok(error_response));
                                return;
                            }
                        }
                    } else {
                        log(
                            Level::Warn,
                            "kv-watch-demo",
                            "Skipping; bucket ID not specified!",
                        );
                    }
                }
                tmp_store
            },
        };

        // Handle the action
        let message = match (action.as_deref(), value) {
            (Some("on_set"), Some(val)) => {
                kv_watcher.on_set(Some(link_name.to_string()), key, val.into_bytes())
            }
            (Some("on_delete"), _) => kv_watcher.on_delete(Some(link_name.to_string()), key),
            _ => {
                let error_response = OutgoingResponse::new(Fields::new());
                error_response.set_status_code(400).unwrap();
                let error_body = error_response.body().unwrap();
                error_body
                    .write()
                    .unwrap()
                    .blocking_write_and_flush(b"Invalid action parameter")
                    .unwrap();
                OutgoingBody::finish(error_body, None).expect("failed to finish response body");
                ResponseOutparam::set(response_out, Ok(error_response));
                return;
            }
        };

        // Handle the final response
        match message {
            Ok(msg) => {
                response.set_status_code(200).unwrap();
                let response_body = response.body().unwrap();
                response_body
                    .write()
                    .unwrap()
                    .blocking_write_and_flush(msg.as_bytes())
                    .unwrap();
                OutgoingBody::finish(response_body, None).expect("failed to finish response body");
            }
            Err(e) => {
                response.set_status_code(500).unwrap();
                let response_body = response.body().unwrap();
                response_body
                    .write()
                    .unwrap()
                    .blocking_write_and_flush(format!("Operation failed: {:?}", e).as_bytes())
                    .unwrap();
                OutgoingBody::finish(response_body, None).expect("failed to finish response body");
            }
        }

        ResponseOutparam::set(response_out, Ok(response));
    }
}

impl KvWatcher {
    fn on_set(
        &mut self,
        link_name: Option<String>,
        key: String,
        value: Vec<u8>,
    ) -> Result<String, ErrorCode> {
        if let Some(bucket_id) = link_name {
            if let Some(bucket) = self.store.remove(&bucket_id) {
                // Simply call on_set since it returns ()
                watcher::on_set(bucket, &key, &value);

                if let Ok(new_bucket) = store::open(&bucket_id) {
                    self.store.insert(bucket_id.clone(), new_bucket);
                }

                let value_string = String::from_utf8(value.clone())
                    .unwrap_or_else(|_| "[Binary Data]".to_string());

                log(
                    Level::Info,
                    "kv-watcher-import",
                    &format!(
                        "Watching for set operation for key '{}' with value '{}', in bucket with id '{}'",
                        &key, value_string, &bucket_id
                    ),
                );
                Ok(format!("{}: Successfully created on_set trigger", &key).to_string())
            } else {
                log(
                    Level::Warn,
                    "kv-watcher-import",
                    &format!("Bucket with id '{}' not found or not specified", &bucket_id),
                );
                Ok("Trigger Not Set : Bucket not found".to_string())
            }
        } else {
            Ok("Linkname Doesn't contain Bucket".to_string())
        }
    }

    fn on_delete(&mut self, link_name: Option<String>, key: String) -> Result<String, ErrorCode> {
        if let Some(bucket_id) = link_name {
            if let Some(bucket) = self.store.remove(&bucket_id) {
                // Simply call on_delete since it returns ()
                watcher::on_delete(bucket, &key);

                if let Ok(new_bucket) = store::open(&bucket_id) {
                    self.store.insert(bucket_id.clone(), new_bucket);
                }

                log(
                    Level::Info,
                    "kv-watcher-import",
                    &format!(
                        "Watching for delete operation on key '{}', in bucket with id '{}'",
                        &key, &bucket_id
                    ),
                );
                Ok(format!("{}: Successfully created on_delete trigger", &key).to_string())
            } else {
                log(
                    Level::Warn,
                    "kv-watcher-import",
                    &format!("Bucket with id '{}' not found or not specified", &bucket_id),
                );
                Ok("Trigger Not Set : Bucket not found".to_string())
            }
        } else {
            Ok("Linkname Doesn't contain Bucket".to_string())
        }
    }
}

use wasmcloud_component::export;
use wasmcloud_component::http;
use wasmcloud_component::http::ErrorCode;
use wasmcloud_component::wasi::keyvalue::{store, watcher};
use wasmcloud_component::wasi::logging::logging::{log, Level};
use wasmcloud_component::wasmcloud::bus::lattice;

use std::collections::HashMap;

mod bindings {
    wit_bindgen::generate!({ generate_all });
}

use bindings::exports::wasi::keyvalue::handler::Guest as KvWatcherDemoGuest;

#[derive(Debug)]
struct KvWatcher {
    store: HashMap<String, store::Bucket>,
}
// Stuff that should happen if those actions do happen, these methods are invoked by the host.
impl KvWatcherDemoGuest for KvWatcher {
    fn on_set(bucket: store::Bucket, key: String, value: Vec<u8>) {
        log(
            Level::Info,
            "kv-watch-demo",
            format!(
                "{} was set with {} value in {} bucket",
                &key,
                String::from_utf8(&value),
                &bucket
            )
            .as_str(),
        );
    }
    fn on_delete(bucket: store::Bucket, key: String) {
        log(
            Level::Info,
            "kv-watch-demo",
            format!(
                "the value of key : {} was deleted in {} bucket",
                &key, &bucket
            )
            .as_str(),
        );
    }
    //on_set and on_delete
    //event handling logic
}

impl http::Server for KvWatcher {
    // call on_set and on_delete invokers by using self.on_.... by first extracting query params
    // then call KvWatcher helper methods

    // format : ("http://localhost:{port}/action={action}&key={key}&value={v}") for on_set
    // format : ("http://localhost:{port}/action={action}&key={key}") for on_delete
    fn handle(
        request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        let (parts, _) = request.into_parts();
        let Some(path_with_query) = parts.uri.path_and_query() else {
            return http::Response::builder()
                .status(400)
                .body("Bad request, did not contain path and query".into())
                .map_err(|e| {
                    ErrorCode::InternalError(Some(format!("failed to build response {e:?}")))
                });
        };
    
        let mut link_name = "default";
        let mut key = None;
        let mut value = None;
        let mut action = None;
    
        if let Some(query) = path_with_query.query() {
            let query_params = query
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
        }
    
        let Some(key) = key else {
            return http::Response::builder()
                .status(400)
                .body("Missing 'key' parameter".into())
                .map_err(|e| {
                    ErrorCode::InternalError(Some(format!("failed to build response {e:?}")))
                });
        };
    
        lattice::set_link_name(
            link_name,
            vec![
                lattice::CallTargetInterface::new("wasi", "keyvalue", "store"),
                lattice::CallTargetInterface::new("wasi", "keyvalue", "watcher"),
            ],
        )
        .map_err(|e| ErrorCode::InternalError(Some(format!("failed to set link name {e:?}"))))?;
    
        // Initialize KvWatcher with the bucket
        let link_names = vec![Some(link_name.to_string())];
        let kv_watcher = KvWatcher {
            store: {
                let mut tmp_store: HashMap<String, store::Bucket> = HashMap::new();
    
                for link_name_option in link_names.clone() {
                    match link_name_option {
                        Some(link_name) => {
                            let bucket = store::open(&link_name)
                                .map_err(|e| ErrorCode::InternalError(Some(format!("failed to open bucket {e:?}"))))?;
                            tmp_store.insert(link_name, bucket);
                        }
                        None => {
                            log(Level::Warn, "kv-watch-demo", "Skipping; bucket ID not specified!");
                        }
                    };
                }
                tmp_store
            },
        };
    
        let result = match (action.as_deref(), value) {
            (Some("on_set"), Some(val)) => {
                kv_watcher.on_set(Some(link_name.to_string()), key, val.as_bytes().to_vec())
            }
            (Some("on_delete"), _) => kv_watcher.on_delete(Some(link_name.to_string()), key),
            _ => return http::Response::builder()
                .status(400)
                .body("Invalid action parameter".into())
                .map_err(|e| ErrorCode::InternalError(Some(format!("failed to build response {e:?}")))),
        }
        .map_err(|e| ErrorCode::InternalError(Some(format!("operation failed {e:?}"))))?;
    
        Ok(http::Response::new(result))
    }
}

impl KvWatcher {
    // why is this here ?
    fn set_link(&self, link_name: String) -> Result<(), ()> {
        lattice::set_link_name(
            &link_name,
            vec![
                lattice::CallTargetInterface::new("wasi", "keyvalue", "store"),
                lattice::CallTargetInterface::new("wasi", "keyvalue", "watcher"),
            ],
        );
        log(
            Level::Info,
            "kv-watch-demo",
            format!("Accessing the NATS Kv store identified by '{}'", &link_name).as_str(),
        );
        Ok(())
    }
    // Helper methods for creating triggers
    fn on_set(&self, link_name: Option<String>, key: String, value: Vec<u8>) -> Result<String> {
        match link_name {
            Some(bucket_id) => match self.store.get(&bucket_id) {
                Some(bucket) => match watcher::on_set(bucket, &key, &value) {
                    Ok(_) => {
                        let value_string = match String::from_utf8(value.clone()) {
                            Ok(v) => v,
                            Err(_) => "[Binary Data]".to_string(),
                        };
                        log(
                            Level::Info,
                            "kv-watcher-demo",
                            &format!(
                                "Watching for set operation for key '{}' with value '{}', in bucket with id '{}'",
                                &key, value_string, &bucket_id
                            ),
                        );
                        Ok(())
                    }
                    Err(err) => {
                        log(
                            Level::Error,
                            "kv-watcher-demo",
                            &format!(
                                "Failed to watch for set operation for key '{}' in bucket with id '{}'",
                                &key, &bucket_id
                            ),
                        );
                        return Err(err.to_string());
                    }
                },
                None => {
                    log(
                        Level::Warn,
                        "kv-watcher-demo",
                        &format!("Bucket with id '{}' not found or not specified", &bucket_id),
                    );
                    Ok(())
                }
            },
            None => Ok(()),
        }
        Ok("Successsfully created on_set trigger")
    }
    fn on_delete(&self, link_name: Option<String>, key: String) -> Result<String> {
        match link_name {
            Some(bucket_id) => match self.store.get(&bucket_id) {
                Some(bucket) => match watcher::on_set(bucket, &key) {
                    Ok(_) => {
                        log(
                            Level::Info,
                            "kv-watcher-demo",
                            &format!(
                                "Watching for delete operation on key '{}', in bucket with id '{}'",
                                &key, &bucket_id
                            ),
                        );
                        Ok(())
                    }
                    Err(err) => {
                        log(
                            Level::Error,
                            "kv-watcher-demo",
                            &format!(
                                "Failed to watch delete operation for key '{}' in bucket with id '{}'",
                                &key, &bucket_id
                            ),
                        );
                        return Err(err.to_string());
                    }
                },
                None => {
                    log(
                        Level::Warn,
                        "kv-watcher-demo",
                        &format!("Bucket with id '{}' not found or not specified", &bucket_id),
                    );
                    Ok(())
                }
            },
            None => Ok(()),
        }
        Ok("Successsfully created on_delete trigger")
    }
}

export!(KvWatcher);

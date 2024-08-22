wit_bindgen::generate!({ generate_all });

use crate::exports::wasmcloud::messaging::handler::Guest as NatsKvDemoGuest;
use crate::wasi::keyvalue::{atomics, store};
use crate::wasi::logging::logging::{log, Level};
use crate::wasmcloud::messaging::{consumer, types};

use crate::wasmcloud::bus::lattice;

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
struct NatsKvDemo {
    store: HashMap<String, store::Bucket>,
}

const MAGIC_NUMBER: u64 = 42;
const DEFAULT_PUB_SUBJECT: &str = "nats.demo";

impl NatsKvDemoGuest for NatsKvDemo {
    fn handle_message(msg: types::BrokerMessage) -> Result<(), String> {
        // 1) Gather the component configuration data from various sources
        // 1.1) Get the first bucket id as configuration data
        let link_name1 = match crate::wasi::config::runtime::get("link_name1") {
            Ok(Some(id)) => Some(id),
            Ok(None) => return Err("First bucket id not found".to_string().into()),
            Err(_) => return Err("Failed to get first bucket id".to_string().into()),
        };
        log(
            Level::Info,
            "kv-demo",
            &format!(
                "First Bucket ID: '{}'",
                link_name1.clone().unwrap_or_default()
            ),
        );

        // 1.2) Get the second bucket id as configuration data, or set to None
        let link_name2 = match crate::wasi::config::runtime::get("link_name2") {
            Ok(Some(value)) => Some(value),
            Ok(None) | Err(_) => None,
        };
        log(
            Level::Info,
            "kv-demo",
            &format!(
                "Second Bucket ID: '{}'",
                link_name2.clone().unwrap_or_default()
            ),
        );

        // 1.3) Get the subject to publish to as a configuration data
        let pub_subject = match crate::wasi::config::runtime::get("pub_subject") {
            Ok(Some(value)) => value,
            Ok(None) => DEFAULT_PUB_SUBJECT.to_string(),
            Err(_) => return Err("Failed to get publish subject".to_string()),
        };
        log(
            Level::Info,
            "kv-demo",
            format!("Publish subject: '{}'", pub_subject).as_str(),
        );

        // 1.4) Get the counter's delta from the incoming message
        //      If the provided data is not a number, use the MAGIC_NUMBER
        let delta = match String::from_utf8(msg.body) {
            Ok(value) => value.parse::<u64>().unwrap_or(MAGIC_NUMBER),
            Err(_) => MAGIC_NUMBER,
        };
        log(
            Level::Info,
            "kv-demo",
            format!("Counter delta number: '{}'", &delta).as_str(),
        );

        // 2) Initialize the demo variables, and perform a variety of key-value operations
        // 2.1) Create a HashMap of the NATS Kv store, with the bucket ids as keys
        let link_names = vec![link_name1.clone(), link_name2.clone()];
        let demo = NatsKvDemo {
            store: {
                let mut tmp_store: HashMap<String, store::Bucket> = HashMap::new();

                for link_name_option in link_names.clone() {
                    match link_name_option {
                        Some(link_name) => {
                            let bucket = store::open(&link_name).expect("failed to access bucket");
                            tmp_store.insert(link_name, bucket);
                        }
                        None => {
                            log(Level::Warn, "kv-demo", "Skipping; bucket ID not specified!");
                        }
                    };
                }
                tmp_store
            },
        };

        // 2.2) As long as the store::open() function is a no-op, we need use the lattice::set_link_name()
        // function to make different configurations sets available to the NATS Kv provider
        for link_name_option in link_names {
            if let Some(link_name) = link_name_option {
                let _ = demo.set_link(link_name);
            }
        }
        // 2.3) Identify the keys to work with
        let local_key = "local_key".to_string();
        let counter = "counter".to_string();
        let imported_key = "imported_key".to_string();

        // 2.4) Initialize the local key with a random numeric value, in the first bucket
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");
        let random_number = now.as_secs() % MAGIC_NUMBER;
        demo.set(
            link_name1.clone(),
            local_key.clone(),
            random_number.to_string().into_bytes(),
        )?;

        // 2.5) Read the value of the local key, from the first bucket, and publish it to the pub_subject
        if let Some(local_value) = demo.get(link_name1.clone(), local_key.clone())? {
            demo.publish(pub_subject.clone(), local_value)?;
        }

        // 2.6) Increment the counter by the provided delta, in the first bucket, and publish the new value to the pub_subject
        if let Some(counter_value) = demo.increment(link_name1.clone(), counter.clone(), delta)? {
            demo.publish(pub_subject.clone(), counter_value.to_string().into_bytes())?;
        }

        // 2.7) Use the value of the first bucket's local key to set the imported key in the second bucket,
        // then publish the value to the pub_subject
        if let Some(local_value) = demo.get(link_name1.clone(), local_key.clone())? {
            demo.set(link_name2.clone(), imported_key.clone(), local_value)?;
            if let Some(imported_value) = demo.get(link_name2.clone(), imported_key.clone())? {
                demo.publish(pub_subject.clone(), imported_value)?;
            }
        }

        // 2.8) List all keys in the first bucket, and publish them to the pub_subject
        if let Some(keys) = demo.list_keys(link_name1.clone())? {
            let keys_string = keys.join(", ");
            demo.publish(pub_subject.clone(), keys_string.into_bytes())?;
        }

        // 2.9) Delete the local key from the first bucket
        demo.delete(link_name1.clone(), local_key.clone())?;

        Ok(())
    }
}

impl NatsKvDemo {
    /// Helper function to change the active link name, for NATS Kv provider.
    /// This function is needed, for as long as the store::open() function is a no-op.
    fn set_link(&self, link_name: String) -> Result<(), ()> {
        lattice::set_link_name(
            &link_name,
            vec![
                lattice::CallTargetInterface::new("wasi", "keyvalue", "store"),
                lattice::CallTargetInterface::new("wasi", "keyvalue", "atomics"),
            ],
        );
        log(
            Level::Info,
            "kv-demo",
            format!("Accessing the NATS Kv store identified by '{}'", &link_name).as_str(),
        );
        Ok(())
    }

    // Helper function to get the value of a key, from the desired bucket
    fn get(&self, link_name: Option<String>, key: String) -> Result<Option<Vec<u8>>, String> {
        match link_name {
            Some(bucket_id) => match self.store.get(&bucket_id) {
                Some(bucket) => match bucket.get(&key) {
                    Ok(Some(value)) => {
                        let value_string = match String::from_utf8(value.clone()) {
                            Ok(v) => v,
                            Err(_) => "[Binary Data]".to_string(),
                        };
                        log(
                            Level::Info,
                            "kv-demo",
                            &format!(
                                "Read key '{}' with value '{}', from bucket with id '{}'",
                                &key, value_string, &bucket_id
                            ),
                        );
                        Ok(Some(value))
                    }
                    Ok(None) => {
                        log(
                            Level::Info,
                            "kv-demo",
                            &format!(
                                "Key '{}' not found in bucket with id '{}'",
                                &key, &bucket_id
                            ),
                        );
                        Ok(None)
                    }
                    Err(err) => {
                        log(
                            Level::Info,
                            "kv-demo",
                            &format!(
                                "Error accessing key '{}' in bucket with id '{}'",
                                &key, &bucket_id
                            ),
                        );
                        return Err(err.to_string());
                    }
                },
                None => {
                    log(
                        Level::Warn,
                        "kv-demo",
                        &format!("Bucket with id '{}' not found or not specified", &bucket_id),
                    );
                    Ok(None)
                }
            },
            None => Ok(None),
        }
    }

    // Helper function to set the value of a key, in the desired bucket
    fn set(&self, link_name: Option<String>, key: String, value: Vec<u8>) -> Result<(), String> {
        match link_name {
            Some(bucket_id) => match self.store.get(&bucket_id) {
                Some(bucket) => match bucket.set(&key, &value) {
                    Ok(_) => {
                        let value_string = match String::from_utf8(value.clone()) {
                            Ok(v) => v,
                            Err(_) => "[Binary Data]".to_string(),
                        };
                        log(
                            Level::Info,
                            "kv-demo",
                            &format!(
                                "Set key '{}' with value '{}', in bucket with id '{}'",
                                &key, value_string, &bucket_id
                            ),
                        );
                        Ok(())
                    }
                    Err(err) => {
                        log(
                            Level::Error,
                            "kv-demo",
                            &format!(
                                "Failed to set key '{}' in bucket with id '{}'",
                                &key, &bucket_id
                            ),
                        );
                        return Err(err.to_string());
                    }
                },
                None => {
                    log(
                        Level::Warn,
                        "kv-demo",
                        &format!("Bucket with id '{}' not found or not specified", &bucket_id),
                    );
                    Ok(())
                }
            },
            None => Ok(()),
        }
    }

    // Helper function to delete the value of a key, from the desired bucket
    fn delete(&self, link_name: Option<String>, key: String) -> Result<(), String> {
        match link_name {
            Some(bucket_id) => match self.store.get(&bucket_id) {
                Some(bucket) => match bucket.delete(&key) {
                    Ok(_) => {
                        log(
                            Level::Info,
                            "kv-demo",
                            &format!(
                                "Deleted key '{}' from bucket with id '{}'",
                                &key, &bucket_id
                            ),
                        );
                        Ok(())
                    }
                    Err(err) => {
                        log(
                            Level::Error,
                            "kv-demo",
                            &format!(
                                "Failed to delete key '{}' from bucket with id '{}'",
                                &key, &bucket_id
                            ),
                        );
                        return Err(err.to_string());
                    }
                },
                None => {
                    log(
                        Level::Warn,
                        "kv-demo",
                        &format!("Bucket with id '{}' not found or not specified", &bucket_id),
                    );
                    Ok(())
                }
            },
            None => Ok(()),
        }
    }

    // Helper function to list all keys in the desired bucket
    fn list_keys(&self, link_name: Option<String>) -> Result<Option<Vec<String>>, String> {
        match link_name {
            Some(bucket_id) => match self.store.get(&bucket_id) {
                Some(bucket) => match bucket.list_keys(Some(0u64)) {
                    Ok(key_response) => {
                        let keys = key_response.keys;
                        let all_keys = keys.join(", ");
                        log(
                            Level::Info,
                            "kv-demo",
                            &format!(
                                "Listed keys: '{}', from bucket with id '{}'",
                                all_keys, &bucket_id
                            ),
                        );
                        Ok(Some(keys))
                    }
                    Err(err) => {
                        log(
                            Level::Error,
                            "kv-demo",
                            &format!("Failed to list keys, from bucket with id '{}'", &bucket_id),
                        );
                        return Err(err.to_string());
                    }
                },
                None => {
                    log(
                        Level::Warn,
                        "kv-demo",
                        &format!("Bucket with id '{}' not found or not specified", &bucket_id),
                    );
                    Ok(None)
                }
            },
            None => Ok(None),
        }
    }

    // Helper function to increment the value of a key, in the desired bucket
    fn increment(
        &self,
        link_name: Option<String>,
        key: String,
        delta: u64,
    ) -> Result<Option<u64>, String> {
        match link_name {
            Some(bucket_id) => match self.store.get(&bucket_id) {
                Some(bucket) => match atomics::increment(&bucket, &key, delta) {
                    Ok(counter) => {
                        log(
                            Level::Info,
                            "kv-demo",
                            &format!(
                                "Incremented key '{}' to '{}', in bucket with id '{}'",
                                &key, counter, &bucket_id
                            ),
                        );
                        Ok(Some(counter))
                    }
                    Err(err) => {
                        log(
                            Level::Error,
                            "kv-demo",
                            &format!(
                                "Failed to increment key '{}', in bucket with id '{}'",
                                &key, &bucket_id
                            ),
                        );
                        return Err(err.to_string());
                    }
                },
                None => {
                    log(
                        Level::Warn,
                        "kv-demo",
                        &format!("Bucket with id '{}' not found or not specified", &bucket_id),
                    );
                    Ok(None)
                }
            },
            None => Ok(None),
        }
    }

    // Helper function to publish a message to the pub_subject
    fn publish(&self, pub_subject: String, message: Vec<u8>) -> Result<(), String> {
        if let Err(_) = consumer::publish(&types::BrokerMessage {
            subject: pub_subject.clone(),
            reply_to: None,
            body: message.clone(),
        }) {
            log(Level::Error, "kv-demo", "Failed to publish message");
            return Err("Failed to publish message".to_string());
        }
        let msg_string = match String::from_utf8(message) {
            Ok(v) => v,
            Err(_) => "[Binary Data]".to_string(),
        };
        log(
            Level::Info,
            "kv-demo",
            &format!(
                "Published message: '{}', to subject '{}'",
                msg_string, pub_subject
            ),
        );
        Ok(())
    }
}

export!(NatsKvDemo);

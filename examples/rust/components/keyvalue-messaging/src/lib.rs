#![allow(clippy::missing_safety_doc)]
wit_bindgen::generate!();

use crate::exports::wasmcloud::messaging::handler::Guest as NatsKvDemoGuest;
use crate::wasi::keyvalue::{store, atomics};
use crate::wasmcloud::messaging::{consumer, types};
use crate::wasi::logging::logging::{log, Level};

struct NatsKvDemo;

const DEFAULT_BUCKET: &str = "WASMCLOUD";
const DEFAULT_COUNT: u64 = 1;
const DEFAULT_PUB_SUBJECT: &str = "nats.atomic";


impl NatsKvDemoGuest for NatsKvDemo {
    fn handle_message(msg: types::BrokerMessage) -> Result<(), String> {
        // Get the bucket name as a configuration value
        let bucket_name = match crate::wasi::config::runtime::get("bucket") {
            Ok(Some(value)) => value,
            Ok(None) => DEFAULT_BUCKET.to_string(),
            Err(_) => return Err("Failed to get bucket name".to_string()),
        };
        log(Level::Info, "kv-demo", format!("Bucket name: {}", bucket_name).as_str());

        // Get the repitition count as a configuration value
        let count = match crate::wasi::config::runtime::get("count") {
            Ok(Some(value)) => value.parse::<u64>().unwrap_or(DEFAULT_COUNT),
            Ok(None) => DEFAULT_COUNT,
            Err(_) => return Err("Failed to get repetition count".to_string()),
        };
        log(Level::Info, "kv-demo", format!("Count: {}", count).as_str());

        // Get the subject to publish to as a configuration value
        let pub_subject = match crate::wasi::config::runtime::get("pub_subject") {
            Ok(Some(value)) => value,
            Ok(None) => DEFAULT_PUB_SUBJECT.to_string(),
            Err(_) => return Err("Failed to get publish subject".to_string()),
        };
        log(Level::Info, "kv-demo", format!("Publish subject: {}", pub_subject).as_str());

        // Get the key from the message
        let key = match String::from_utf8(msg.body) {
            Ok(value) => value,
            Err(_) => return Err("Failed to convert message body to string".to_string()),
        };
        log(Level::Info, "kv-demo", format!("Key: {}", key).as_str());

        // Open the bucket
        let bucket: store::Bucket = store::open(&bucket_name).expect("failed to open bucket");

        // Increment the key, and repeat the increment count times
        for _ in 1..=count {
            let counter = atomics::increment(&bucket, &key, 1);
            if let Ok(count) = counter {
                log(Level::Info, "kv-demo", format!("Incremented key {} to {}", key, count).as_str());
            }
        }

        // Read the value of the key, and publish it to the pub_subject
        match bucket.get(&key) {
            Ok(Some(value)) => {
                if let Err(_) = consumer::publish(&types::BrokerMessage {
                    subject: pub_subject.clone(),
                    reply_to: None,
                    body: value.clone(),
                }) {
                    log(Level::Error, "kv-demo", "Failed to publish message");
                }
                match String::from_utf8(value) {
                    Ok(value_string) => {
                        log(Level::Info, "kv-demo", format!("published key {} with value {} to NATS {} subject", key.clone(), value_string, pub_subject).as_str());
                    }
                    Err(_) => log(Level::Error, "kv-demo", "Failed to convert value to string"),
                }
            }
            Ok(None) => log(Level::Info, "kv-demo", format!("No value found for key {}", key.clone()).as_str()),
            Err(_) => return Err("Failed to get key value".to_string()),
        };

        // List all keys in the bucket, and publish them to the pub_subject
        match bucket.list_keys(Some(0u64)) {
            Ok(key_response) => {
                for key in key_response.keys {
                    if let Err(_) = consumer::publish(&types::BrokerMessage {
                        subject: pub_subject.clone(),
                        reply_to: None,
                        body: key.clone().into_bytes(),
                    }) {
                        log(Level::Error, "kv-demo", "Failed to publish message");
                    }
                    log(Level::Info, "kv-demo", format!("Listed key: {}", key).as_str());
                }
            },
            Err(_) => log(Level::Error, "kv-demo", "Failed to list keys"),
        }

        // Delete the key
        if let Err(_) = bucket.delete(&key) {
            log(Level::Error, "kv-demo", format!("Failed to delete key {}", key).as_str());
        } else {
            log(Level::Info, "kv-demo", format!("Deleted key {}", key).as_str());
        }

        Ok(())
    }
}

export!(NatsKvDemo);

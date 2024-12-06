//! HTTP Blobstore Example
//!
//! This example demonstrates all operations available in the WASI blobstore interface:
//!
//! 1. Container Operations:
//!    - Creates two containers ("ying" and "yang")
//!    - Verifies container existence
//!    - Retrieves container metadata
//!
//! 2. Basic Blob Operations:
//!    - Writes four blobs ("earth", "air", "fire", "water") to "ying" container
//!    - Reads back and verifies content
//!    - Demonstrates partial content reading (first 4 bytes)
//!
//! 3. Advanced Operations:
//!    - Moves "fire" from "ying" to "yang" container
//!    - Copies "water" from "ying" to "yang" container
//!    - Lists objects in both containers
//!    - Cleans up by clearing "ying" container
//!
//! The results are returned as a JSON response showing:
//! - Success/failure of each operation
//! - Timestamps of operations
//! - Detailed messages about what happened
//!
//! Container names can be configured through runtime config "container_names"
//! as a comma-separated string. If not provided, defaults to "ying,yang".

mod bindings {
    use crate::BlobstoreComponent;

    wit_bindgen::generate!({ generate_all });

    export!(BlobstoreComponent);
}

use bindings::wasi::blobstore::blobstore::{
    copy_object, create_container, get_container, move_object,
};
use bindings::wasi::blobstore::types::{IncomingValue, ObjectId, OutgoingValue};
use bindings::wasi::http::types::*;
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const DEFAULT_CONTAINERS: [&str; 2] = ["ying", "yang"];
const TEST_BLOBS: [&str; 4] = ["earth", "air", "fire", "water"];

/// Represents the result of an operation
#[derive(Serialize, Deserialize)]
struct OperationResult {
    success: bool,
    message: String,
    timestamp: String,
}

/// Represents the result of an operation
#[derive(Serialize, Deserialize)]
struct BlobstoreDemo {
    container_ops: HashMap<String, OperationResult>,
    blob_ops: HashMap<String, OperationResult>,
    container_names: Vec<String>,
}

struct BlobstoreComponent;

/// Implements the HTTP handler for the BlobstoreDemo component
impl bindings::exports::wasi::http::incoming_handler::Guest for BlobstoreComponent {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        let mut demo = BlobstoreDemo {
            container_ops: HashMap::new(),
            blob_ops: HashMap::new(),
            container_names: get_container_names(),
        };

        // Demonstrate container operations
        demonstrate_container_ops(&mut demo);

        // Demonstrate blob operations
        demonstrate_blob_ops(&mut demo);

        // Send response
        let response = OutgoingResponse::new(Fields::new());
        let response_body = response.body().expect("response body to exist");
        let stream = response_body.write().unwrap();
        ResponseOutparam::set(response_out, Ok(response));

        let json = serde_json::to_string_pretty(&demo).unwrap();
        stream.blocking_write_and_flush(json.as_bytes()).unwrap();

        drop(stream);
        OutgoingBody::finish(response_body, None).expect("failed to finish response body");
    }
}

/// Retrieves the container names from the runtime configuration
fn get_container_names() -> Vec<String> {
    match bindings::wasi::config::runtime::get("container_names") {
        Ok(Some(names)) => names.split(',').map(String::from).collect(),
        _ => DEFAULT_CONTAINERS.iter().map(|&s| s.to_string()).collect(),
    }
}

/// Demonstrates container operations
fn demonstrate_container_ops(demo: &mut BlobstoreDemo) {
    // Clone the names to avoid borrow checker issues
    let container_names = demo.container_names.clone();
    for container in container_names {
        // Create container
        match create_container(&container) {
            Ok(c) => {
                record_success(demo, "create_container", &format!("Created {}", container));

                // Get container info/metadata
                match c.info() {
                    Ok(info) => record_success(
                        demo,
                        "container_info",
                        &format!(
                            "Container {} created at {}",
                            info.name,
                            format_timestamp(info.created_at)
                        ),
                    ),
                    Err(e) => record_error(demo, "container_info", &e),
                }
            }
            Err(e) => record_error(demo, "create_container", &e),
        }
    }
}

/// Demonstrates blob operations
fn demonstrate_blob_ops(demo: &mut BlobstoreDemo) {
    let ying = demo.container_names[0].clone();
    let yang = demo.container_names[1].clone();

    // Write all elements to ying
    for blob in TEST_BLOBS {
        let data = format!("Content of {}", blob);
        match write_blob(&ying, blob, data.as_bytes()) {
            Ok(_) => record_success(demo, "write_blob", &format!("Wrote {} to {}", blob, ying)),
            Err(e) => record_error(demo, "write_blob", &e),
        }

        // Test has_object
        if let Ok(c) = get_container(&ying) {
            match c.has_object(&blob.to_string()) {
                Ok(exists) => record_success(
                    demo,
                    "has_object",
                    &format!("Object {} exists in {}: {}", blob, ying, exists),
                ),
                Err(e) => record_error(demo, "has_object", &e),
            };

            // Test object_info
            match c.object_info(&blob.to_string()) {
                Ok(info) => record_success(
                    demo,
                    "object_info",
                    &format!(
                        "Object {} in {} created at {}, size: {}",
                        info.name,
                        info.container,
                        format_timestamp(info.created_at),
                        info.size
                    ),
                ),
                Err(e) => record_error(demo, "object_info", &e),
            };
        }
    }

    // Read back and verify content
    for blob in TEST_BLOBS {
        match read_blob(&ying, blob, None, None) {
            Ok(data) => record_success(
                demo,
                "read_blob",
                &format!(
                    "Read {} from {}: {}",
                    blob,
                    ying,
                    String::from_utf8_lossy(&data)
                ),
            ),
            Err(e) => record_error(
                demo,
                "read_blob",
                &format!("Failed to read {}: {}", blob, e),
            ),
        }
    }

    // Demonstrate partial read (first 4 bytes)
    match read_blob(&ying, "water", Some(0), Some(4)) {
        Ok(data) => record_success(
            demo,
            "partial_read",
            &format!(
                "Partial read of 'water' (first 4 bytes): {}",
                String::from_utf8_lossy(&data)
            ),
        ),
        Err(e) => record_error(demo, "partial_read", &format!("Failed partial read: {}", e)),
    }

    // Move fire to yang
    match move_blob(&ying, &yang, "fire") {
        Ok(_) => record_success(
            demo,
            "move_blob",
            &format!("Moved fire from {} to {}", ying, yang),
        ),
        Err(e) => record_error(demo, "move_blob", &format!("Failed to move fire: {}", e)),
    }

    // Copy water to yang
    match copy_blob(&ying, &yang, "water") {
        Ok(_) => record_success(
            demo,
            "copy_blob",
            &format!("Copied water from {} to {}", ying, yang),
        ),
        Err(e) => record_error(demo, "copy_blob", &format!("Failed to copy water: {}", e)),
    }

    // List objects in both containers
    let container_names = demo.container_names.clone();
    for container in container_names {
        if let Ok(c) = get_container(&container) {
            match c.list_objects() {
                Ok(stream) => match stream.read_stream_object_names(100) {
                    Ok((objects, _)) => record_success(
                        demo,
                        "list_objects",
                        &format!("Objects in {}: {:?}", container, objects),
                    ),
                    Err(e) => record_error(
                        demo,
                        "list_objects",
                        &format!("Failed to read stream: {}", e),
                    ),
                },
                Err(e) => record_error(
                    demo,
                    "list_objects",
                    &format!("Failed to list objects: {}", e),
                ),
            }
        }
    }

    // Test batch delete_objects
    if let Ok(c) = get_container(&ying) {
        let blobs_to_delete = vec!["earth".to_string(), "air".to_string()];
        match c.delete_objects(&blobs_to_delete) {
            Ok(_) => record_success(
                demo,
                "delete_objects",
                &format!(
                    "Deleted multiple objects from {}: {:?}",
                    ying, blobs_to_delete
                ),
            ),
            Err(e) => record_error(demo, "delete_objects", &e),
        };
    }

    // Test single delete_object
    if let Ok(c) = get_container(&yang) {
        match c.delete_object(&"water".to_string()) {
            Ok(_) => record_success(
                demo,
                "delete_object",
                &format!("Deleted water from {}", yang),
            ),
            Err(e) => record_error(demo, "delete_object", &e),
        };
    }

    // Clean up ying container (renamed from clear to clear_container)
    if let Ok(c) = get_container(&ying) {
        match c.clear() {
            Ok(_) => record_success(demo, "clear_container", &format!("Cleared {}", ying)),
            Err(e) => record_error(
                demo,
                "clear_container",
                &format!("Failed to clear {}: {}", ying, e),
            ),
        }
    }
}

/// Records a successful operation
fn record_success(demo: &mut BlobstoreDemo, op: &str, msg: &str) {
    let ops = if op.contains("blob") || op.contains("object") {
        &mut demo.blob_ops
    } else {
        &mut demo.container_ops
    };

    ops.insert(
        op.to_string(),
        OperationResult {
            success: true,
            message: msg.to_string(),
            timestamp: format_timestamp(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            ),
        },
    );
}

/// Records an error operation
fn record_error(demo: &mut BlobstoreDemo, op: &str, msg: &str) {
    let ops = if op.contains("blob") || op.contains("object") {
        &mut demo.blob_ops
    } else {
        &mut demo.container_ops
    };

    ops.insert(
        op.to_string(),
        OperationResult {
            success: false,
            message: msg.to_string(),
            timestamp: format_timestamp(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            ),
        },
    );
}

// Helper functions for blob operations
fn write_blob(container: &str, name: &str, data: &[u8]) -> Result<(), String> {
    let container = get_container(&container.to_string())?;
    let value = OutgoingValue::new_outgoing_value();
    let stream = value
        .outgoing_value_write_body()
        .map_err(|_| "Failed to get write body".to_string())?;
    stream
        .blocking_write_and_flush(data)
        .map_err(|_| "Failed to write data".to_string())?;
    drop(stream);

    // Write data before finishing the value
    let result = container
        .write_data(&name.to_string(), &value)
        .map_err(|e| e.to_string());

    OutgoingValue::finish(value).map_err(|_| "Failed to finish value".to_string())?;

    result
}

/// Reads a blob from the specified container
fn read_blob(
    container: &str,
    name: &str,
    start: Option<u64>,
    end: Option<u64>,
) -> Result<Vec<u8>, String> {
    let container = get_container(&container.to_string())?;
    let start = start.unwrap_or(0);
    let end = end.unwrap_or(u64::MAX);
    let value = container
        .get_data(&name.to_string(), start, end)
        .map_err(|e| e.to_string())?;
    let body = IncomingValue::incoming_value_consume_sync(value).map_err(|e| e.to_string())?;
    Ok(body)
}

/// Moves a blob from one container to another
fn move_blob(src_container: &str, dst_container: &str, name: &str) -> Result<(), String> {
    let src = ObjectId {
        container: src_container.to_string(),
        object: name.to_string(),
    };
    let dst = ObjectId {
        container: dst_container.to_string(),
        object: name.to_string(),
    };
    move_object(&src, &dst).map_err(|e| e.to_string())
}

/// Copies a blob from one container to another
fn copy_blob(src_container: &str, dst_container: &str, name: &str) -> Result<(), String> {
    let src = ObjectId {
        container: src_container.to_string(),
        object: name.to_string(),
    };
    let dst = ObjectId {
        container: dst_container.to_string(),
        object: name.to_string(),
    };
    copy_object(&src, &dst).map_err(|e| e.to_string())
}

/// Formats a Unix timestamp as a string
fn format_timestamp(unix_time: u64) -> String {
    let dt = Utc
        .timestamp_opt(unix_time as i64, 0)
        .unwrap()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();
    dt
}

use std::process::Command;

use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::io::AsyncReadExt;

const TEST_IMAGE: &str = "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0";

#[test]
#[ignore = "network: pulls a real component from ghcr.io"]
fn end_to_end_pull_compile_write() {
    let dir = tempfile::tempdir().unwrap();
    let output_path = dir.path().join("out.cwasm");
    let output_url = format!("file://{}", output_path.display());

    let status = Command::new(env!("CARGO_BIN_EXE_wash-precompile"))
        .arg("--image")
        .arg("ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0")
        .arg("--output")
        .arg(&output_url)
        .status()
        .expect("failed to spawn wash-precompile");

    assert!(status.success(), "wash-precompile exited with {status}");

    let metadata = std::fs::metadata(&output_path).expect("output file not written");
    assert!(metadata.len() > 0, "output file is empty");
}

#[tokio::test]
#[ignore = "network: pulls a real component, requires Docker for NATS container"]
async fn end_to_end_nats_output() {
    let container = GenericImage::new("nats", "2-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .expect("failed to start NATS container");

    let port = container
        .get_host_port_ipv4(4222)
        .await
        .expect("failed to get NATS host port");
    let nats_url = format!("nats://127.0.0.1:{port}");

    let bucket = "test-precompile";
    let key = "myapp/test.cwasm";
    let output_url = format!("nats://{bucket}/{key}");

    // NOTE: Output to the same url two times so that we test if the bucket
    // exists flow also succeeds
    let status = Command::new(env!("CARGO_BIN_EXE_wash-precompile"))
        .env("NATS_URL", &nats_url)
        .arg("--image")
        .arg(TEST_IMAGE)
        .arg("--output")
        .arg(&output_url)
        .status()
        .expect("failed to spawn wash-precompile");

    assert!(status.success(), "wash-precompile exited with {status}");

    let status = Command::new(env!("CARGO_BIN_EXE_wash-precompile"))
        .env("NATS_URL", &nats_url)
        .arg("--image")
        .arg(TEST_IMAGE)
        .arg("--output")
        .arg(&output_url)
        .status()
        .expect("failed to spawn wash-precompile");

    assert!(status.success(), "wash-precompile exited with {status}");

    let client = async_nats::connect(&nats_url)
        .await
        .expect("failed to connect to NATS");
    let jetstream = async_nats::jetstream::new(client);
    let store = jetstream
        .get_object_store(bucket)
        .await
        .expect("bucket not created by worker");

    let mut object = store.get(key).await.expect("object not found in bucket");
    let mut bytes = Vec::new();
    object
        .read_to_end(&mut bytes)
        .await
        .expect("failed to read object");

    assert!(!bytes.is_empty(), "object is empty");
}

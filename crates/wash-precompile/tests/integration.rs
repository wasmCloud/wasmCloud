use std::process::Command;

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

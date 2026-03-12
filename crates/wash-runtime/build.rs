// Build scripts commonly use expect() since panics produce clear compile-time errors
#![allow(clippy::expect_used)]

use std::env;
use std::fs::{self};
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_dir() -> anyhow::Result<PathBuf> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let mut current_path = PathBuf::from(manifest_dir);

    // Search upwards for the workspace root
    loop {
        if current_path.join("Cargo.lock").exists() {
            println!("cargo:rustc-env=WORKSPACE_ROOT={}", current_path.display());
            return Ok(current_path);
        }

        if let Some(parent) = current_path.parent() {
            current_path = parent.to_path_buf();
        } else {
            anyhow::bail!("Could not find workspace root")
        }
    }
}

fn check_and_rebuild_fixtures(
    workspace_dir: &Path,
    tracked_examples: &[&str],
) -> anyhow::Result<()> {
    let fixtures_dir = workspace_dir.join("crates/wash-runtime/tests/fixtures");
    let wasm_dir = workspace_dir.join("crates/wash-runtime/tests/wasm");

    if !fixtures_dir.exists() {
        println!("No fixtures dir found at {}", fixtures_dir.display());
        anyhow::bail!("No fixtures dir found");
    }

    // Create wasm directory if it doesn't exist
    if fs::create_dir_all(&wasm_dir).is_err() {
        println!(
            "Failed to create wasm directory at {}. Some tests will fail.",
            wasm_dir.display()
        );
        anyhow::bail!("Failed to create wasm directory");
    }

    // Tell cargo to rerun this build script if (only) fixture source files change
    for example in tracked_examples {
        let example_dir = fixtures_dir.join(example);
        if !example_dir.exists() {
            return Err(anyhow::anyhow!(
                "Fixture directory {} does not exist",
                example_dir.display()
            ));
        }

        // Only watch source files and Cargo.toml
        println!(
            "cargo:rerun-if-changed={}/Cargo.toml",
            example_dir.display()
        );

        let src_dir = example_dir.join("src");
        if src_dir.exists() {
            println!("cargo:rerun-if-changed={}", src_dir.display());
        }

        let wit_dir = example_dir.join("wit");
        if wit_dir.exists() {
            println!("cargo:rerun-if-changed={}", wit_dir.display());
        }

        // Build the example
        let status = Command::new("cargo")
            .args(["build", "--target", "wasm32-wasip2", "--release"])
            .current_dir(&example_dir)
            .status();

        match status {
            Ok(s) if s.success() => {
                // Copy wasm artifacts
                let artifact_dir = example_dir.join("target/wasm32-wasip2/release");
                if artifact_dir.exists() {
                    for wasm_entry in fs::read_dir(&artifact_dir)? {
                        let wasm_entry = wasm_entry?;
                        let wasm_path = wasm_entry.path();

                        if wasm_path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                            let dest = wasm_dir
                                .join(wasm_path.file_name().expect("wasm file should have a name"));
                            fs::copy(&wasm_path, &dest)?;
                        }
                    }
                }
            }
            Ok(_) => {
                println!("cargo:warning=Failed to build {}", example);
                continue;
            }
            Err(e) => {
                println!(
                    "cargo:warning=Failed to execute cargo for {}: {}",
                    example, e
                );
                continue;
            }
        }
    }

    Ok(())
}

fn main() {
    let out_dir = PathBuf::from(
        env::var("OUT_DIR").expect("failed to look up `OUT_DIR` from environment variables"),
    );
    let workspace_dir = workspace_dir().expect("failed to get workspace dir");

    // Track specific example directories we care about
    let tracked_examples = [
        "http-counter",
        "cron-service",
        "cron-component",
        "http-blobstore",
        "http-webgpu",
        "cpu-usage-service",
        "messaging-handler",
        "inter-component-call-caller",
        "inter-component-call-callee",
        "inter-component-call-middleware",
        "http-allowed-hosts",
    ];

    // Rebuild fixtures if examples changed
    check_and_rebuild_fixtures(&workspace_dir, &tracked_examples)
        .expect("failed to check/rebuild fixtures");

    let top_proto_dir = workspace_dir.join("proto");
    let proto_dir = top_proto_dir.join("wasmcloud/runtime/v2");

    let proto_dir_files = fs::read_dir(proto_dir).expect("failed to list files in `proto_dir`");
    let proto_files: Vec<PathBuf> = proto_dir_files
        .into_iter()
        .map(|file| file.expect("failed to read proto file").path())
        .collect();

    let descriptor_file = out_dir.join("runtime.bin");

    tonic_prost_build::configure()
        .compile_well_known_types(true)
        .file_descriptor_set_path(&descriptor_file)
        .extern_path(".google.protobuf", "::pbjson_types")
        .compile_protos(&proto_files, &[top_proto_dir])
        .expect("failed to compile protos");

    // Generate serde bindings for the Runtime API
    let descriptor_bytes = std::fs::read(descriptor_file).expect("failed to read descriptor file");

    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_bytes)
        .expect("failed to register descriptor")
        .build(&[".wasmcloud.runtime.v2"])
        .expect("failed to build final protos");

    println!("cargo:rerun-if-changed=build.rs");
}

// Build scripts commonly use expect() since panics produce clear compile-time errors
#![allow(clippy::expect_used)]

//! Generates the Rust bindings for the `wasmcloud.runtime.v2` protobuf
//! service into `OUT_DIR`, included from `src/washlet/mod.rs`.
//!
//! Proto codegen is all this build script does — it stays pure-Rust so the
//! build never spawns a nested `cargo` ("cargo building cargo"). The wasm
//! test fixtures are built separately by `cargo xtask build-fixtures`
//! (see /xtask).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Returns the path to the workspace root directory (the dir holding `Cargo.lock`).
fn workspace_dir() -> anyhow::Result<PathBuf> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let mut current_path = PathBuf::from(manifest_dir);

    loop {
        if current_path.join("Cargo.lock").exists() {
            return Ok(current_path);
        }

        if let Some(parent) = current_path.parent() {
            current_path = parent.to_path_buf();
        } else {
            anyhow::bail!("Could not find workspace root")
        }
    }
}

fn compile_protos(workspace_dir: &Path, out_dir: &Path) {
    let top_proto_dir = workspace_dir.join("proto");
    let proto_dir = top_proto_dir.join("wasmcloud/runtime/v2");

    let proto_files: Vec<PathBuf> = fs::read_dir(&proto_dir)
        .expect("failed to read proto dir")
        .map(|f| f.expect("failed to read proto file").path())
        .collect();

    // Tell cargo to re-run this build script whenever any .proto file
    // under proto/wasmcloud/runtime/v2 changes.
    println!("cargo:rerun-if-changed={}", proto_dir.display());
    for proto in &proto_files {
        println!("cargo:rerun-if-changed={}", proto.display());
    }

    let descriptor_file = out_dir.join("runtime.bin");

    tonic_prost_build::configure()
        .compile_well_known_types(true)
        .file_descriptor_set_path(&descriptor_file)
        .extern_path(".google.protobuf", "::pbjson_types")
        .compile_protos(&proto_files, &[top_proto_dir])
        .expect("failed to compile protos");

    let descriptor_bytes = fs::read(&descriptor_file).expect("failed to read descriptor file");

    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_bytes)
        .expect("failed to register descriptor")
        .build(&[".wasmcloud.runtime.v2"])
        .expect("failed to build final protos");
}

fn main() {
    let out_dir = PathBuf::from(
        env::var("OUT_DIR").expect("failed to look up `OUT_DIR` from environment variables"),
    );
    let workspace_dir = workspace_dir().expect("failed to get workspace dir");

    compile_protos(&workspace_dir, &out_dir);

    println!("cargo:rerun-if-changed=build.rs");
}

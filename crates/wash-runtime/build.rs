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
            .args([
                "build",
                "-p",
                example,
                "--target",
                "wasm32-wasip2",
                "--release",
            ])
            .current_dir(&fixtures_dir)
            .status();

        match status {
            Ok(s) if s.success() => {
                // Copy wasm artifacts
                let artifact_dir = fixtures_dir.join("target/wasm32-wasip2/release");
                if artifact_dir.exists() {
                    let underscored_name = format!("{}.wasm", example.replace("-", "_"));
                    let underscored_path = artifact_dir.join(&underscored_name);

                    let (wasm_name, wasm_path) = if underscored_path.exists() {
                        (underscored_name, underscored_path)
                    } else {
                        let hyphenated_name = format!("{example}.wasm");
                        let hyphenated_path = artifact_dir.join(&hyphenated_name);
                        (hyphenated_name, hyphenated_path)
                    };

                    if wasm_path.exists() {
                        let dest = wasm_dir.join(&wasm_name);
                        fs::copy(&wasm_path, &dest)?;
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

/// P3 fixtures are built with wasm32-wasip1 and then componentized with the reactor adapter.
/// WIT deps are resolved from the shared `p3-wit-deps/` directory — each subdirectory
/// contains a WIT package that gets copied into the fixture's `wit/deps/` before building.
/// The fixture's `wkg.toml` documents these overrides for use with `wkg wit fetch` outside
/// the build script.
fn check_and_rebuild_p3_fixtures(workspace_dir: &Path, p3_fixtures: &[&str]) -> anyhow::Result<()> {
    let fixtures_dir = workspace_dir.join("crates/wash-runtime/tests/fixtures");
    let wasm_dir = workspace_dir.join("crates/wash-runtime/tests/wasm");

    fs::create_dir_all(&wasm_dir)?;

    // Reactor adapter from the pinned provider crate (matches our wasmtime version)
    let reactor_adapter =
        wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER;

    // Watch the shared P3 WIT deps
    let shared_wit = fixtures_dir.join("p3-wit-deps");
    println!("cargo:rerun-if-changed={}", shared_wit.display());

    for example in p3_fixtures {
        let example_dir = fixtures_dir.join(example);
        if !example_dir.exists() {
            return Err(anyhow::anyhow!(
                "P3 fixture directory {} does not exist",
                example_dir.display()
            ));
        }

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

        // Step 0: Resolve WIT deps from shared p3-wit-deps/ into fixture's wit/deps/
        resolve_p3_wit_deps(&shared_wit, &example_dir)?;

        // Step 1: Build the core module with wasm32-wasip1
        let status = Command::new("cargo")
            .args([
                "build",
                "-p",
                example,
                "--target",
                "wasm32-wasip1",
                "--release",
            ])
            .current_dir(&fixtures_dir)
            .status();

        match status {
            Ok(s) if s.success() => {}
            Ok(_) => {
                println!("cargo:warning=Failed to build P3 fixture {}", example);
                continue;
            }
            Err(e) => {
                println!(
                    "cargo:warning=Failed to execute cargo for P3 fixture {}: {}",
                    example, e
                );
                continue;
            }
        }

        // Step 2: Componentize with reactor adapter using wit-component crate
        let artifact_dir = fixtures_dir.join("target/wasm32-wasip1/release");
        if !artifact_dir.exists() {
            println!("cargo:warning=No artifact dir for P3 fixture {}", example);
            continue;
        }

        let underscored_name = format!("{}.wasm", example.replace("-", "_"));
        let underscored_path = artifact_dir.join(&underscored_name);

        let (wasm_name, wasm_path) = if underscored_path.exists() {
            (underscored_name, underscored_path)
        } else {
            let hyphenated_name = format!("{example}.wasm");
            let hyphenated_path = artifact_dir.join(&hyphenated_name);
            (hyphenated_name, hyphenated_path)
        };

        if wasm_path.exists() {
            let dest = wasm_dir.join(&wasm_name);
            let core_module = fs::read(&wasm_path)?;
            let component = wit_component::ComponentEncoder::default()
                .validate(true)
                .module(&core_module)
                .expect("failed to set module")
                .adapter("wasi_snapshot_preview1", reactor_adapter)
                .expect("failed to set adapter")
                .encode()
                .expect("failed to encode component");
            fs::write(&dest, component)?;
        }
    }

    Ok(())
}

/// Copy shared P3 WIT packages into a fixture's `wit/deps/` directory.
/// Each subdirectory in `shared_wit_dir` contains a `package.wit` declaring a WIT package.
/// We read the package declaration to determine the correct `wit/deps/` directory name
/// (e.g., `wasi-http-0.3.0-rc-2026-03-15`).
fn resolve_p3_wit_deps(shared_wit_dir: &Path, fixture_dir: &Path) -> anyhow::Result<()> {
    let deps_dir = fixture_dir.join("wit/deps");
    fs::create_dir_all(&deps_dir)?;

    for entry in fs::read_dir(shared_wit_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let pkg_wit = entry.path().join("package.wit");
        if !pkg_wit.exists() {
            continue;
        }

        // Read the package declaration to get namespace:name@version
        let content = fs::read_to_string(&pkg_wit)?;
        let Some(pkg_line) = content.lines().find(|l| l.starts_with("package ")) else {
            continue;
        };

        // Parse "package wasi:http@0.3.0-rc-2026-03-15;" -> "wasi-http-0.3.0-rc-2026-03-15"
        let pkg_id = pkg_line
            .trim_start_matches("package ")
            .trim_end_matches(';')
            .trim();
        let dep_name = pkg_id.replace([':', '@'], "-");

        let dest_dir = deps_dir.join(&dep_name);
        fs::create_dir_all(&dest_dir)?;
        fs::copy(&pkg_wit, dest_dir.join("package.wit"))?;
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

    // P3 fixtures: built with wasm32-wasip1 + reactor adapter
    let p3_fixtures = [
        "http-handler-p3",
        "http-blobstore-p3",
        "cli-service-p3",
        "socket-test-p3",
        "inter-component-call-p3-caller",
        "inter-component-call-p3-callee",
    ];

    // Build test fixtures. The rerun-if-changed directives ensure these only
    // rebuild when fixture source files actually change, not on every build.
    check_and_rebuild_fixtures(&workspace_dir, &tracked_examples)
        .expect("failed to check/rebuild fixtures");

    check_and_rebuild_p3_fixtures(&workspace_dir, &p3_fixtures)
        .expect("failed to check/rebuild P3 fixtures");

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

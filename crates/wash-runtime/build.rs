// Build scripts commonly use expect() since panics produce clear compile-time errors
#![allow(clippy::expect_used)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER;

/// Returns the path to the workspace root directory
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

fn emit_fixture_rerun_if_changed(fixture_dir: &Path) {
    println!(
        "cargo:rerun-if-changed={}/Cargo.toml",
        fixture_dir.display()
    );
    for sub in ["src", "wit"] {
        let d = fixture_dir.join(sub);
        if d.exists() {
            println!("cargo:rerun-if-changed={}", d.display());
        }
    }
}

/// Returns `true` on success. Failures are reported via `cargo:warning`
/// and swallowed so one bad fixture doesn't mask the rest.
fn run_cargo_build_wasm(fixtures_dir: &Path, fixture: &str, target: &str) -> bool {
    let status = Command::new("cargo")
        .args(["build", "-p", fixture, "--target", target, "--release"])
        .current_dir(fixtures_dir)
        .status();
    match status {
        Ok(s) if s.success() => true,
        Ok(_) => {
            println!("cargo:warning=Failed to build {fixture}");
            false
        }
        Err(e) => {
            println!("cargo:warning=Failed to execute cargo for {fixture}: {e}");
            false
        }
    }
}

/// Which preview of the WASI component model a fixture targets. Drives
/// the `cargo` target triple and the post-build step: `P2` emits a
/// component directly, `P3` builds a core module that we wrap with the
/// WASI reactor adapter to produce a component.
#[derive(Copy, Clone)]
enum FixtureKind {
    P2,
    P3,
}

impl FixtureKind {
    fn target(self) -> &'static str {
        match self {
            FixtureKind::P2 => "wasm32-wasip2",
            FixtureKind::P3 => "wasm32-wasip1",
        }
    }

    fn shared_wit_dir(self) -> &'static str {
        match self {
            FixtureKind::P2 => "p2-wit-deps",
            FixtureKind::P3 => "p3-wit-deps",
        }
    }
}

/// Wrap a `wasm32-wasip1` core module with the WASI reactor adapter to
/// produce a component. The adapter is pinned by the
/// `wasi-preview1-component-adapter-provider` dep alongside our wasmtime
/// version, so its ABI stays in lockstep.
fn componentize(core_module: &[u8], adapter: &[u8]) -> Vec<u8> {
    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(core_module)
        .expect("failed to set module")
        .adapter("wasi_snapshot_preview1", adapter)
        .expect("failed to set adapter")
        .encode()
        .expect("failed to encode component")
}

/// Build a batch of WIT fixtures: populate `wit/deps/` from the kind's
/// shared WIT directory, run `cargo build` for the kind's target, and
/// stage the resulting wasm under `tests/wasm/` (componentizing core
/// modules for P3 on the way through).
///
/// `skip_shared_wit` lists fixtures whose world uses only local
/// interfaces (no wasi imports). Copying shared deps into those
/// fixtures would pollute their wit resolution with unneeded packages.
fn build_fixtures(
    workspace_dir: &Path,
    fixtures: &[&str],
    kind: FixtureKind,
    skip_shared_wit: &[&str],
) -> anyhow::Result<()> {
    let fixtures_dir = workspace_dir.join("crates/wash-runtime/tests/fixtures");
    let wasm_dir = workspace_dir.join("crates/wash-runtime/tests/wasm");

    if !fixtures_dir.exists() {
        anyhow::bail!("No fixtures dir found at {}", fixtures_dir.display());
    }
    fs::create_dir_all(&wasm_dir)?;

    let shared_wit = fixtures_dir.join(kind.shared_wit_dir());
    println!("cargo:rerun-if-changed={}", shared_wit.display());

    let target = kind.target();
    let artifact_dir = fixtures_dir.join(format!("target/{target}/release"));

    for fixture in fixtures {
        let fixture_dir = fixtures_dir.join(fixture);
        if !fixture_dir.exists() {
            anyhow::bail!("Fixture directory {} does not exist", fixture_dir.display());
        }

        emit_fixture_rerun_if_changed(&fixture_dir);

        if fixture_dir.join("wit").exists() && !skip_shared_wit.contains(fixture) {
            copy_shared_wit_deps(&shared_wit, &fixture_dir)?;
        }

        if !run_cargo_build_wasm(&fixtures_dir, fixture, target) {
            continue;
        }

        //.  try the underscore name first (cdylib), fall back to the hyphenated name (bin)
        let wasm_name = format!("{}.wasm", fixture.replace('-', "_"));
        let wasm_path = artifact_dir
            .join(&wasm_name)
            .exists()
            .then(|| artifact_dir.join(&wasm_name))
            .or_else(|| {
                let bin = artifact_dir.join(format!("{fixture}.wasm"));
                bin.exists().then_some(bin)
            });
        let Some(wasm_path) = wasm_path else {
            println!("cargo:warning=No artifact for fixture {fixture}");
            continue;
        };
        let dest = wasm_dir.join(&wasm_name);

        match kind {
            FixtureKind::P2 => {
                fs::copy(&wasm_path, dest)?;
            }
            FixtureKind::P3 => {
                let core = fs::read(&wasm_path)?;
                fs::write(
                    dest,
                    componentize(&core, WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER),
                )?;
            }
        }
    }

    Ok(())
}

/// Copy every `{pkg}/package.wit` from `shared_wit_dir` into the
/// fixture's `wit/deps/{pkg}/package.wit`. Source dir names already
/// include the version (e.g. `wasi-http-0.2.2`), matching the layout
/// wit-bindgen expects, so this is a plain recursive copy.
fn copy_shared_wit_deps(shared_wit_dir: &Path, fixture_dir: &Path) -> anyhow::Result<()> {
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
        let dest_dir = deps_dir.join(entry.file_name());
        fs::create_dir_all(&dest_dir)?;
        fs::copy(&pkg_wit, dest_dir.join("package.wit"))?;
    }

    Ok(())
}

const P2_FIXTURES: &[&str] = &[
    "http-handler-p2",
    "http-counter",
    "cron-service",
    "cron-component",
    "http-blobstore",
    "http-webgpu",
    "cpu-usage-service",
    "messaging-handler",
    "messaging-echo",
    "inter-component-call-caller",
    "inter-component-call-callee",
    "inter-component-call-middleware",
    "http-allowed-hosts",
];

const P3_FIXTURES: &[&str] = &[
    "http-handler-p3",
    "http-blobstore-p3",
    "cli-service-p3",
    "socket-test-p3",
    "inter-component-call-p3-caller",
    "inter-component-call-p3-callee",
];

// Fixtures with local-only WIT worlds (no wasi imports). Shared deps
// would pollute their wit resolution with unneeded packages.
const P2_SKIP_SHARED_WIT: &[&str] = &["cron-service", "cron-component"];

fn build_all_fixtures(workspace_dir: &Path) {
    build_fixtures(
        workspace_dir,
        P2_FIXTURES,
        FixtureKind::P2,
        P2_SKIP_SHARED_WIT,
    )
    .expect("failed to build P2 fixtures");
    build_fixtures(workspace_dir, P3_FIXTURES, FixtureKind::P3, &[])
        .expect("failed to build P3 fixtures");
}

fn compile_protos(workspace_dir: &Path, out_dir: &Path) {
    let top_proto_dir = workspace_dir.join("proto");
    let proto_dir = top_proto_dir.join("wasmcloud/runtime/v2");

    let proto_files: Vec<PathBuf> = fs::read_dir(&proto_dir)
        .expect("failed to read proto dir")
        .map(|f| f.expect("failed to read proto file").path())
        .collect();

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

    // Export WORKSPACE_ROOT so runtime code (`env!("WORKSPACE_ROOT")`)
    // can locate fixture artifacts regardless of how it was invoked.
    println!("cargo:rustc-env=WORKSPACE_ROOT={}", workspace_dir.display());

    build_all_fixtures(&workspace_dir);
    compile_protos(&workspace_dir, &out_dir);

    println!("cargo:rerun-if-changed=build.rs");
}

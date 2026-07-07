//! Workspace task runner (the cargo-xtask pattern).
//!
//! Run via the `cargo xtask <task>` alias defined in `/.cargo/config.toml`.
//!
//! The primary task is `build-fixtures`, which compiles the wash-runtime
//! wasm test fixtures and stages them under `tests/wasm/`.
//! Run build-fixtures once, then the tests read `.wasm` files via `include_bytes!`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER;

#[derive(Parser)]
#[command(name = "xtask", about = "wasmCloud workspace tasks", version)]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Build the wash-runtime wasm test fixtures into
    /// `crates/wash-runtime/tests/wasm/`.
    BuildFixtures,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let workspace = workspace_dir().context("failed to locate workspace root")?;
    match cli.task {
        Task::BuildFixtures => build_fixtures(&workspace),
    }
}

/// Walk up from this crate's manifest dir to the workspace root (the
/// directory holding `Cargo.lock`). Works regardless of the directory
/// `cargo xtask` was invoked from.
fn workspace_dir() -> Result<PathBuf> {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join("Cargo.lock").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!("could not find workspace root (no Cargo.lock above the xtask crate)");
        }
    }
}

/// Which preview of the WASI component model a fixture targets. Drives the
/// `cargo` target triple and the post-build step: `P2` emits a component
/// directly, `P3` builds a core module that we wrap with the WASI reactor
/// adapter to produce a component.
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
    "keyvalue-counter",
    "keyvalue-implements",
    "postgres-implements",
];

const P3_FIXTURES: &[&str] = &[
    "http-handler-p3",
    "http-blobstore-p3",
    "cli-service-p3",
    "socket-test-p3",
    "tls-echo-client-p3",
    "inter-component-call-p3-caller",
    "inter-component-call-p3-callee",
    "stream-producer-p3",
    "stream-consumer-p3",
    "stream-pacer-p3",
    "res-producer-p3",
    "res-sink-p3",
    "res-caller-p3",
    "ephemeral-callee-p3",
    "ephemeral-caller-p3",
    "blobstore-implements-p3",
    "blobstore-default-p3",
    "keyvalue-implements-p3",
    "keyvalue-default-p3",
    "postgres-stream-p3",
];

// Fixtures with local-only WIT worlds (no wasi imports). Copying shared
// deps into these would pollute their wit resolution with unneeded packages.
const P2_SKIP_SHARED_WIT: &[&str] = &["cron-service", "cron-component"];

fn build_fixtures(workspace: &Path) -> Result<()> {
    let fixtures_dir = workspace.join("crates/wash-runtime/tests/fixtures");
    let wasm_dir = workspace.join("crates/wash-runtime/tests/wasm");

    if !fixtures_dir.exists() {
        bail!("no fixtures dir found at {}", fixtures_dir.display());
    }
    fs::create_dir_all(&wasm_dir)
        .with_context(|| format!("failed to create {}", wasm_dir.display()))?;

    build_kind(
        &fixtures_dir,
        &wasm_dir,
        FixtureKind::P2,
        P2_FIXTURES,
        P2_SKIP_SHARED_WIT,
    )?;
    build_kind(&fixtures_dir, &wasm_dir, FixtureKind::P3, P3_FIXTURES, &[])?;

    println!(
        "staged {} fixtures into {}",
        P2_FIXTURES.len() + P3_FIXTURES.len(),
        wasm_dir.display()
    );
    Ok(())
}

/// Build every fixture for one WASI preview: populate each fixture's
/// `wit/deps/` from the shared WIT dir, run a single `cargo build` for the
/// kind's target, then stage the resulting wasm under `tests/wasm/`
/// (componentizing core modules for P3 on the way through).
fn build_kind(
    fixtures_dir: &Path,
    wasm_dir: &Path,
    kind: FixtureKind,
    fixtures: &[&str],
    skip_shared_wit: &[&str],
) -> Result<()> {
    let shared_wit = fixtures_dir.join(kind.shared_wit_dir());

    for fixture in fixtures {
        let fixture_dir = fixtures_dir.join(fixture);
        if !fixture_dir.exists() {
            bail!("fixture directory {} does not exist", fixture_dir.display());
        }
        if fixture_dir.join("wit").exists() && !skip_shared_wit.contains(fixture) {
            copy_shared_wit_deps(&shared_wit, &fixture_dir)
                .with_context(|| format!("failed to stage wit deps for {fixture}"))?;
        }
    }

    let target = kind.target();
    println!("building {} fixtures for {target}…", fixtures.len());

    // One cargo build for the whole batch instead of one per fixture. The
    // nested fixtures workspace shares a target dir, so a single invocation
    // builds shared deps once and parallelizes across fixtures.
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--release", "--target", target])
        .current_dir(fixtures_dir)
        // Drop any ambient CARGO_TARGET_DIR (the bench hosts set one job-wide)
        // so the nested build lands in `fixtures_dir/target`, where the staging
        // step below looks for artifacts. Without this, the build succeeds but
        // writes elsewhere and staging fails to find anything.
        .env_remove("CARGO_TARGET_DIR");
    for fixture in fixtures {
        cmd.args(["-p", fixture]);
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to execute cargo for {target} fixtures"))?;
    if !status.success() {
        // Fail the whole batch on any fixture error: a fixture that won't
        // compile should turn the build red, not leave tests running
        // against stale wasm.
        bail!("cargo build failed for {target} fixtures");
    }

    let artifact_dir = fixtures_dir.join(format!("target/{target}/release"));
    for fixture in fixtures {
        stage_fixture(&artifact_dir, wasm_dir, fixture, kind)
            .with_context(|| format!("failed to stage {fixture}"))?;
    }

    Ok(())
}

/// Copy the built wasm for one fixture into `tests/wasm/`, componentizing
/// P3 core modules with the reactor adapter on the way.
fn stage_fixture(
    artifact_dir: &Path,
    wasm_dir: &Path,
    fixture: &str,
    kind: FixtureKind,
) -> Result<()> {
    // cdylib crates emit the underscore name; bin crates emit the
    // hyphenated name. Try the cdylib name first, fall back to the bin name.
    let wasm_name = format!("{}.wasm", fixture.replace('-', "_"));
    let cdylib = artifact_dir.join(&wasm_name);
    let bin = artifact_dir.join(format!("{fixture}.wasm"));
    let wasm_path = if cdylib.exists() {
        cdylib
    } else if bin.exists() {
        bin
    } else {
        bail!(
            "no wasm artifact for fixture {fixture} in {}",
            artifact_dir.display()
        );
    };

    let dest = wasm_dir.join(&wasm_name);
    match kind {
        FixtureKind::P2 => {
            fs::copy(&wasm_path, &dest).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    wasm_path.display(),
                    dest.display()
                )
            })?;
        }
        FixtureKind::P3 => {
            let core = fs::read(&wasm_path)
                .with_context(|| format!("failed to read {}", wasm_path.display()))?;
            let component = componentize(&core, WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER)
                .with_context(|| format!("failed to componentize {fixture}"))?;
            fs::write(&dest, component)
                .with_context(|| format!("failed to write {}", dest.display()))?;
        }
    }
    Ok(())
}

/// Wrap a `wasm32-wasip1` core module with the WASI reactor adapter to
/// produce a component. The adapter is pinned by the
/// `wasi-preview1-component-adapter-provider` dep alongside our wasmtime
/// version, so its ABI stays in lockstep.
fn componentize(core_module: &[u8], adapter: &[u8]) -> Result<Vec<u8>> {
    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(core_module)
        .context("failed to set module")?
        .adapter("wasi_snapshot_preview1", adapter)
        .context("failed to set adapter")?
        .encode()
        .context("failed to encode component")
}

/// Copy every `*.wit` file from each `{pkg}/` subdirectory of
/// `shared_wit_dir` into the fixture's `wit/deps/{pkg}/`. Source dir names
/// already include the version (e.g. `wasi-http-0.2.2`), matching the
/// layout wit-bindgen expects.
///
/// We copy the full set of `.wit` files (not just `package.wit`) because
/// some packages — notably `wasi-tls@0.3.0-draft` — split their definitions
/// across multiple files (`client.wit`, `types.wit`, `world.wit`) instead
/// of a single bundled `package.wit`.
fn copy_shared_wit_deps(shared_wit_dir: &Path, fixture_dir: &Path) -> Result<()> {
    let deps_dir = fixture_dir.join("wit/deps");
    // Clear stale deps before re-staging
    if deps_dir.exists() {
        fs::remove_dir_all(&deps_dir)
            .with_context(|| format!("failed to clear stale {}", deps_dir.display()))?;
    }
    fs::create_dir_all(&deps_dir)?;

    for entry in fs::read_dir(shared_wit_dir)
        .with_context(|| format!("failed to read {}", shared_wit_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let wit_files: Vec<_> = fs::read_dir(entry.path())?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|f| f.path().extension().and_then(|s| s.to_str()) == Some("wit"))
            .collect();
        if wit_files.is_empty() {
            continue;
        }
        let dest_dir = deps_dir.join(entry.file_name());
        fs::create_dir_all(&dest_dir)?;
        for src in wit_files {
            fs::copy(src.path(), dest_dir.join(src.file_name()))?;
        }
    }

    Ok(())
}

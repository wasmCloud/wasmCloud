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

mod e2e_images;

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
    /// Build the runtime-operator e2e fixture components, deploy the in-cluster
    /// oci-registry, and push the fixtures into it. Configured via env
    /// (E2E_IMAGES_MODE, E2E_FIXTURES_DIR); see the e2e_images module.
    E2eImages,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let workspace = workspace_dir().context("failed to locate workspace root")?;
    match cli.task {
        Task::BuildFixtures => build_fixtures(&workspace),
        Task::E2eImages => e2e_images::run(&workspace),
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
pub(crate) enum FixtureKind {
    P2,
    P3,
}

impl FixtureKind {
    pub(crate) fn target(self) -> &'static str {
        match self {
            FixtureKind::P2 => "wasm32-wasip2",
            FixtureKind::P3 => "wasm32-wasip1",
        }
    }
}

const P2_FIXTURES: &[&str] = &[
    "http-handler-p2",
    "http-counter",
    "cron-service",
    "cron-component",
    "http-blobstore",
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
    "svc-counter",
    "svc-http-proxy",
    "svc-tcp-echo",
    "http-loopback-gateway",
    "msg-counter",
    "bridge-backend",
    "bridge-service",
    "http-webgpu",
    "kv-plugin",
    "kv-plugin-caller",
];

fn build_fixtures(workspace: &Path) -> Result<()> {
    let fixtures_dir = workspace.join("crates/wash-runtime/tests/fixtures");
    let wasm_dir = workspace.join("crates/wash-runtime/tests/wasm");

    if !fixtures_dir.exists() {
        bail!("no fixtures dir found at {}", fixtures_dir.display());
    }
    fs::create_dir_all(&wasm_dir)
        .with_context(|| format!("failed to create {}", wasm_dir.display()))?;

    let wash = ensure_wash(workspace)?;

    let fixtures = P2_FIXTURES
        .iter()
        .map(|f| (*f, FixtureKind::P2))
        .chain(P3_FIXTURES.iter().map(|f| (*f, FixtureKind::P3)));
    let mut count = 0;
    for (fixture, kind) in fixtures {
        build_and_stage(&wash, &fixtures_dir, &wasm_dir, fixture, kind)
            .with_context(|| format!("failed to build fixture {fixture}"))?;
        count += 1;
    }

    println!("staged {count} fixtures into {}", wasm_dir.display());
    Ok(())
}

/// Locate the `wash` binary. Fixtures build through `wash build`, which fetches
/// each fixture's WIT deps from its `wkg.toml` local file refs, runs its
/// `cargo build`, and wraps a wasip1 core module into a component. Build `wash`
/// if no compiled copy is present.
pub(crate) fn ensure_wash(workspace: &Path) -> Result<PathBuf> {
    // `cargo build -p wash` writes under CARGO_TARGET_DIR when the environment
    // sets one (the bench hosts point it outside the workspace so the cargo
    // cache survives `actions/checkout --clean`), and under `<workspace>/target`
    // otherwise. Resolve the binary against the same directory cargo uses, or
    // the built `wash` is looked for at a path nothing was written to.
    // Join against workspace so a relative CARGO_TARGET_DIR resolves the same
    // way cargo resolves it (cargo runs below with current_dir = workspace); an
    // absolute value replaces the base, matching cargo too.
    let target_dir = workspace.join(
        std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target")),
    );
    for profile in ["release", "debug"] {
        let candidate = target_dir.join(profile).join("wash");
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    println!("building wash…");
    let status = Command::new("cargo")
        .args(["build", "-p", "wash"])
        .current_dir(workspace)
        .status()
        .context("failed to execute cargo build -p wash")?;
    if !status.success() {
        bail!("failed to build the wash binary");
    }
    Ok(target_dir.join("debug").join("wash"))
}

/// Build one fixture through `wash build` and stage the resulting component
/// under `tests/wasm/`.
fn build_and_stage(
    wash: &Path,
    fixtures_dir: &Path,
    wasm_dir: &Path,
    fixture: &str,
    kind: FixtureKind,
) -> Result<()> {
    let fixture_dir = fixtures_dir.join(fixture);
    if !fixture_dir.exists() {
        bail!("fixture directory {} does not exist", fixture_dir.display());
    }

    let status = Command::new(wash)
        .args(["-C", &fixture_dir.to_string_lossy(), "build"])
        // The bench hosts set a job-wide CARGO_TARGET_DIR; drop it so the nested
        // build lands in `fixtures_dir/target`, where staging looks below.
        .env_remove("CARGO_TARGET_DIR")
        .status()
        .with_context(|| format!("failed to run wash build for {fixture}"))?;
    if !status.success() {
        bail!("wash build failed for {fixture}");
    }

    // cdylib crates emit the underscore name; bin crates emit the hyphenated
    // name. Try the cdylib name first, fall back to the bin name.
    let artifact_dir = fixtures_dir.join(format!("target/{}/release", kind.target()));
    let staged_name = format!("{}.wasm", fixture.replace('-', "_"));
    let cdylib = artifact_dir.join(&staged_name);
    let bin = artifact_dir.join(format!("{fixture}.wasm"));
    let built = if cdylib.exists() {
        cdylib
    } else if bin.exists() {
        bin
    } else {
        bail!(
            "no wasm artifact for fixture {fixture} in {}",
            artifact_dir.display()
        );
    };

    fs::copy(&built, wasm_dir.join(&staged_name))
        .with_context(|| format!("failed to stage {fixture}"))?;
    Ok(())
}

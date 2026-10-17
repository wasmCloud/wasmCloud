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
mod fixtures;
mod protos;

use fixtures::{P2_FIXTURES, P3_FIXTURES};

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
    BuildFixtures {
        /// Build only these fixtures (by directory name); all of them if none.
        names: Vec<String>,
    },
    /// Regenerate the committed protobuf bindings in
    /// `crates/wash-runtime/src/washlet/generated/` from
    /// `/proto/wasmcloud/runtime/v2`. CI fails if they drift.
    GenerateProtos,
    /// Build the runtime-operator e2e fixture components, deploy the in-cluster
    /// oci-registry, and push the fixtures into it. Configured via env
    /// (E2E_IMAGES_MODE, E2E_FIXTURES_DIR); see the e2e_images module.
    E2eImages,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let workspace = workspace_dir().context("failed to locate workspace root")?;
    match cli.task {
        Task::BuildFixtures { names } => build_fixtures(&workspace, &names),
        Task::E2eImages => e2e_images::run(&workspace),
        Task::GenerateProtos => protos::run(&workspace),
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

fn build_fixtures(workspace: &Path, names: &[String]) -> Result<()> {
    let fixtures_dir = workspace.join("crates/wash-runtime/tests/fixtures");
    let wasm_dir = workspace.join("crates/wash-runtime/tests/wasm");

    if !fixtures_dir.exists() {
        bail!("no fixtures dir found at {}", fixtures_dir.display());
    }
    fs::create_dir_all(&wasm_dir)
        .with_context(|| format!("failed to create {}", wasm_dir.display()))?;

    let known = |name: &str| P2_FIXTURES.contains(&name) || P3_FIXTURES.contains(&name);
    if let Some(unknown) = names.iter().find(|n| !known(n)) {
        bail!("unknown fixture {unknown}");
    }

    let wash = ensure_wash(workspace)?;
    let linker = wasip2_linker(workspace)?;

    let fixtures = P2_FIXTURES
        .iter()
        .map(|f| (*f, FixtureKind::P2))
        .chain(P3_FIXTURES.iter().map(|f| (*f, FixtureKind::P3)))
        .filter(|(f, _)| names.is_empty() || names.iter().any(|n| n == f));
    let mut count = 0;
    for (fixture, kind) in fixtures {
        build_and_stage(
            &wash,
            linker.as_deref(),
            &fixtures_dir,
            &wasm_dir,
            fixture,
            kind,
        )
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
    let target_dir = target_dir(workspace);
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

fn target_dir(workspace: &Path) -> PathBuf {
    workspace.join(
        std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target")),
    )
}

/// The standalone `wasm-component-ld` CI links wasm32-wasip2 fixtures with.
/// Keep in step with `wasm-component-ld-version` in
/// `.github/actions/setup-rust/action.yml`.
const WASM_COMPONENT_LD_VERSION: &str = "0.5.24";

/// The oldest `wasm-component-ld` that can decode the labeled (`implements`)
/// imports wit-bindgen emits. Older ones fail to link such a fixture with
/// "invalid leading byte (0x2) for import name".
const WASM_COMPONENT_LD_MIN: (u64, u64, u64) = (0, 5, 23);

/// The `wasm-component-ld` to link wasm32-wasip2 fixtures with, or `None`
/// when the one bundled with the pinned toolchain is new enough.
///
/// CI copies a newer one over the bundled one (see `setup-rust`). Locally,
/// [`WASM_COMPONENT_LD_VERSION`] is installed once under the target dir
/// instead, and handed to cargo as the wasm32-wasip2 linker, so the toolchain
/// itself is left alone.
pub(crate) fn wasip2_linker(workspace: &Path) -> Result<Option<PathBuf>> {
    let exe = format!("wasm-component-ld{}", std::env::consts::EXE_SUFFIX);
    let rustc = |arg: &[&str]| -> Result<String> {
        // From the workspace, so its rust-toolchain.toml picks the toolchain.
        let out = Command::new("rustc")
            .args(arg)
            .current_dir(workspace)
            .output()
            .context("failed to execute rustc")?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let sysroot = rustc(&["--print", "sysroot"])?;
    let host = rustc(&["-vV"])?
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        .context("rustc -vV printed no host")?;
    let bundled = Path::new(&sysroot)
        .join("lib/rustlib")
        .join(host)
        .join("bin")
        .join(&exe);
    if linker_version(&bundled).is_some_and(|v| v >= WASM_COMPONENT_LD_MIN) {
        return Ok(None);
    }

    let root = target_dir(workspace)
        .join("xtask-tools")
        .join(format!("wasm-component-ld-{WASM_COMPONENT_LD_VERSION}"));
    let installed = root.join("bin").join(&exe);
    if !installed.exists() {
        println!(
            "the toolchain's wasm-component-ld is too old for labeled imports; \
             installing wasm-component-ld {WASM_COMPONENT_LD_VERSION}…"
        );
        let status = Command::new("cargo")
            .args(["install", "--locked", "wasm-component-ld", "--version"])
            .arg(WASM_COMPONENT_LD_VERSION)
            .arg("--root")
            .arg(&root)
            .current_dir(workspace)
            .status()
            .context("failed to execute cargo install wasm-component-ld")?;
        if !status.success() {
            bail!("failed to install wasm-component-ld {WASM_COMPONENT_LD_VERSION}");
        }
    }
    Ok(Some(installed))
}

/// The version `wasm-component-ld --version` reports, or `None` when it
/// cannot be run or read.
fn linker_version(path: &Path) -> Option<(u64, u64, u64)> {
    let out = Command::new(path).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut parts = text.split_whitespace().nth(1)?.split('.');
    let mut next = || parts.next()?.parse().ok();
    Some((next()?, next()?, next()?))
}

/// Build one fixture through `wash build` and stage the resulting component
/// under `tests/wasm/`.
fn build_and_stage(
    wash: &Path,
    linker: Option<&Path>,
    fixtures_dir: &Path,
    wasm_dir: &Path,
    fixture: &str,
    kind: FixtureKind,
) -> Result<()> {
    let fixture_dir = fixtures_dir.join(fixture);
    if !fixture_dir.exists() {
        bail!("fixture directory {} does not exist", fixture_dir.display());
    }

    let mut build = Command::new(wash);
    build
        .args(["-C", &fixture_dir.to_string_lossy(), "build"])
        // The bench hosts set a job-wide CARGO_TARGET_DIR; drop it so the nested
        // build lands in `fixtures_dir/target`, where staging looks below.
        .env_remove("CARGO_TARGET_DIR");
    if let Some(linker) = linker {
        build.env("CARGO_TARGET_WASM32_WASIP2_LINKER", linker);
    }
    let status = build
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

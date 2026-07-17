//! `cargo xtask e2e-images`: build the e2e fixture components, deploy the
//! in-cluster oci-registry, and push the fixtures into it.
//!
//! The runtime-operator e2e suite invokes this from BeforeSuite after
//! `helm install` brings up the `registry` and `default` hostgroups; it can
//! also be run standalone against an installed cluster.
//!
//! `E2E_IMAGES_MODE` selects which phases run:
//!   all   (default) build the fixtures, then deploy the registry and push — the
//!                   self-contained local path.
//!   build           only build the fixture components (no cluster). A CI job
//!                   runs this and uploads the results so the e2e leg can reuse
//!                   them without rebuilding.
//!   push            skip the build; deploy the registry and push fixtures that
//!                   were already built (read from `E2E_FIXTURES_DIR`).
//!
//! `E2E_FIXTURES_DIR` (optional): a flat directory of prebuilt `<name>.wasm`
//! components plus the `wash` binary. `build` stages the outputs here; `push`
//! reads them from here instead of rebuilding.
//!
//! Reachability: the specs pull the same content from the in-cluster Service DNS
//! (`oci-registry.wasmcloud-system.svc`) — a different authority than the push
//! side, which is fine (OCI stores by repo path + tag, not by hostname). See
//! runtime-operator/test/e2e/testdata/oci-registry.yaml for why both authorities
//! must be portless.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// The fixture directories (under crates/wash-runtime/tests/fixtures) to build
/// and push. Each is a wasm32-wasip2 cdylib served at
/// <registry>/fixtures/<name>:e2e. To add a fixture: drop a wash-buildable dir,
/// add a row here, and reference `registryRef("<name>")` in a spec.
const FIXTURES: &[&str] = &[
    "messaging-handler",
    "keyvalue-implements",
    "http-handler-p2",
];

/// Fixed so it always matches the pull side (registryImageTag in
/// e2e_suite_test.go) — the two have no shared source, so this isn't a knob.
const TAG: &str = "e2e";

/// Dedicated loopback so the port-forward doesn't clash with kind's :80 mapping
/// (pinned to 127.0.0.1 in deploy/kind/kind-config.yaml). :80 keeps the Host
/// header portless so the host's exact-match router accepts it.
const PUSH_ADDR: &str = "127.0.0.2";

#[derive(Copy, Clone, PartialEq)]
enum Mode {
    Build,
    Push,
    All,
}

pub fn run(workspace: &Path) -> Result<()> {
    let mode = match env::var("E2E_IMAGES_MODE").as_deref().unwrap_or("all") {
        "build" => Mode::Build,
        "push" => Mode::Push,
        "all" => Mode::All,
        other => bail!("invalid E2E_IMAGES_MODE={other} (want build|push|all)"),
    };
    let fixtures_dir = workspace.join("crates/wash-runtime/tests/fixtures");
    let namespace = env::var("NAMESPACE").unwrap_or_else(|_| "wasmcloud-system".to_string());
    let fixtures_out = env::var("E2E_FIXTURES_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

    // The wash used to build/push. In `all` mode the build phase resolves it and
    // the push phase reuses it.
    let mut wash: Option<PathBuf> = None;

    if mode != Mode::Push {
        wash = Some(build_phase(
            workspace,
            &fixtures_dir,
            fixtures_out.as_deref(),
        )?);
        if mode == Mode::Build {
            eprintln!(">>> e2e-images: built {} fixtures", FIXTURES.len());
            return Ok(());
        }
    }

    let wash = match wash {
        Some(w) => w,
        None => push_wash(fixtures_out.as_deref())?,
    };
    push_phase(
        workspace,
        &fixtures_dir,
        &namespace,
        &wash,
        mode,
        fixtures_out.as_deref(),
    )
}

/// Build the in-repo wash and each fixture; when E2E_FIXTURES_DIR is set, stage
/// the wash binary + built components there (for a CI job to upload).
fn build_phase(
    workspace: &Path,
    fixtures_dir: &Path,
    fixtures_out: Option<&Path>,
) -> Result<PathBuf> {
    let wash = build_wash(workspace)?;

    if let Some(out) = fixtures_out {
        fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
        // Stage the wash we built so the push side (a separate CI job) reuses
        // this exact binary rather than a released wash.
        fs::copy(&wash, out.join("wash")).context("staging wash")?;
    }

    for fixture in FIXTURES {
        eprintln!(">>> e2e-images: wash build {fixture}");
        wash_build(&wash, &fixtures_dir.join(fixture))
            .with_context(|| format!("wash build {fixture}"))?;
        if let Some(out) = fixtures_out {
            let built = built_component(fixtures_dir, fixture)?;
            fs::copy(&built, out.join(component_name(fixture)))
                .with_context(|| format!("staging {fixture}"))?;
        }
    }
    Ok(wash)
}

/// Deploy the registry, wait until it serves, port-forward it on a loopback,
/// and push every fixture.
fn push_phase(
    workspace: &Path,
    fixtures_dir: &Path,
    namespace: &str,
    wash: &Path,
    mode: Mode,
    fixtures_out: Option<&Path>,
) -> Result<()> {
    let manifest = workspace.join("runtime-operator/test/e2e/testdata/oci-registry.yaml");
    // Resolve the kubeconfig up front so the sudo'd port-forward (root, different
    // HOME) still targets this cluster.
    let kubeconfig = env::var("KUBECONFIG")
        .unwrap_or_else(|_| format!("{}/.kube/config", env::var("HOME").unwrap_or_default()));

    eprintln!(">>> e2e-images: deploying oci-registry");
    kubectl(&kubeconfig, &["apply", "-f", &manifest.to_string_lossy()])?;
    kubectl(
        &kubeconfig,
        &[
            "wait",
            "--for=condition=Ready",
            "--timeout=5m",
            "-n",
            namespace,
            "workloaddeployment/oci-registry",
        ],
    )?;

    // macOS only creates 127.0.0.1 by default; alias the loopback we forward to.
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("sudo")
            .args(["ifconfig", "lo0", "alias", PUSH_ADDR, "up"])
            .status();
    }

    eprintln!(">>> e2e-images: port-forwarding deployment/hostgroup-registry -> {PUSH_ADDR}:80");
    let _pf = PortForward::start(&kubeconfig, namespace)?;
    wait_for_registry()?;

    for fixture in FIXTURES {
        let component = match (mode, fixtures_out) {
            (Mode::Push, Some(out)) => out.join(component_name(fixture)),
            _ => built_component(fixtures_dir, fixture)?,
        };
        let reference = format!("{PUSH_ADDR}/fixtures/{fixture}:{TAG}");
        eprintln!(">>> e2e-images: wash oci push {reference}");
        run_checked(
            Command::new(wash).args([
                "oci",
                "push",
                "--insecure",
                &reference,
                &component.to_string_lossy(),
            ]),
            "wash oci push",
        )?;
    }

    eprintln!(">>> e2e-images: pushed {} fixtures", FIXTURES.len());
    Ok(())
}

/// The in-repo wash to build fixtures with. WASH overrides it; otherwise build
/// it debug (matches `cargo xtask build-fixtures`; the released wash can't build
/// fixtures from their local wkg.toml refs).
///
/// TODO(wash release): once a released wash can `wash build` these fixtures from
/// their local wkg.toml refs, use it (setup-wash-action) and drop this build +
/// the wash staging + the protoc/setup-rust the e2e-fixtures job carries for it.
fn build_wash(workspace: &Path) -> Result<PathBuf> {
    if let Some(w) = env::var("WASH").ok().filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(w));
    }
    // Reuse the shared resolver so this honors CARGO_TARGET_DIR (and reuses an
    // existing build) exactly like `cargo xtask build-fixtures`.
    crate::ensure_wash(workspace)
}

/// The wash to push with (push-only mode). Prefer the staged in-repo wash in
/// E2E_FIXTURES_DIR (downloaded from the build job), then WASH, then PATH.
fn push_wash(fixtures_out: Option<&Path>) -> Result<PathBuf> {
    if let Some(w) = env::var("WASH").ok().filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(w));
    }
    if let Some(out) = fixtures_out {
        let staged = out.join("wash");
        if staged.is_file() {
            // Artifact download can drop the exec bit; restore it.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&staged)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&staged, perms)?;
            }
            return Ok(staged);
        }
    }
    // Fall back to a wash on PATH (a released wash can push).
    Ok(PathBuf::from("wash"))
}

fn wash_build(wash: &Path, fixture_dir: &Path) -> Result<()> {
    if !fixture_dir.exists() {
        bail!("fixture directory {} does not exist", fixture_dir.display());
    }
    run_checked(
        Command::new(wash)
            .args(["-C", &fixture_dir.to_string_lossy(), "build"])
            // The bench hosts set a job-wide CARGO_TARGET_DIR; drop it so the
            // nested build lands in fixtures/target, where the component lookup
            // below expects it.
            .env_remove("CARGO_TARGET_DIR"),
        "wash build",
    )
}

/// The built component filename: the wasm32-wasip2 cdylib underscore name.
fn component_name(fixture: &str) -> String {
    format!("{}.wasm", fixture.replace('-', "_"))
}

/// The built component path for a fixture (all e2e fixtures are wasm32-wasip2).
fn built_component(fixtures_dir: &Path, fixture: &str) -> Result<PathBuf> {
    let path = fixtures_dir
        .join("target/wasm32-wasip2/release")
        .join(component_name(fixture));
    if !path.exists() {
        bail!("no wasm artifact for {fixture} at {}", path.display());
    }
    Ok(path)
}

fn kubectl(kubeconfig: &str, args: &[&str]) -> Result<()> {
    run_checked(
        Command::new("kubectl")
            .arg("--kubeconfig")
            .arg(kubeconfig)
            .args(args),
        "kubectl",
    )
}

fn run_checked(cmd: &mut Command, what: &str) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("failed to run {what}"))?;
    if !status.success() {
        bail!("{what} failed with {status}");
    }
    Ok(())
}

/// Wait until the registry answers `/v2/` through the port-forward.
fn wait_for_registry() -> Result<()> {
    let url = format!("http://{PUSH_ADDR}/v2/");
    eprintln!(">>> e2e-images: waiting for the registry API on {url}");
    for attempt in 1..=30 {
        let ok = Command::new("curl")
            .args(["-fsS", &url])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }
        if attempt == 30 {
            bail!("registry never answered /v2/ through the port-forward");
        }
        sleep(Duration::from_secs(2));
    }
    Ok(())
}

/// A `sudo kubectl port-forward` running in the background, killed on drop.
///
/// Forwards to the registry hostgroup pod (via its Deployment), not the Service:
/// the oci-registry Service is selectorless (the operator manages its route
/// EndpointSlice), so `kubectl port-forward svc/...` can't resolve a target pod.
/// The pod's HTTP server demuxes by Host header, and PUSH_ADDR (:80, portless)
/// is a registered alias, so this reaches the registry all the same. The Service
/// remains the in-cluster pull path.
///
/// TODO(wash release): the :80 + dedicated-loopback + sudo dance exists only
/// because the host's HTTP router matches the Host header exactly and rejects a
/// host that carries a port. Once the host matches on host-without-port (or the
/// registry gets ingress that isn't Host-demuxed), forward a normal Service port
/// and drop the loopback + sudo.
struct PortForward {
    child: Child,
}

impl PortForward {
    fn start(kubeconfig: &str, namespace: &str) -> Result<Self> {
        let child = Command::new("sudo")
            .args([
                "kubectl",
                "--kubeconfig",
                kubeconfig,
                "port-forward",
                "--address",
                PUSH_ADDR,
                "-n",
                namespace,
                "deployment/hostgroup-registry",
                "80:80",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("starting kubectl port-forward (needs sudo to bind :80)")?;
        Ok(Self { child })
    }
}

impl Drop for PortForward {
    fn drop(&mut self) {
        // $! is the sudo pid; sudo relays SIGTERM to kubectl. pkill is a backstop
        // in case it doesn't.
        let _ = Command::new("sudo")
            .args(["kill", &self.child.id().to_string()])
            .status();
        let _ = Command::new("sudo")
            .args(["pkill", "-f", "port-forward.*hostgroup-registry"])
            .status();
        let _ = self.child.wait();
    }
}

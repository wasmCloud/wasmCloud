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
//! side, which is fine (OCI stores by repo path + tag, not by hostname). Both
//! authorities are registered on the registry workload; see
//! runtime-operator/test/e2e/testdata/oci-registry.yaml.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::FixtureKind;

/// The fixture directories (under crates/wash-runtime/tests/fixtures) to build
/// and push, each paired with its component-model preview — `FixtureKind` drives
/// the build target triple (and thus where `wash build` leaves the component).
/// Served at <registry>/fixtures/<name>:e2e. To add a fixture: drop a
/// wash-buildable dir, add a row here, and reference `registryRef("<name>")` in a
/// spec.
const FIXTURES: &[(&str, FixtureKind)] = &[
    ("messaging-echo", FixtureKind::P2),
    ("keyvalue-implements", FixtureKind::P2),
    ("http-handler-p2", FixtureKind::P2),
    // Host component plugin fixtures (P3 async): kv-plugin serves acme:kv/store
    // from its own supervised store; kv-plugin-caller imports it over HTTP.
    ("kv-plugin", FixtureKind::P3),
    ("kv-plugin-caller", FixtureKind::P3),
    // Reports, per instance, the peak calls it had in flight and how many it
    // has served — which is what makes poolSize/maxConcurrency/maxInvocations
    // observable from outside the cluster.
    ("http-sleeper", FixtureKind::P3),
];

/// Fixed so it always matches the pull side (registryImageTag in
/// e2e_suite_test.go) — the two have no shared source, so this isn't a knob.
const TAG: &str = "e2e";

/// Defaults for the registry's HTTP Basic credentials, overridable with
/// `E2E_REGISTRY_USER` / `E2E_REGISTRY_PASSWORD`. The suite reads the same two
/// variables (see e2e_suite_test.go), so setting them moves both sides at once
/// — which is what a credential shared across processes needs. Both carry
/// `test`: they are throwaway credentials for a registry that lives as long as
/// one test run, and should read that way wherever they surface.
///
/// This is where they are defined. The Secrets holding them are created from
/// here (see [`create_registry_secrets`]) rather than checked in beside the
/// registry manifest, where they could not follow the environment.
const DEFAULT_REGISTRY_USER: &str = "test-e2e-user";
const DEFAULT_REGISTRY_PASSWORD: &str = "test-e2e-password";

fn registry_user() -> String {
    env_or("E2E_REGISTRY_USER", DEFAULT_REGISTRY_USER)
}

fn registry_password() -> String {
    env_or("E2E_REGISTRY_PASSWORD", DEFAULT_REGISTRY_PASSWORD)
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Where the registry is port-forwarded for pushing.
///
/// Plain loopback, and [`free_local_port`] picks the port, so this needs no
/// root: binding :80 does, and every address but 127.0.0.1 has to be aliased
/// onto the loopback first on macOS. Both were once required because the
/// host's router matched the Host header including its port, so the registry
/// had to answer on the default port for clients to omit it; the router now
/// matches the name alone.
const PUSH_ADDR: &str = "127.0.0.1";

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

    for &(fixture, kind) in FIXTURES {
        eprintln!(">>> e2e-images: wash build {fixture}");
        wash_build(&wash, &fixtures_dir.join(fixture))
            .with_context(|| format!("wash build {fixture}"))?;
        if let Some(out) = fixtures_out {
            let built = built_component(fixtures_dir, fixture, kind)?;
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
    let kubeconfig = env::var("KUBECONFIG")
        .unwrap_or_else(|_| format!("{}/.kube/config", env::var("HOME").unwrap_or_default()));

    eprintln!(">>> e2e-images: creating registry credentials");
    create_registry_secrets(&kubeconfig, namespace)?;

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

    let ca_bundle = fetch_ca_bundle(&kubeconfig, namespace, workspace)?;
    let port = free_local_port()?;
    eprintln!(
        ">>> e2e-images: port-forwarding deployment/hostgroup-registry -> {PUSH_ADDR}:{port}"
    );
    let _pf = PortForward::start(&kubeconfig, namespace, port)?;
    wait_for_registry(port, &ca_bundle)?;

    let (user, password) = (registry_user(), registry_password());
    for &(fixture, kind) in FIXTURES {
        let component = match (mode, fixtures_out) {
            (Mode::Push, Some(out)) => out.join(component_name(fixture)),
            _ => built_component(fixtures_dir, fixture, kind)?,
        };
        let reference = format!("{PUSH_ADDR}:{port}/fixtures/{fixture}:{TAG}");
        eprintln!(">>> e2e-images: wash oci push {reference}");
        run_checked(
            Command::new(wash).args([
                "oci",
                "push",
                "--ca-path",
                &ca_bundle.to_string_lossy(),
                "--user",
                &user,
                "--password",
                &password,
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
    // Fall back to a wash on PATH. It has to be recent enough to carry
    // `oci push --ca-path`: the registry serves TLS from the chart's own CA,
    // and a wash without the flag cannot be told to trust it.
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

/// The built component filename: the fixture crate's underscore name.
fn component_name(fixture: &str) -> String {
    format!("{}.wasm", fixture.replace('-', "_"))
}

/// The built component path for a fixture. `wash build` leaves the component in
/// place under the release dir of the kind's target triple — a component
/// directly for P2, the reactor-wrapped core module for P3.
fn built_component(fixtures_dir: &Path, fixture: &str, kind: FixtureKind) -> Result<PathBuf> {
    let path = fixtures_dir
        .join(format!("target/{}/release", kind.target()))
        .join(component_name(fixture));
    if !path.exists() {
        bail!("no wasm artifact for {fixture} at {}", path.display());
    }
    Ok(path)
}

/// Create the two Secrets the registry flow needs, replacing whatever an
/// earlier run left so a changed credential actually takes effect.
///
/// `oci-registry-auth` is what the registry checks requests against: the
/// operator materializes it into the `wasmcloud:secrets` config its component
/// reads. `oci-registry-pull` is what a hostgroup presents when pulling a
/// fixture back out, keyed by the registry domain `registryRef` builds refs on.
///
/// Built here rather than checked in beside the registry manifest for two
/// reasons: a manifest cannot pick up `E2E_REGISTRY_USER`/`_PASSWORD`, and
/// `kubectl create secret` owns the encoding — a docker config committed as
/// base64 hides the very credential that has to match the one above it.
fn create_registry_secrets(kubeconfig: &str, namespace: &str) -> Result<()> {
    let (user, password) = (registry_user(), registry_password());
    for name in ["oci-registry-auth", "oci-registry-pull"] {
        kubectl(
            kubeconfig,
            &[
                "delete",
                "secret",
                name,
                "-n",
                namespace,
                "--ignore-not-found",
            ],
        )?;
    }
    // Key names are the config keys the registry asks its secrets backend for.
    kubectl(
        kubeconfig,
        &[
            "create",
            "secret",
            "generic",
            "oci-registry-auth",
            "-n",
            namespace,
            &format!("--from-literal=registry-username={user}"),
            &format!("--from-literal=registry-password={password}"),
        ],
    )?;
    kubectl(
        kubeconfig,
        &[
            "create",
            "secret",
            "docker-registry",
            "oci-registry-pull",
            "-n",
            namespace,
            &format!("--docker-server=oci-registry.{namespace}.svc"),
            &format!("--docker-username={user}"),
            &format!("--docker-password={password}"),
        ],
    )
}

/// The chart's CA secret, whose certificate signs the registry's serving cert.
/// Matches `global.certificates.caSecretName` in the chart's values.
const CA_SECRET: &str = "wasmcloud-ca";

/// Write the chart's CA certificate to a file the push side can point
/// `wash oci push --ca-path` at.
///
/// The hosts get this CA from a mounted Secret; the pushing process runs
/// outside the cluster, so it has to read it out. Without it the push cannot
/// verify the registry — the CA is generated per install and signs nothing the
/// public roots know about.
fn fetch_ca_bundle(kubeconfig: &str, namespace: &str, workspace: &Path) -> Result<PathBuf> {
    // A go-template decodes the Secret's base64 in kubectl, which a jsonpath
    // cannot do.
    let pem = kubectl_output(
        kubeconfig,
        &[
            "get",
            "secret",
            CA_SECRET,
            "-n",
            namespace,
            "-o",
            r#"go-template={{index .data "tls.crt" | base64decode}}"#,
        ],
    )
    .with_context(|| format!("reading the {CA_SECRET} secret"))?;
    if !pem.contains("BEGIN CERTIFICATE") {
        bail!("{CA_SECRET} did not contain a PEM certificate");
    }
    // Under the workspace's target dir rather than the shared temp dir, so
    // concurrent runs on one machine cannot overwrite each other's CA.
    let path = workspace.join("target/e2e-registry-ca.pem");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&path, pem).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn kubectl_output(kubeconfig: &str, args: &[&str]) -> Result<String> {
    let output = Command::new("kubectl")
        .arg("--kubeconfig")
        .arg(kubeconfig)
        .args(args)
        .output()
        .context("failed to run kubectl")?;
    if !output.status.success() {
        bail!(
            "kubectl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("kubectl produced non-utf8 output")
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

/// A free TCP port on the push address, for the port-forward to claim.
///
/// Asking the OS beats hardcoding one: a fixed port collides with whatever the
/// developer happens to be running, and on macOS the obvious candidates are
/// already taken — AirPlay Receiver listens on 5000 and 7000 by default and
/// answers requests, so a registry that never came up looks like one serving
/// 403s instead. Between the probe closing and kubectl binding, the port could
/// in principle be taken; nothing else here is racing for it.
fn free_local_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind((PUSH_ADDR, 0))
        .with_context(|| format!("finding a free port on {PUSH_ADDR}"))?;
    let port = listener
        .local_addr()
        .context("reading the probe socket's port")?
        .port();
    Ok(port)
}

/// Wait until the registry answers `/v2/` through the port-forward.
fn wait_for_registry(port: u16, ca_bundle: &Path) -> Result<()> {
    let url = format!("https://{PUSH_ADDR}:{port}/v2/");
    eprintln!(">>> e2e-images: waiting for the registry API on {url}");
    for attempt in 1..=30 {
        let ok = Command::new("curl")
            // `/v2/` is authenticated like everything else, so an unauthenticated
            // probe would wait out all 30 attempts on a registry that is up and
            // answering 401.
            .args([
                "-fsS",
                "--cacert",
                &ca_bundle.to_string_lossy(),
                "-u",
                &format!("{}:{}", registry_user(), registry_password()),
                &url,
            ])
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

/// A `kubectl port-forward` running in the background, killed on drop.
///
/// Forwards to the registry hostgroup pod (via its Deployment), not the Service:
/// the oci-registry Service is selectorless (the operator manages its route
/// EndpointSlice), so `kubectl port-forward svc/...` can't resolve a target pod.
/// The pod's HTTP server demuxes by Host header, and PUSH_ADDR is a registered
/// alias — the port the client tacks on does not affect that match — so this
/// reaches the registry all the same. The Service remains the in-cluster pull
/// path.
struct PortForward {
    child: Child,
}

impl PortForward {
    fn start(kubeconfig: &str, namespace: &str, port: u16) -> Result<Self> {
        let child = Command::new("kubectl")
            .args([
                "--kubeconfig",
                kubeconfig,
                "port-forward",
                "--address",
                PUSH_ADDR,
                "-n",
                namespace,
                "deployment/hostgroup-registry",
                &format!("{port}:80"),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("starting kubectl port-forward")?;
        Ok(Self { child })
    }
}

impl Drop for PortForward {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

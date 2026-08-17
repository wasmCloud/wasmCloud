//! `wizard-verify`: prove that everything `wash wizard` generates builds with
//! zero warnings, boots under `wash dev`, and re-derives as the shape asked for.
//! Runs on demand, not CI: each case compiles a wasm workspace and boots a host.

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use wash_topology::Shape;

/// How long to wait for `wash dev` to build and register the workload.
const BOOT_TIMEOUT: Duration = Duration::from_secs(120);

/// Post-build settle time — long enough for a link failure to reach the log.
const SETTLE: Duration = Duration::from_secs(3);

/// Lines meaning the host rejected the workload; any of these is a failure.
const FAILURE_MARKERS: [&str; 4] = [
    "not found in the linker",
    "failed to reload component",
    "failed to resolve",
    "unbinding all plugins",
];

/// One generated shape to prove out.
struct Case {
    /// Also the generated project name, so a failure names a real directory.
    name: &'static str,
    trigger: &'static str,
    linking: &'static str,
    count: usize,
    /// Explicit per-branch depths, for a lopsided shape.
    branches: &'static [usize],
    edition: &'static str,
    egress: Option<&'static str>,
    grpc: Option<&'static str>,
    capabilities: &'static [&'static str],
    /// `--place` arguments as `NODE=CAP,...`; once any is given it is the whole story.
    place: &'static [&'static str],
    /// `(path, needle)` pairs a placement case requires in the emitted source.
    expect_in: &'static [(&'static str, &'static str)],
    /// The same, inverted: a component that must *not* have got something.
    expect_not_in: &'static [(&'static str, &'static str)],
    /// What the topology deriver must independently conclude this is.
    expect_shape: Shape,
    /// `unresolved` reasons that are correct for this shape rather than a defect.
    allow_unresolved: &'static [&'static str],
    /// Set when the case cannot boot here for a reason outside the generator.
    blocked: Option<Blocked>,
}

impl Case {
    const fn new(name: &'static str, trigger: &'static str, linking: &'static str) -> Self {
        Case {
            name,
            trigger,
            linking,
            count: 2,
            branches: &[],
            edition: "p2",
            egress: None,
            grpc: None,
            capabilities: &[],
            place: &[],
            expect_in: &[],
            expect_not_in: &[],
            expect_shape: Shape::Single,
            allow_unresolved: &[],
            blocked: None,
        }
    }
}

fn cases() -> Vec<Case> {
    vec![
        // --- trigger x linking -------------------------------------------
        Case {
            expect_shape: Shape::Single,
            ..Case::new("http-none", "http", "none")
        },
        Case {
            expect_shape: Shape::Chain,
            ..Case::new("http-chain", "http", "chain")
        },
        Case {
            count: 3,
            expect_shape: Shape::FanOut,
            ..Case::new("http-fanout", "http", "fan-out")
        },
        Case {
            expect_shape: Shape::FanOut,
            allow_unresolved: &["dynamic-publish-subject"],
            // Re-tested after the messaging@0.3.0 rebase (2026-08): still real.
            blocked: Some(Blocked(
                "wasmcloud:messaging/consumer does not link on this workspace (the \
                 shipped distributed-workloads template fails identically)",
            )),
            ..Case::new("http-messaging", "http", "messaging")
        },
        Case {
            // A worker with no HTTP export at all: nothing could curl this.
            expect_shape: Shape::Single,
            ..Case::new("messaging-none", "messaging", "none")
        },
        Case {
            expect_shape: Shape::Chain,
            ..Case::new("messaging-chain", "messaging", "chain")
        },
        Case {
            // Services are p3-only: a cdylib exporting `wasi:cli/run@0.3.0`.
            edition: "p3",
            expect_shape: Shape::Service,
            ..Case::new("service-none", "service", "none")
        },
        // --- one per capability ------------------------------------------
        Case {
            capabilities: &["keyvalue"],
            ..Case::new("cap-keyvalue", "http", "none")
        },
        Case {
            capabilities: &["blobstore"],
            ..Case::new("cap-blobstore", "http", "none")
        },
        Case {
            capabilities: &["config"],
            ..Case::new("cap-config", "http", "none")
        },
        Case {
            capabilities: &["logging"],
            ..Case::new("cap-logging", "http", "none")
        },
        Case {
            capabilities: &["postgres"],
            // The host connects to postgres eagerly at startup; it still must build and derive.
            blocked: Some(Blocked(
                "wasmcloud:postgres needs a running database to boot",
            )),
            ..Case::new("cap-postgres", "http", "none")
        },
        Case {
            capabilities: &["otel"],
            ..Case::new("cap-otel", "http", "none")
        },
        Case {
            egress: Some("example.com"),
            ..Case::new("cap-egress", "http", "none")
        },
        Case {
            grpc: Some("grpc.example.com"),
            ..Case::new("cap-grpc", "http", "none")
        },
        Case {
            // gRPC from a synchronous entry point, driven with block_on.
            grpc: Some("grpc.example.com"),
            ..Case::new("grpc-messaging", "messaging", "none")
        },
        // No grpc-service case: services are p3-only, the gRPC scaffold p2-only; `validate()` refuses the pairing.
        Case {
            // The async `wasmcloud:keyvalue@0.2.0` package over the default multiplexed backend.
            edition: "p3",
            capabilities: &["keyvalue"],
            ..Case::new("cap-keyvalue-p3", "http", "none")
        },
        Case {
            // Secrets: handle-then-reveal, values from the generated `dev.host_interfaces` entry.
            edition: "p3",
            capabilities: &["secrets"],
            ..Case::new("cap-secrets", "http", "none")
        },
        Case {
            // GPU adapter presence; a headless host answers 200 with "no adapter".
            edition: "p3",
            capabilities: &["webgpu"],
            ..Case::new("cap-webgpu", "http", "none")
        },
        Case {
            // A p3 messaging worker; nothing curls it, so generation + build is the gate.
            edition: "p3",
            expect_shape: Shape::Single,
            ..Case::new("p3-messaging", "messaging", "none")
        },
        Case {
            // p3 fan-out over the broker; boot hits the same missing-consumer wall as http-messaging.
            edition: "p3",
            expect_shape: Shape::FanOut,
            allow_unresolved: &["dynamic-publish-subject"],
            blocked: Some(Blocked(
                "wasmcloud:messaging/consumer does not link on this workspace (the \
                 shipped distributed-workloads template fails identically)",
            )),
            ..Case::new("p3-fanout-messaging", "http", "messaging")
        },
        Case {
            // Lopsided: three branches, two/one/three deep — inexpressible as a single `Linking`.
            branches: &[2, 1, 3],
            expect_shape: Shape::FanOut,
            ..Case::new("lopsided", "http", "fan-out")
        },
        Case {
            edition: "p3",
            expect_shape: Shape::Single,
            ..Case::new("p3-none", "http", "none")
        },
        // --- per-component placement -------------------------------------
        Case {
            // A capability on a middle hop, which the default placement could never reach.
            count: 3,
            grpc: Some("grpc.example.com"),
            capabilities: &["logging"],
            place: &["step2=grpc", "step3=logging"],
            // A synchronous `invoke` export driving the async gRPC adapter mid-chain.
            expect_in: &[
                ("step2/src/lib.rs", "block_on"),
                ("step3/src/lib.rs", "logging"),
            ],
            expect_not_in: &[("step1/src/lib.rs", "block_on")],
            expect_shape: Shape::Chain,
            ..Case::new("place-midchain-grpc", "http", "chain")
        },
        Case {
            // Two branches given different capabilities — undescribable as a flat list.
            count: 2,
            capabilities: &["keyvalue", "blobstore"],
            place: &["branch1=keyvalue", "branch2=blobstore"],
            expect_in: &[
                ("branch1/src/lib.rs", "keyvalue"),
                ("branch2/src/lib.rs", "blobstore"),
            ],
            expect_not_in: &[
                ("branch1/src/lib.rs", "blobstore"),
                ("branch2/src/lib.rs", "keyvalue"),
            ],
            expect_shape: Shape::FanOut,
            ..Case::new("place-split-fanout", "http", "fan-out")
        },
        Case {
            // An empty terminal plus a loaded trigger; the empty one trips `unused_mut` under deny-warnings.
            count: 2,
            capabilities: &["logging"],
            place: &["ingress=logging"],
            expect_in: &[("ingress/src/lib.rs", "logging")],
            expect_not_in: &[("step2/src/lib.rs", "logging")],
            expect_shape: Shape::Chain,
            ..Case::new("place-empty-terminal", "http", "chain")
        },
        // --- everything at once ------------------------------------------
        Case {
            count: 2,
            egress: Some("example.com"),
            grpc: Some("grpc.example.com"),
            capabilities: &["keyvalue", "blobstore", "config", "logging", "otel"],
            expect_shape: Shape::Chain,
            ..Case::new("kitchen-sink", "http", "chain")
        },
    ]
}

/// A case that builds fine but the host will not run here, with its reason.
#[derive(Clone, Copy)]
struct Blocked(&'static str);

impl Blocked {
    fn reason(self) -> &'static str {
        self.0
    }
}

/// Outcome of one case.
enum Outcome {
    Verified,
    Blocked(Blocked),
}

/// Run every case, reporting each and failing at the end if any did.
pub fn run(workspace: &Path) -> Result<()> {
    let wash = crate::ensure_wash(workspace).context("failed to locate wash")?;
    let tempdir = tempfile::tempdir().context("failed to create a temp dir")?;
    println!("  generating into {}\n", tempdir.path().display());

    let mut failures = Vec::new();
    let mut blocked = Vec::new();
    for case in cases() {
        print!("  {:<18} ... ", case.name);
        std::io::stdout().flush().ok();
        match verify(&wash, tempdir.path(), &case) {
            Ok(Outcome::Verified) => println!("ok"),
            Ok(Outcome::Blocked(kind)) => {
                match kind {
                    Blocked(_) => println!("built, boot BLOCKED"),
                }
                println!("      {}", kind.reason());
                blocked.push(case.name);
            }
            Err(err) => {
                println!("FAILED");
                println!("      {err:#}");
                failures.push(case.name);
            }
        }
    }

    let verified = cases().len() - failures.len() - blocked.len();
    if failures.is_empty() {
        println!("\n{verified} generated shape(s) verified");
        if !blocked.is_empty() {
            println!("{} blocked: {}", blocked.len(), blocked.join(", "));
        }
        Ok(())
    } else {
        bail!("{} case(s) failed: {}", failures.len(), failures.join(", "))
    }
}

fn verify(wash: &Path, parent: &Path, case: &Case) -> Result<Outcome> {
    let project = generate(wash, parent, case)?;
    // Checked before building: a placement case's claim is *which* file got the capability.
    check_sources(&project, case)?;

    build(wash, &project)?;

    if case.blocked.is_none() {
        let port = free_port()?;
        set_dev_address(&project, port)?;
        boot(wash, &project, port, case)?;
    }

    // Re-derive the shape: the generator and deriver drifted once, over world naming.
    let topology = wash_topology::derive(&project, case.name)
        .context("failed to derive a topology from the generated project")?;
    if topology.shape != case.expect_shape {
        bail!(
            "asked for {:?} but the deriver reads it as {:?}",
            case.expect_shape,
            topology.shape
        );
    }
    let unexpected: Vec<&str> = topology
        .unresolved
        .iter()
        .map(|u| u.reason.as_str())
        .filter(|reason| !case.allow_unresolved.contains(reason))
        .collect();
    if !unexpected.is_empty() {
        bail!("unresolved wiring: {}", unexpected.join(", "));
    }

    Ok(match case.blocked {
        Some(blocked) => Outcome::Blocked(blocked),
        None => Outcome::Verified,
    })
}

fn generate(wash: &Path, parent: &Path, case: &Case) -> Result<PathBuf> {
    let mut cmd = Command::new(wash);
    cmd.arg("-C")
        .arg(parent)
        .arg("wizard")
        .arg("--trigger")
        .arg(case.trigger)
        .arg("--linking")
        .arg(case.linking)
        .arg("--count")
        .arg(case.count.to_string())
        .arg("--name")
        .arg(case.name);
    cmd.arg("--edition").arg(case.edition);
    for depth in case.branches {
        cmd.arg("--branch").arg(depth.to_string());
    }
    if let Some(host) = case.egress {
        cmd.arg("--egress").arg(host);
    }
    if let Some(host) = case.grpc {
        cmd.arg("--grpc").arg(host);
    }
    for capability in case.capabilities {
        cmd.arg("--capability").arg(capability);
    }
    for entry in case.place {
        cmd.arg("--place").arg(entry);
    }

    let output = cmd.output().context("failed to run wash wizard")?;
    if !output.status.success() {
        bail!(
            "wash wizard failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(parent.join(case.name))
}

/// Check that each capability landed where it was placed, and nowhere else —
/// without the negative half, ignoring `--place` entirely would still pass.
fn check_sources(project: &Path, case: &Case) -> Result<()> {
    for (relative, needle) in case.expect_in {
        let path = project.join(relative);
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if !source.contains(needle) {
            bail!("{relative} should mention `{needle}` and does not");
        }
    }
    for (relative, needle) in case.expect_not_in {
        let path = project.join(relative);
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if source.contains(needle) {
            bail!("{relative} mentions `{needle}`, which was placed elsewhere");
        }
    }
    Ok(())
}

/// Build, treating any warning as a failure the user would otherwise clean up.
fn build(wash: &Path, project: &Path) -> Result<()> {
    let output = Command::new(wash)
        .arg("-C")
        .arg(project)
        .arg("build")
        .output()
        .context("failed to run wash build")?;

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        bail!("wash build failed:\n{combined}");
    }

    let warnings: Vec<&str> = combined
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("warning:"))
        // Linker-stderr warnings come from the host toolchain, not the generated crates.
        .filter(|line| !line.contains("linker stderr"))
        .collect();
    if !warnings.is_empty() {
        bail!(
            "generated code built with warnings:\n  {}",
            warnings.join("\n  ")
        );
    }
    Ok(())
}

/// Ask the OS for an unused port; racy in principle, fine in this temp run.
fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("failed to reserve a port")?;
    let port = listener
        .local_addr()
        .context("failed to read the reserved port")?
        .port();
    drop(listener);
    Ok(port)
}

/// Point the generated project's `wash dev` at `port` by rewriting `dev.address`.
/// Edited as generic YAML so nothing the generator wrote is silently dropped.
fn set_dev_address(project: &Path, port: u16) -> Result<()> {
    let path = project.join(".wash").join("config.yaml");
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let mut config: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&raw).context("failed to parse the generated config")?;

    let mapping = config
        .as_mapping_mut()
        .context("generated config is not a mapping")?;
    let dev = mapping
        .entry(serde_yaml_ng::Value::from("dev"))
        .or_insert_with(|| serde_yaml_ng::Value::Mapping(Default::default()));
    dev.as_mapping_mut()
        .context("`dev` in the generated config is not a mapping")?
        .insert(
            serde_yaml_ng::Value::from("address"),
            serde_yaml_ng::Value::from(format!("127.0.0.1:{port}")),
        );

    std::fs::write(
        &path,
        serde_yaml_ng::to_string(&config).context("failed to serialize the config")?,
    )
    .with_context(|| format!("failed to write {}", path.display()))
}

/// Kills the host on every exit path, so a failed case never leaks a `wash dev`.
struct Host(Child);

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Boot the workload and require the host to accept it.
fn boot(wash: &Path, project: &Path, port: u16, case: &Case) -> Result<()> {
    // The log is kept: a rejection's reason lives there, and the temp dir is soon gone.
    let log_path = project.join("dev.log");
    let log = std::fs::File::create(&log_path)
        .with_context(|| format!("failed to create {}", log_path.display()))?;
    let errors = log
        .try_clone()
        .context("failed to duplicate the log handle")?;

    let child = Command::new(wash)
        .arg("-C")
        .arg(project)
        .arg("dev")
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors))
        .spawn()
        .context("failed to start wash dev")?;
    let mut host = Host(child);

    wait_for_registration(&mut host, &log_path, port, case.trigger)
        .map_err(|err| annotate(err, &log_path))
}

/// Wait until the host registers the workload or rejects it: HTTP answers non-404
/// once routable; messaging/service triggers are judged by a clean settle after build.
fn wait_for_registration(host: &mut Host, log_path: &Path, port: u16, trigger: &str) -> Result<()> {
    let deadline = Instant::now() + BOOT_TIMEOUT;
    let mut built_at: Option<Instant> = None;

    while Instant::now() < deadline {
        if let Some(status) = host.0.try_wait().context("failed to poll wash dev")? {
            bail!("wash dev exited early with {status}");
        }
        let log = std::fs::read_to_string(log_path).unwrap_or_default();
        if let Some(marker) = FAILURE_MARKERS.iter().find(|m| log.contains(**m)) {
            bail!("host rejected the workload: {marker}");
        }

        if trigger == "http" {
            if http_get(port).is_ok_and(|response| !is_unrouted(&response)) {
                return Ok(());
            }
        } else if log.contains("Finished `release` profile") {
            // cargo's line once the rebuild completes; give a link failure time to appear.
            let since = *built_at.get_or_insert_with(Instant::now);
            if since.elapsed() >= SETTLE {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!("wash dev did not register the workload within {BOOT_TIMEOUT:?}")
}

/// The host is up but has nothing to route to yet.
fn is_unrouted(response: &str) -> bool {
    response
        .lines()
        .next()
        .is_some_and(|status| status.contains("404"))
}

/// Attach the tail of the host log to a failure.
fn annotate(err: anyhow::Error, log_path: &Path) -> anyhow::Error {
    let Ok(log) = std::fs::read_to_string(log_path) else {
        return err;
    };
    let tail: Vec<&str> = log.lines().rev().take(30).collect();
    let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    anyhow::anyhow!("{err:#}\n--- wash dev log (tail) ---\n{tail}")
}

/// Minimal hand-written HTTP/1.1 GET so the harness needs no curl.
fn http_get(port: u16) -> Result<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).context("connect failed")?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    stream.flush()?;

    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer).context("read failed")?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

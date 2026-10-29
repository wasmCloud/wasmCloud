#![cfg(unix)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, allow(unstable_features))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use tempfile::TempDir;
use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner as _,
};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader},
    process::{Child, Command},
    sync::oneshot,
    time::timeout,
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

struct WashProcess {
    child: Child,
    log: tokio::task::JoinHandle<std::io::Result<String>>,
    output: tokio::task::JoinHandle<std::io::Result<String>>,
    ready: oneshot::Receiver<()>,
}

impl WashProcess {
    fn spawn(home: &Path, args: &[&str], ready_message: &'static str) -> Result<Self> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_wash"));
        command
            .args(args)
            .env_clear()
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("RUST_LOG", "info")
            .current_dir(home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        let mut child = command.spawn().context("failed to spawn wash")?;
        let mut stdout = child.stdout.take().context("stdout was piped")?;
        let output = tokio::spawn(async move {
            let mut output = String::new();
            stdout.read_to_string(&mut output).await?;
            Ok(output)
        });
        let stderr = child.stderr.take().context("stderr was piped")?;
        let (ready_tx, ready) = oneshot::channel();
        let log = tokio::spawn(async move {
            let mut ready_tx = Some(ready_tx);
            let mut lines = BufReader::new(stderr).lines();
            let mut log = String::new();
            while let Some(line) = lines.next_line().await? {
                if line.contains(ready_message)
                    && let Some(tx) = ready_tx.take()
                {
                    let _ = tx.send(());
                }
                log.push_str(&line);
                log.push('\n');
            }
            Ok(log)
        });
        Ok(Self {
            child,
            log,
            output,
            ready,
        })
    }

    async fn wait_ready(&mut self) -> Result<()> {
        if !matches!(timeout(STARTUP_TIMEOUT, &mut self.ready).await, Ok(Ok(()))) {
            let _ = self.child.kill().await;
            bail!(
                "wash did not start: {}{}",
                (&mut self.log).await??,
                (&mut self.output).await??
            );
        }
        Ok(())
    }

    async fn exit(mut self) -> Result<(std::process::ExitStatus, String)> {
        let status = timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await??;
        Ok((
            status,
            format!("{}{}", self.log.await??, self.output.await??),
        ))
    }

    async fn shutdown(self, signal: &str) -> Result<String> {
        let pid = self.child.id().context("wash already exited")?;
        let status = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(pid.to_string())
            .status()
            .await?;
        anyhow::ensure!(status.success(), "failed to send SIG{signal}");
        let (status, log) = self.exit().await?;
        anyhow::ensure!(status.success(), "wash exited with {status}: {log}");
        Ok(log)
    }
}

fn available_address() -> Result<String> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .to_string())
}

fn dev_project(dev: Value) -> Result<TempDir> {
    let home = tempfile::tempdir()?;
    std::fs::create_dir(home.path().join(".wash"))?;
    std::fs::write(home.path().join("component.wasm"), b"\0asm\x0d\0\x01\0")?;
    let mut config = json!({ "dev": {
        "command": "true",
        "component_path": "component.wasm",
        "address": "127.0.0.1:0"
    }});
    config
        .get_mut("dev")
        .and_then(Value::as_object_mut)
        .context("dev is an object")?
        .extend(
            dev.as_object()
                .context("dev overrides are an object")?
                .clone(),
        );
    std::fs::write(
        home.path().join(".wash/config.json"),
        serde_json::to_vec(&config)?,
    )?;
    Ok(home)
}

fn write_identity(home: &Path) -> Result<(String, String)> {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])?;
    let cert_path = home.join("cert.pem");
    let key_path = home.join("key.pem");
    std::fs::write(&cert_path, cert.pem())?;
    std::fs::write(&key_path, signing_key.serialize_pem())?;
    Ok((
        cert_path.to_string_lossy().into(),
        key_path.to_string_lossy().into(),
    ))
}

#[tokio::test]
async fn dev_serves_http_and_stops_on_sigterm() -> Result<()> {
    let address = available_address()?;
    let home = dev_project(json!({ "address": address }))?;
    let mut wash = WashProcess::spawn(home.path(), &["dev"], "listening for HTTP requests")?;
    wash.wait_ready().await?;
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()?
        .get(format!("http://{address}/"))
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    let log = wash.shutdown("TERM").await?;
    assert!(log.contains("Stopping development session"), "{log}");
    Ok(())
}

#[tokio::test]
async fn dev_serves_https_with_filesystem_plugins_and_sidecars() -> Result<()> {
    let address = available_address()?;
    let home = dev_project(json!({}))?;
    let (cert, key) = write_identity(home.path())?;
    let config = json!({ "dev": {
        "command": "true", "component_path": "component.wasm", "address": address,
        "tls_cert_path": cert, "tls_key_path": key,
        "http_client_ca_paths": [cert],
        "wasi_blobstore_path": home.path().join("blobstore"),
        "wasi_keyvalue_path": home.path().join("keyvalue"),
        "volumes": [{ "host_path": home.path().join("volume"), "guest_path": "/data" }],
        "components": [{ "name": "sidecar", "file": "component.wasm" }]
    }});
    std::fs::create_dir(home.path().join("volume"))?;
    std::fs::write(
        home.path().join(".wash/config.json"),
        serde_json::to_vec(&config)?,
    )?;
    let mut wash = WashProcess::spawn(home.path(), &["dev"], "listening for HTTP requests")?;
    wash.wait_ready().await?;
    let ca = reqwest::Certificate::from_pem(&std::fs::read(&cert)?)?;
    let response = reqwest::Client::builder()
        .no_proxy()
        .add_root_certificate(ca)
        .build()?
        .get(format!("https://{address}/"))
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    let log = wash.shutdown("INT").await?;
    assert!(log.contains("Stopping development session"), "{log}");
    Ok(())
}

#[tokio::test]
async fn dev_reports_startup_failures() -> Result<()> {
    for (dev, expected) in [
        (
            json!({ "wasm_proposals": ["not-a-proposal"] }),
            "invalid dev.wasm_proposals entry",
        ),
        (
            json!({ "tls_cert_path": "missing.pem", "tls_key_path": "missing.key" }),
            "TLS certificate file",
        ),
        (
            json!({ "http_client_ca_paths": ["missing-ca.pem"] }),
            "failed to load dev.http_client_ca_paths",
        ),
        (
            json!({ "command": "false" }),
            "build command 'false' failed",
        ),
        (
            json!({ "component_path": "missing.wasm" }),
            "component not found at expected path",
        ),
        (
            json!({ "host_interfaces": [{
                "namespace": "wasi", "package": "http", "interfaces": ["incoming-handler"],
                "config": { "localRoute": "http://invalid.local" }
            }] }),
            "invalid `localRoute` entries",
        ),
    ] {
        let home = dev_project(dev)?;
        let (status, log) =
            WashProcess::spawn(home.path(), &["dev"], "listening for HTTP requests")?
                .exit()
                .await?;
        assert!(!status.success(), "startup unexpectedly succeeded: {log}");
        assert!(log.contains(expected), "expected {expected:?}: {log}");
    }
    Ok(())
}

async fn nats() -> Result<(String, Option<ContainerAsync<GenericImage>>)> {
    if let Ok(url) = std::env::var("WASH_TEST_NATS_URL") {
        return Ok((url, None));
    }
    let server = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await?;
    Ok((
        format!(
            "nats://127.0.0.1:{}",
            server.get_host_port_ipv4(4222).await?
        ),
        Some(server),
    ))
}

#[tokio::test]
#[ignore = "requires NATS (WASH_TEST_NATS_URL or Docker)"]
async fn dev_starts_with_nats_backends() -> Result<()> {
    let (nats_url, _server) = nats().await?;
    let home = dev_project(json!({ "data_nats_url": nats_url }))?;
    let mut wash = WashProcess::spawn(home.path(), &["dev"], "listening for HTTP requests")?;
    wash.wait_ready().await?;
    let log = wash.shutdown("TERM").await?;
    assert!(log.contains("Stopping development session"), "{log}");
    Ok(())
}

#[tokio::test]
#[ignore = "requires NATS (WASH_TEST_NATS_URL or Docker)"]
async fn host_serves_probes_and_ingress_and_shuts_down() -> Result<()> {
    let (nats_url, _server) = nats().await?;
    let home = tempfile::tempdir()?;
    let ingress = available_address()?;
    let probe = available_address()?;
    let mut wash = WashProcess::spawn(
        home.path(),
        &[
            "host",
            "--scheduler-nats-url",
            &nats_url,
            "--data-nats-url",
            &nats_url,
            "--host-name",
            "coverage-host",
            "--environment",
            "coverage",
            "--http-addr",
            &ingress,
            "--probe-addr",
            &probe,
            "--drain-delay",
            "1ms",
        ],
        "Host started",
    )?;
    wash.wait_ready().await?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?;
    for path in ["/livez", "/readyz"] {
        timeout(STARTUP_TIMEOUT, async {
            loop {
                let response = client.get(format!("http://{probe}{path}")).send().await?;
                if response.status() == reqwest::StatusCode::OK {
                    break Ok::<_, anyhow::Error>(());
                }
                assert_eq!(
                    response.status(),
                    reqwest::StatusCode::SERVICE_UNAVAILABLE,
                    "{path}"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
    }
    let response = client.get(format!("http://{ingress}/")).send().await?;
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    let log = wash.shutdown("TERM").await?;
    assert!(log.contains("Draining"), "{log}");
    assert!(log.contains("Stopping host"), "{log}");
    Ok(())
}

#[tokio::test]
#[ignore = "requires NATS (WASH_TEST_NATS_URL or Docker)"]
async fn host_serves_https_with_outbound_identity_refresh() -> Result<()> {
    let (nats_url, _server) = nats().await?;
    let home = tempfile::tempdir()?;
    let (cert, key) = write_identity(home.path())?;
    let ingress = available_address()?;
    let mut wash = WashProcess::spawn(
        home.path(),
        &[
            "host",
            "--scheduler-nats-url",
            &nats_url,
            "--data-nats-url",
            &nats_url,
            "--http-addr",
            &ingress,
            "--tls-cert-path",
            &cert,
            "--tls-key-path",
            &key,
            "--http-client-cert-path",
            &cert,
            "--http-client-key-path",
            &key,
            "--http-client-ca-path",
            &cert,
            "--http-client-identity-refresh",
            "60s",
        ],
        "Host started",
    )?;
    wash.wait_ready().await?;
    let ca = reqwest::Certificate::from_pem(&std::fs::read(cert)?)?;
    let response = reqwest::Client::builder()
        .no_proxy()
        .add_root_certificate(ca)
        .build()?
        .get(format!("https://{ingress}/"))
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    let log = wash.shutdown("INT").await?;
    assert!(log.contains("Stopping host"), "{log}");
    Ok(())
}

#[tokio::test]
async fn host_rejects_invalid_limits_before_connecting_to_nats() -> Result<()> {
    for (flag, value, expected) in [
        ("--max-guest-memory", "garbage", "max-guest-memory"),
        ("--max-process-memory", "garbage", "max-process-memory"),
        ("--max-http-ingress-connections", "0", "must be at least 1"),
    ] {
        let home = tempfile::tempdir()?;
        let (status, log) = WashProcess::spawn(
            home.path(),
            &[
                "host",
                "--scheduler-nats-url",
                "nats://127.0.0.1:1",
                flag,
                value,
            ],
            "Host started",
        )?
        .exit()
        .await?;
        assert!(!status.success(), "invalid limit was accepted: {log}");
        assert!(log.contains(expected), "expected {expected:?}: {log}");
        assert!(
            !log.contains("failed to connect"),
            "validation happened after NATS: {log}"
        );
    }
    Ok(())
}

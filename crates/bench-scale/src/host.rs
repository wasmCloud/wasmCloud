//! Running nats-server and the host as child processes, and finding the host
//! on NATS by its heartbeat.

use std::collections::HashSet;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use futures::StreamExt as _;

use crate::api::{HEARTBEAT_SUBJECT, HostHeartbeat};
use crate::cli::HostCli;

/// A child process killed when this is dropped, so nothing outlives the
/// bench: not on error, not on panic, not on Ctrl-C (which unwinds the run).
pub struct ChildGuard {
    child: Child,
    exited: Option<ExitStatus>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self {
            child,
            exited: None,
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The exit status if the process has ended on its own.
    pub fn exited(&mut self) -> Option<ExitStatus> {
        if self.exited.is_none() {
            self.exited = self.child.try_wait().ok().flatten();
        }
        self.exited
    }
}

impl ChildGuard {
    /// Kill the process unless it already ended, and reap it.
    pub fn kill(&mut self) {
        if self.exited().is_none() {
            let _ = self.child.kill();
            self.exited = self.child.wait().ok();
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The last `lines` lines of a log file, for a stop report.
pub fn log_tail(path: &Path, lines: usize) -> String {
    let Ok(body) = std::fs::read(path) else {
        return String::new();
    };
    let body = String::from_utf8_lossy(&body);
    let all: Vec<&str> = body.lines().collect();
    let skip = all.len().saturating_sub(lines);
    all.iter()
        .skip(skip)
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

fn log_file(path: &Path) -> Result<(Stdio, Stdio)> {
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    Ok((Stdio::from(file.try_clone()?), Stdio::from(file)))
}

/// Only what a process needs to find its tools and home, so the bench's own
/// environment (a cargo build's CARGO_*/RUST* variables) does not reach it.
fn isolated(command: &mut Command) {
    const KEPT: &[&str] = &[
        "PATH",
        "HOME",
        "TMPDIR",
        "USER",
        "LANG",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ];
    // Without these, Winsock fails to initialise and the temp and profile
    // directories fall back to the Windows directory.
    #[cfg(windows)]
    const KEPT_WINDOWS: &[&str] = &[
        "SystemRoot",
        "SystemDrive",
        "windir",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "PATHEXT",
        "ComSpec",
    ];
    #[cfg(not(windows))]
    const KEPT_WINDOWS: &[&str] = &[];
    command.env_clear();
    for &key in KEPT.iter().chain(KEPT_WINDOWS) {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("finding a free port")?;
    Ok(listener.local_addr()?.port())
}

/// Run nats-server on a free loopback port and wait until it accepts
/// connections.
pub async fn spawn_nats(binary: &Path, run_dir: &Path) -> Result<(ChildGuard, String)> {
    let port = free_port()?;
    let log = run_dir.join("nats-server.log");
    let (stdout, stderr) = log_file(&log)?;
    let mut command = Command::new(binary);
    isolated(&mut command);
    let child = command
        .args(["-a", "127.0.0.1", "-p", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .with_context(|| format!("running {}", binary.display()))?;
    let mut guard = ChildGuard::new(child);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = guard.exited() {
            bail!("nats-server exited ({status}):\n{}", log_tail(&log, 20));
        }
        if tokio::time::timeout(
            Duration::from_millis(200),
            tokio::net::TcpStream::connect(addr),
        )
        .await
        .is_ok_and(|r| r.is_ok())
        {
            break;
        }
        if Instant::now() > deadline {
            bail!("nats-server did not listen on {addr} within 15s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok((guard, format!("nats://{addr}")))
}

/// The `wash host` flags a binary takes, from its `host --help`.
pub async fn wash_host_flags(binary: &Path) -> Result<HashSet<String>> {
    let mut command = Command::new(binary);
    isolated(&mut command);
    command.args(["host", "--help"]).stdin(Stdio::null());
    let output = crate::process::output(command, Duration::from_secs(15))
        .await
        .with_context(|| format!("running {} host --help", binary.display()))?;
    if !output.status.success() {
        bail!(
            "`{} host --help` failed; pass --host-cli raw for a host with another CLI:\n{}",
            binary.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(parse_long_flags(&String::from_utf8_lossy(&output.stdout)))
}

/// The `--long-flag`s a help text declares: those opening an option line, not
/// ones its prose mentions.
pub fn parse_long_flags(help: &str) -> HashSet<String> {
    help.lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with('-'))
        .filter_map(|line| {
            line.split(|c: char| c.is_whitespace() || c == ',' || c == '=' || c == '[' || c == '<')
                .find_map(|word| word.strip_prefix("--"))
        })
        .map(|flag| {
            flag.trim_end_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .to_string()
        })
        .filter(|flag| !flag.is_empty())
        .collect()
}

/// What the host is told, in units it parses.
#[derive(Debug, Clone)]
pub struct HostLimits {
    pub memory_limit: u64,
    pub guest_memory: u64,
    pub core_instances: u32,
}

/// The host command line, and the default flags the binary did not take.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HostCommand {
    pub args: Vec<String>,
    pub unsupported_flags: Vec<String>,
    /// Set as `--host-name`, so the bench can tell its host's heartbeat from
    /// others on a shared NATS.
    pub host_name: Option<String>,
}

pub fn host_command(
    cli: HostCli,
    supported: &HashSet<String>,
    nats_url: &str,
    limits: &HostLimits,
    oci_cache: &Path,
    host_name: &str,
    extra: &[String],
) -> HostCommand {
    let substitute = |arg: &str| {
        arg.replace("{nats_url}", nats_url)
            .replace("{memory_limit}", &limits.memory_limit.to_string())
            .replace("{guest_memory}", &limits.guest_memory.to_string())
            .replace("{core_instances}", &limits.core_instances.to_string())
            .replace("{http_addr}", "127.0.0.1:0")
    };
    if cli == HostCli::Raw {
        return HostCommand {
            args: extra.iter().map(|a| substitute(a)).collect(),
            unsupported_flags: Vec::new(),
            host_name: None,
        };
    }
    let wanted: Vec<(&str, Option<String>)> = vec![
        ("scheduler-nats-url", Some(nats_url.to_string())),
        ("data-nats-url", Some(nats_url.to_string())),
        ("http-addr", Some("127.0.0.1:0".to_string())),
        ("allow-insecure-registries", None),
        ("max-process-memory", Some(limits.memory_limit.to_string())),
        ("max-guest-memory", Some(limits.guest_memory.to_string())),
        ("core-instances", Some(limits.core_instances.to_string())),
        ("oci-cache-dir", Some(oci_cache.display().to_string())),
        ("host-name", Some(host_name.to_string())),
    ];
    let mut args = vec!["host".to_string()];
    let mut unsupported_flags = Vec::new();
    let mut named = None;
    for (flag, value) in wanted {
        if !supported.contains(flag) {
            unsupported_flags.push(flag.to_string());
            continue;
        }
        if flag == "host-name" {
            named = Some(host_name.to_string());
        }
        args.push(format!("--{flag}"));
        args.extend(value);
    }
    args.extend(extra.iter().map(|a| substitute(a)));
    HostCommand {
        args,
        unsupported_flags,
        host_name: named,
    }
}

pub fn spawn_host(
    binary: &Path,
    command_line: &HostCommand,
    log_level: &str,
    log: &Path,
) -> Result<ChildGuard> {
    let (stdout, stderr) = log_file(log)?;
    let mut command = Command::new(binary);
    isolated(&mut command);
    let child = command
        .args(&command_line.args)
        .env("RUST_LOG", log_level)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .with_context(|| format!("running {}", binary.display()))?;
    Ok(ChildGuard::new(child))
}

/// Which heartbeat is the host under test.
pub enum Pick<'a> {
    Id(&'a str),
    Name(&'a str),
    Any,
}

impl Pick<'_> {
    fn matches(&self, heartbeat: &HostHeartbeat) -> bool {
        match self {
            Self::Id(id) => heartbeat.id == *id,
            Self::Name(name) => heartbeat.hostname == *name || heartbeat.friendly_name == *name,
            Self::Any => true,
        }
    }
}

/// A heartbeat subscription opened before the host starts, so its first
/// heartbeat (sent immediately) is not missed.
pub struct Heartbeats(async_nats::Subscriber);

impl Heartbeats {
    pub async fn subscribe(nats: &async_nats::Client) -> Result<Self> {
        let subscriber = nats
            .subscribe(HEARTBEAT_SUBJECT)
            .await
            .context("subscribing to heartbeats")?;
        // Round-trip to the server so the subscription is in place before
        // anything starts.
        nats.flush().await.context("flushing NATS")?;
        Ok(Self(subscriber))
    }

    /// Wait for a heartbeat `pick` accepts, giving up early if `host` exits.
    pub async fn wait(
        &mut self,
        pick: Pick<'_>,
        limit: Duration,
        mut host: Option<(&mut ChildGuard, &PathBuf)>,
    ) -> Result<HostHeartbeat> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            if let Some((guard, log)) = host.as_mut()
                && let Some(status) = guard.exited()
            {
                bail!(
                    "the host exited ({status}) before it heartbeat:\n{}",
                    log_tail(log, 30)
                );
            }
            let next = tokio::time::timeout(Duration::from_millis(250), self.0.next()).await;
            match next {
                Ok(Some(message)) => {
                    if let Ok(heartbeat) = serde_json::from_slice::<HostHeartbeat>(&message.payload)
                        && pick.matches(&heartbeat)
                    {
                        return Ok(heartbeat);
                    }
                }
                Ok(None) => bail!("the heartbeat subscription closed"),
                Err(_) => {}
            }
            if tokio::time::Instant::now() > deadline {
                bail!("no matching host heartbeat within {limit:?}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELP: &str = "Usage: wash host [OPTIONS]\n\nOptions:\n      --scheduler-nats-url <SCHEDULER_NATS_URL>\n          The URL\n      --data-nats-url <DATA_NATS_URL>\n      --http-addr <HTTP_ADDR>\n      --allow-insecure-registries\n      --core-instances <CORE_INSTANCES>\n          [env: WASH_CORE_INSTANCES=]\n      --max-guest-memory <MAX_GUEST_MEMORY>\n      --host-name <HOST_NAME>\n  -h, --help\n          Print help (see `--max-process-memory`)\n";

    #[test]
    fn reads_flags_from_help() {
        let flags = parse_long_flags(HELP);
        for flag in [
            "scheduler-nats-url",
            "http-addr",
            "allow-insecure-registries",
            "core-instances",
            "help",
        ] {
            assert!(flags.contains(flag), "{flag} missing from {flags:?}");
        }
        assert!(!flags.contains("oci-cache-dir"));
    }

    #[test]
    fn leaves_out_flags_the_host_lacks() {
        let flags = parse_long_flags(HELP);
        // Named in prose only, so not taken as declared.
        assert!(!flags.contains("max-process-memory"));
        let limits = HostLimits {
            memory_limit: 1 << 30,
            guest_memory: 3 << 28,
            core_instances: 100,
        };
        let command = host_command(
            HostCli::Wash,
            &flags,
            "nats://127.0.0.1:4222",
            &limits,
            Path::new("/tmp/oci"),
            "bench-scale-1",
            &["--max-concurrent-starts".into(), "4".into()],
        );
        assert_eq!(command.args.first().map(String::as_str), Some("host"));
        let joined = command.args.join(" ");
        assert!(joined.contains("--scheduler-nats-url nats://127.0.0.1:4222"));
        assert!(joined.contains("--http-addr 127.0.0.1:0"));
        assert!(joined.contains("--allow-insecure-registries --max-guest-memory 805306368"));
        assert!(joined.contains("--core-instances 100"));
        assert!(joined.ends_with("--max-concurrent-starts 4"));
        assert_eq!(
            command.unsupported_flags,
            ["max-process-memory", "oci-cache-dir"]
        );
        assert_eq!(command.host_name.as_deref(), Some("bench-scale-1"));
    }

    #[test]
    fn raw_command_substitutes_placeholders() {
        let limits = HostLimits {
            memory_limit: 1000,
            guest_memory: 750,
            core_instances: 7,
        };
        let command = host_command(
            HostCli::Raw,
            &HashSet::new(),
            "nats://n:1",
            &limits,
            Path::new("/c"),
            "x",
            &[
                "run".into(),
                "--nats={nats_url}".into(),
                "--mem={memory_limit}".into(),
                "{core_instances}".into(),
            ],
        );
        assert_eq!(
            command.args,
            ["run", "--nats=nats://n:1", "--mem=1000", "7"]
        );
        assert!(command.host_name.is_none());
    }
}

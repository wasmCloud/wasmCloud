use std::path::{Path, PathBuf};
use std::str::FromStr;
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use tracing::warn;
use wash_lib::cli::{CommandOutput, OutputKind};
use wash_lib::run;
use wash_lib::run::{CtxBuilder, DirPerms, LocalRuntime};

#[derive(Parser, Debug, Clone)]
pub struct RunCommand {
    /// Reference to a component. Can be a local file or a registry reference.
    #[clap(name = "ref")]
    pub reference: String,

    /// Args to pass to the component.
    #[clap(name = "args")]
    pub args: Vec<String>,

    #[clap(flatten)]
    pub env: EnvironmentOptions,

    #[clap(flatten)]
    pub dir: DirOptions,

    #[clap(flatten)]
    pub net: NetworkOptions,
}

#[derive(Parser, Debug, Clone)]
pub struct EnvironmentOptions {
    /// Allow access to specific environment variables.
    #[clap(long = "allow-env")]
    pub allow: Vec<String>,

    #[clap(long = "allow-env-prefix")]
    /// Allow access to any environment variable starting with this prefix.
    pub allow_prefix: Vec<String>,

    /// Allow access to all environment variable.
    #[clap(long = "allow-env-all")]
    pub allow_all: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct DirOptions {
    /// Allow mutable access to a directory tree.
    ///
    /// Mutability means files can be deleted and created.
    /// The format of this argument is:
    ///
    /// ```shell
    /// --allow-dir-mutable <host_dir>[:<file_permission>[:<guest_dir>]]
    /// ```
    ///
    /// Where:
    /// - `<host_dir>` is which host directory can be accessed
    /// - `<file_permission>` can be `ro` for read-only or `rw` for read-write (default: `ro`)
    /// - `<guest_dir>` is the pre-opens name under which the directory will be available for the component (default: `.`)
    #[clap(long = "allow-dir-mutable")]
    pub mutable: Vec<String>,

    #[clap(long = "allow-dir")]
    /// Allow immutable access to a directory tree.
    ///
    /// Immutable means no files can be deleted nor created.
    /// The format of this argument is:
    ///
    /// ```shell
    /// --allow-dir <host_dir>[:<file_permission>[:<guest_dir>]]
    /// ```
    ///
    /// Where:
    /// - `<host_dir>` is which host directory can be accessed
    /// - `<file_permission>` can be `ro` for read-only or `rw` for read-write (default: `ro`)
    /// - `<guest_dir>` is the pre-opens name under which the directory will be available for the component (default: `.`)
    pub immutable: Vec<String>,
}

#[derive(Parser, Debug, Clone)]
pub struct NetworkOptions {
    /// Allow name to IP resolution.
    #[clap(long = "allow-net-name-resolution")]
    pub allow_name_resolution: bool,

    /// Allow the component to connect to any peers.
    #[clap(long = "allow-net-connect-all")]
    pub allow_connect_all: bool,

    /// Allow the component to connect to peers.
    ///
    /// The format expected is `<ip_addr>[:<port>[-<upper_port>]]`, where:
    /// - `<ip_addr>` is the peer address, it can be IPv4 or IPv6
    /// - `<port>` is the peer port we can connect to
    /// - `<upper_port>` authorize the connection to ports ranging from `<port>` to `<upper_port>` (inclusive)
    ///
    /// NOTE: In future versions, we may support hostname in place of `<ip_addr>`.
    #[clap(long = "allow-net-connect")]
    pub allow_connect: Vec<String>,

    /// Allow the component to connect to any TCP peers.
    #[clap(long = "allow-net-connect-tcp-all")]
    pub allow_connect_tcp_all: bool,

    /// Allow the component to connect to a specific TCP peer.
    ///
    /// The format expected is the same as expected of `--allow-net-connect`.
    #[clap(long = "allow-net-connect-tcp")]
    pub allow_connect_tcp: Vec<String>,

    /// Allow the component to connect to any UDP peers.
    #[clap(long = "allow-net-connect-udp-all")]
    pub allow_connect_udp_all: bool,

    /// Allow the component to connect to UDP peers in the specified CIDR prefixes.
    ///
    /// The format expected is the same as expected of `--allow-net-connect`.
    #[clap(long = "allow-net-connect-udp")]
    pub allow_connect_udp: Vec<String>,

    /// Allow the component to bind to any interface, port and protocol.
    #[clap(long = "allow-net-bind-all")]
    pub allow_bind_all: bool,

    /// Allow the component to bind to a specific interface, specific port ranges but any protocol.
    ///
    /// The format expected is the same as expected of `--allow-net-connect`.
    #[clap(long = "allow-net-bind")]
    pub allow_bind: Vec<String>,

    /// Allow the component to bind to any interface and port using the TCP protocol.
    #[clap(long = "allow-net-bind-tcp-all")]
    pub allow_bind_tcp_all: bool,

    /// Allow the component to bind to a specific interface and specific port ranges using the TCP protocol.
    ///
    /// The format expected is the same as expected of `--allow-net-connect`.
    #[clap(long = "allow-net-bind-tcp")]
    pub allow_bind_tcp: Vec<String>,

    /// Allow the component to bind to any interface and port using the UDP protocol.
    #[clap(long = "allow-net-bind-udp-all")]
    pub allow_bind_udp_all: bool,

    /// Allow the component to bind to a specific interface and specific port ranges using the UDP protocol.
    ///
    /// The format expected is the same as expected of `--allow-net-connect`.
    #[clap(long = "allow-net-bind-udp")]
    pub allow_bind_udp: Vec<String>,
}

struct ParsedDir {
    path: PathBuf,
    file_perms: Option<FilePerms>,
    guest_dir: Option<String>,
}

enum FilePerms {
    ReadOnly,
    ReadWrite,
}

impl FromStr for FilePerms {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "ro" => Ok(Self::ReadOnly),
            "rw" => Ok(Self::ReadWrite),
            _ => Err(anyhow!("file permission is incorrect, 'ro' or 'rw' expected")),
        }
    }
}

impl Into<run::FilePerms> for FilePerms {
    fn into(self) -> run::FilePerms {
        match self {
            FilePerms::ReadOnly => run::FilePerms::READ,
            FilePerms::ReadWrite => run::FilePerms::WRITE,
        }
    }
}

fn parse_dir(raw: impl AsRef<str>) -> Result<ParsedDir> {
    let mut parts = raw.as_ref().split(':');

    let raw_path = parts
        .next()
        .ok_or(
            anyhow!("dir flags require at least a path")
        )?;

    Ok(ParsedDir {
        path: PathBuf::from(raw_path),
        file_perms: parts.next().map(|raw| FilePerms::from_str(raw)).transpose()?,
        guest_dir: parts.next().map(|raw| raw.to_owned()),
    })
}

pub async fn handle_command(cmd: RunCommand, _output_kind: OutputKind) -> Result<CommandOutput> {
    let runtime = LocalRuntime::new()?;
    let mut ctx_builder = CtxBuilder::new();

    ctx_builder.set_reference(cmd.reference.clone());
    ctx_builder.wasi_ctx().args(&cmd.args);

    handle_env(&mut ctx_builder, &cmd.env);
    handle_dir(&mut ctx_builder, &cmd.dir)?;

    runtime.run(ctx_builder.build()?).await?;

    return anyhow::Ok(CommandOutput::from("Command in progress"));
}

fn handle_env(ctx_build: &mut CtxBuilder, env_opts: &EnvironmentOptions) {
    if env_opts.allow_all {
        ctx_build.wasi_ctx().inherit_env();
    } else {
        for env_name in &env_opts.allow {
            if let Ok(value) = std::env::var(env_name.as_ref()) {
                ctx_build.wasi_ctx().env(env_name, value);
            } else {
                warn!(env_name, "allowed environment is not set");
            }
        }

        for env_prefix in &env_opts.allow_prefix {
            ctx_build.env_with_prefix(env_prefix);
        }
    }
}

fn handle_dir(ctx_build: &mut CtxBuilder, dir_opts: &DirOptions) -> anyhow::Result<()> {
    for dir in &dir_opts.immutable {
        let parsed_dir = parse_dir(dir).context("failed to parse a dir flag")?;
        ctx_build.wasi_ctx().preopened_dir(
            parsed_dir.path,
            parsed_dir.guest_dir.map(String::as_str).unwrap_or("."),
            DirPerms::READ,
            parsed_dir.file_perms.unwrap_or(FilePerms::ReadOnly).into(),
        ).context("failed pre-opening a dir")?;
    }

    for dir in &dir_opts.mutable {
        let parsed_dir = parse_dir(dir).context("failed to parse a dir flag")?;
        ctx_build.wasi_ctx().preopened_dir(
            parsed_dir.path,
            parsed_dir.guest_dir.map(String::as_str).unwrap_or("."),
            DirPerms::MUTATE,
            parsed_dir.file_perms.unwrap_or(FilePerms::ReadOnly).into(),
        ).context("failed pre-opening a dir")?;
    }

    return Ok(());
}

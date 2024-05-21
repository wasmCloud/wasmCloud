use anyhow::{anyhow, Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::warn;
use wash_lib::cli::{CommandOutput, OutputKind};
use wash_lib::run::{
    CtxBuilder, DirPerms, FilePerms as WasmtimeFilePerms, LocalRuntime, SocketAddrUse,
};

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
    #[clap(long = "allow-net-dns")]
    pub allow_name_resolution: bool,

    /// Allow the component to connect to peers.
    #[clap(long = "allow-net-connect")]
    pub allow_connect: bool,

    /// Allow the component to bind to network ports.
    #[clap(long = "allow-net-bind")]
    pub allow_bind: bool,
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
            _ => Err(anyhow!(
                "file permission is incorrect, 'ro' or 'rw' expected"
            )),
        }
    }
}

impl From<FilePerms> for WasmtimeFilePerms {
    fn from(value: FilePerms) -> Self {
        match value {
            FilePerms::ReadOnly => WasmtimeFilePerms::READ,
            FilePerms::ReadWrite => WasmtimeFilePerms::WRITE | WasmtimeFilePerms::READ,
        }
    }
}

fn parse_dir(raw: impl AsRef<str>) -> Result<ParsedDir> {
    let mut parts = raw.as_ref().split(':');

    let raw_path = parts
        .next()
        .ok_or(anyhow!("dir flags require at least a path"))?;

    Ok(ParsedDir {
        path: PathBuf::from(raw_path),
        file_perms: parts.next().map(FilePerms::from_str).transpose()?,
        guest_dir: parts.next().map(|raw| raw.to_owned()),
    })
}

pub async fn handle_command(cmd: RunCommand, _output_kind: OutputKind) -> Result<CommandOutput> {
    let runtime = LocalRuntime::new()?;
    let mut ctx_builder = CtxBuilder::new();

    ctx_builder.set_reference(cmd.reference.clone());
    ctx_builder.wasi_ctx().args(&cmd.args);

    ctx_builder.wasi_ctx().inherit_stdio();

    handle_env(&mut ctx_builder, &cmd.env);
    handle_dir(&mut ctx_builder, &cmd.dir)?;
    handle_net(&mut ctx_builder, &cmd.net);

    runtime.run(ctx_builder.build()?).await?;

    // Q(raskyld): Is there a way to avoid producing outputs and let the run component handle stdout, err, in?
    anyhow::Ok(CommandOutput::from(""))
}

fn handle_env(ctx_build: &mut CtxBuilder, env_opts: &EnvironmentOptions) {
    if env_opts.allow_all {
        ctx_build.wasi_ctx().inherit_env();
    } else {
        for env_name in &env_opts.allow {
            if let Ok(value) = std::env::var(env_name.as_str()) {
                ctx_build.wasi_ctx().env(env_name.as_str(), value.as_str());
            } else {
                warn!(env_name, "allowed environment is not set");
            }
        }

        for env_prefix in &env_opts.allow_prefix {
            ctx_build.env_with_prefix(env_prefix);
        }
    }
}

fn handle_dir(ctx_build: &mut CtxBuilder, dir_opts: &DirOptions) -> Result<()> {
    for dir in &dir_opts.immutable {
        let parsed_dir = parse_dir(dir).context("failed to parse a dir flag")?;
        ctx_build
            .wasi_ctx()
            .preopened_dir(
                parsed_dir.path,
                parsed_dir.guest_dir.unwrap_or(String::from(".")),
                DirPerms::READ,
                parsed_dir.file_perms.unwrap_or(FilePerms::ReadOnly).into(),
            )
            .context("failed pre-opening a dir")?;
    }

    for dir in &dir_opts.mutable {
        let parsed_dir = parse_dir(dir).context("failed to parse a dir flag")?;
        ctx_build
            .wasi_ctx()
            .preopened_dir(
                parsed_dir.path,
                parsed_dir.guest_dir.unwrap_or(String::from(".")),
                DirPerms::READ | DirPerms::MUTATE,
                parsed_dir.file_perms.unwrap_or(FilePerms::ReadOnly).into(),
            )
            .context("failed pre-opening a dir")?;
    }

    Ok(())
}

fn handle_net(ctx_build: &mut CtxBuilder, net_opts: &NetworkOptions) {
    let &NetworkOptions {
        allow_bind: bind,
        allow_connect: connect,
        allow_name_resolution: name_resolution,
    } = net_opts;

    ctx_build
        .wasi_ctx()
        .socket_addr_check(move |_, usage| match usage {
            SocketAddrUse::TcpBind | SocketAddrUse::UdpBind => bind,
            SocketAddrUse::TcpConnect | SocketAddrUse::UdpConnect => connect,
            SocketAddrUse::UdpOutgoingDatagram => connect,
        });

    ctx_build.wasi_ctx().allow_ip_name_lookup(name_resolution);
}

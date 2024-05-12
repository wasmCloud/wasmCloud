use anyhow::Result;
use clap::Parser;
use wash_lib::cli::{CommandOutput, OutputKind};

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

pub async fn handle_command(_cmd: RunCommand, _output_kind: OutputKind) -> Result<CommandOutput> {
    Ok(CommandOutput::from("This command is not implemented yet."))
}

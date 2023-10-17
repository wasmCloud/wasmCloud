use std::{collections::HashMap, path::PathBuf, time::Duration};

use anyhow::{bail, Context, Result};
use clap::Args;
use log::{debug, error};
use wash_lib::cli::CommandOutput;
use wash_lib::config::{context_dir, DEFAULT_LATTICE_PREFIX, DEFAULT_NATS_HOST, DEFAULT_NATS_PORT};
use wash_lib::context::{
    ensure_host_config_context,
    fs::{load_context, ContextDir},
    ContextManager,
};
use wash_lib::id::{ClusterSeed, ModuleId};
use wasmbus_rpc::{common::Message, core::WasmCloudEntity, rpc_client::RpcClient};
use wasmcloud_test_util::testing::TestResults;

use crate::util::{
    default_timeout_ms, extract_arg_value, json_str_to_msgpack_bytes, msgpack_to_json_val,
    nats_client_from_opts,
};

/// fake key (not a real public key)  used to construct origin for invoking actors
const WASH_ORIGIN_KEY: &str = "__WASH__";

/// hostname used for actor invocations
const WASH_HOST_ID: &str = "NwashHostCallerId000000000000000000000000000000000000000";

#[derive(Debug, Args, Clone)]
#[clap(name = "call")]
pub(crate) struct CallCli {
    #[clap(flatten)]
    command: CallCommand,
}

impl CallCli {
    pub(crate) fn command(self) -> CallCommand {
        self.command
    }
}

pub(crate) async fn handle_command(cmd: CallCommand) -> Result<CommandOutput> {
    let is_test = cmd.test;
    let save_output = cmd.save.clone();
    let bin = cmd.bin;
    let res = handle_call(cmd).await?;
    call_output(res, save_output, bin, is_test)
}

#[derive(Debug, Clone, Args)]
pub(crate) struct ConnectionOpts {
    /// RPC Host for connection, defaults to 127.0.0.1 for local nats
    #[clap(short = 'r', long = "rpc-host", env = "WASMCLOUD_RPC_HOST")]
    rpc_host: Option<String>,

    /// RPC Port for connections, defaults to 4222 for local nats
    #[clap(short = 'p', long = "rpc-port", env = "WASMCLOUD_RPC_PORT")]
    rpc_port: Option<String>,

    /// JWT file for RPC authentication. Must be supplied with rpc_seed.
    #[clap(long = "rpc-jwt", env = "WASMCLOUD_RPC_JWT", hide_env_values = true)]
    rpc_jwt: Option<String>,

    /// Seed file or literal for RPC authentication. Must be supplied with rpc_jwt.
    #[clap(long = "rpc-seed", env = "WASMCLOUD_RPC_SEED", hide_env_values = true)]
    rpc_seed: Option<String>,

    /// Credsfile for RPC authentication. Combines rpc_seed and rpc_jwt.
    /// See https://docs.nats.io/developing-with-nats/security/creds for details.
    #[clap(long = "rpc-credsfile", env = "WASH_RPC_CREDS", hide_env_values = true)]
    rpc_credsfile: Option<PathBuf>,

    /// Lattice prefix for wasmcloud command interface, defaults to "default"
    #[clap(short = 'x', long = "lattice-prefix", env = "WASMCLOUD_LATTICE_PREFIX")]
    lattice_prefix: Option<String>,

    /// Timeout length for RPC, defaults to 2000 milliseconds
    #[clap(
        short = 't',
        long = "rpc-timeout-ms",
        default_value_t = default_timeout_ms(),
        env = "WASMCLOUD_RPC_TIMEOUT_MS"
    )]
    timeout_ms: u64,

    /// Path to a context with values to use for RPC connection, authentication, and cluster seed invocation signing
    #[clap(long = "context")]
    pub(crate) context: Option<PathBuf>,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct CallCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,

    /// Optional json file to send as the operation payload
    #[clap(short, long)]
    pub(crate) data: Option<PathBuf>,

    /// Optional file for saving binary response
    #[clap(long)]
    pub(crate) save: Option<PathBuf>,

    /// When using json output, display binary as binary('b'), string('s'), or both('2')
    #[clap(long, default_value = "b")]
    pub(crate) bin: char,

    /// When invoking a test actor, interpret the response as TestResults
    #[clap(long)]
    pub(crate) test: bool,

    /// wasmCloud host cluster seed. This cluster seed must match the cluster seed used to
    /// launch the wasmCloud host in order to pass antiforgery checks made by the host
    /// This is only optional if a default context is available or a context is provided
    #[clap(
        short = 'c',
        long = "cluster-seed",
        env = "WASMCLOUD_CLUSTER_SEED",
        value_parser
    )]
    pub(crate) cluster_seed: Option<ClusterSeed>,

    /// Public key or OCI reference of actor
    #[clap(name = "actor-id")]
    pub(crate) actor_id: ModuleId,

    /// Operation to invoke on actor
    #[clap(name = "operation")]
    pub(crate) operation: String,

    /// Payload to send with operation (in the form of '{"field": "value"}' )
    #[clap(name = "payload")]
    pub(crate) payload: Vec<String>,
}

pub(crate) async fn handle_call(cmd: CallCommand) -> Result<Vec<u8>> {
    debug!(
        "calling actor with operation: {}, data: {}",
        &cmd.operation,
        cmd.payload.join("")
    );
    if !"bs2".contains(cmd.bin) {
        bail!("'bin' parameter must be 'b', 's', or '2'");
    }

    let origin = WasmCloudEntity::new_actor(WASH_ORIGIN_KEY)?;
    let target = WasmCloudEntity::new_actor(&cmd.actor_id)?;

    if cmd.data.is_some() && !cmd.payload.is_empty() {
        bail!("you can use either -d/--data or the payload args, but not both.");
    }
    let payload = if let Some(fname) = cmd.data {
        std::fs::read_to_string(fname)?
    } else {
        cmd.payload.join("")
    };
    debug!(
        "calling actor with operation: {}, data: {}",
        &cmd.operation, &payload
    );
    let bytes = json_str_to_msgpack_bytes(&payload)?;
    let lattice_prefix = cmd
        .opts
        .lattice_prefix
        .clone()
        .unwrap_or_else(|| DEFAULT_LATTICE_PREFIX.to_string());

    let (client, timeout_ms) = rpc_client_from_opts(cmd.opts, cmd.cluster_seed).await?;
    Ok(client
        .send_timeout(
            origin,
            target,
            &lattice_prefix,
            Message {
                method: &cmd.operation,
                arg: bytes.into(),
            },
            Duration::from_millis(timeout_ms),
        )
        .await?)
}

// Helper output functions, used to ensure consistent output between call & standalone commands
pub(crate) fn call_output(
    response: Vec<u8>,
    save_output: Option<PathBuf>,
    bin: char,
    is_test: bool,
) -> Result<CommandOutput> {
    if let Some(ref save_path) = save_output {
        std::fs::write(save_path, response)
            .with_context(|| format!("Error saving results to {}", &save_path.display()))?;

        return Ok(CommandOutput::new(
            "",
            HashMap::<String, serde_json::Value>::new(),
        ));
    }
    if is_test {
        // try to decode it as TestResults, otherwise dump as text
        let test_results = wasmbus_rpc::common::deserialize::<TestResults>(&response)
            .with_context(|| {
                format!(
                    "Error interpreting response as TestResults. Response: {}",
                    String::from_utf8_lossy(&response)
                )
            })?;

        wasmcloud_test_util::cli::print_test_results(&test_results);
        return Ok(CommandOutput::new(
            "",
            HashMap::<String, serde_json::Value>::new(),
        ));
    }

    let mut json = HashMap::new();
    json.insert(
        "response".to_string(),
        msgpack_to_json_val(response.clone(), bin),
    );

    Ok(CommandOutput::new(
        format!(
            "\nCall response (raw): {}",
            String::from_utf8_lossy(&response)
        ),
        json,
    ))
}

async fn rpc_client_from_opts(
    opts: ConnectionOpts,
    cmd_cluster_seed: Option<ClusterSeed>,
) -> Result<(RpcClient, u64)> {
    let ctx = if let Some(context) = opts.context {
        Some(load_context(context)?)
    } else if let Ok(ctx_dir) = context_dir(None) {
        let ctx_dir = ContextDir::new(ctx_dir)?;
        ensure_host_config_context(&ctx_dir)?;
        Some(ctx_dir.load_default_context()?)
    } else {
        None
    };

    // Determine connection parameters, taking explicitly provided flags,
    // then provided context values, lastly using defaults

    let rpc_host = opts.rpc_host.unwrap_or_else(|| {
        ctx.as_ref()
            .map(|c| c.rpc_host.clone())
            .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string())
    });

    let rpc_port = opts.rpc_port.unwrap_or_else(|| {
        ctx.as_ref()
            .map(|c| c.rpc_port.to_string())
            .unwrap_or_else(|| DEFAULT_NATS_PORT.to_string())
    });

    let rpc_jwt = if opts.rpc_jwt.is_some() {
        opts.rpc_jwt
    } else {
        ctx.as_ref().map(|c| c.rpc_jwt.clone()).unwrap_or_default()
    };

    let rpc_seed = if opts.rpc_seed.is_some() {
        opts.rpc_seed
    } else {
        ctx.as_ref().map(|c| c.rpc_seed.clone()).unwrap_or_default()
    };

    let rpc_credsfile = if opts.rpc_credsfile.is_some() {
        opts.rpc_credsfile
    } else {
        ctx.as_ref()
            .map(|c| c.rpc_credsfile.clone())
            .unwrap_or_default()
    };

    // Cluster seed is optional on the CLI to allow for context to supply that variable.
    // If no context is supplied, and there is no default context, then the cluster seed
    // cannot be determined and the RPC will almost certainly fail, unless the antiforgery
    // check allows the invocation to be unsigned.
    let cluster_seed = cmd_cluster_seed.unwrap_or_else(|| {
        ctx.as_ref()
            .map(|c| {
                c.cluster_seed.clone().unwrap_or_else(|| {
                    error!(
                        "No cluster seed provided and no context available, this RPC will fail."
                    );
                    ClusterSeed::default()
                })
            })
            .unwrap_or_default()
    });

    let nc = nats_client_from_opts(&rpc_host, &rpc_port, rpc_jwt, rpc_seed, rpc_credsfile).await?;

    Ok((
        RpcClient::new(
            nc,
            WASH_HOST_ID.to_string(),
            Some(Duration::from_millis(opts.timeout_ms)),
            std::sync::Arc::new(nkeys::KeyPair::from_seed(&extract_arg_value(
                cluster_seed.as_ref(),
            )?)?),
        ),
        opts.timeout_ms,
    ))
}

#[cfg(test)]
mod test {
    use super::CallCommand;
    use anyhow::Result;
    use clap::Parser;
    use std::{path::PathBuf, str::FromStr};
    use wash_lib::id::ModuleId;

    const RPC_HOST: &str = "127.0.0.1";
    const RPC_PORT: &str = "4222";
    const LATTICE_PREFIX: &str = "default";
    const SAVE_FNAME: &str = "/dev/null";
    const DATA_FNAME: &str = "/tmp/data.json";

    const ACTOR_ID: &str = "MDPDJEYIAK6MACO67PRFGOSSLODBISK4SCEYDY3HEOY4P5CVJN6UCWUK";

    #[derive(Debug, Parser)]
    struct Cmd {
        #[clap(flatten)]
        command: CallCommand,
    }

    #[test]
    fn test_rpc_comprehensive() -> Result<()> {
        let call_all: Cmd = Parser::try_parse_from([
            "call",
            "--test",
            "--data",
            DATA_FNAME,
            "--save",
            SAVE_FNAME,
            "--bin",
            "2",
            "--context",
            "~/.wash/contexts/default.json",
            "--cluster-seed",
            "SCAMSVN4M2NZ65RWGYE42BZZ7VYEFEAAHGLIY7R4W7CRHORSMXTDJRKXLY",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout-ms",
            "0",
            ACTOR_ID,
            "HandleOperation",
            "{ \"hello\": \"world\"}",
        ])?;
        match call_all.command {
            CallCommand {
                opts,
                data,
                save,
                bin,
                test,
                actor_id,
                operation,
                payload,
                cluster_seed,
            } => {
                assert_eq!(&opts.rpc_host.unwrap(), RPC_HOST);
                assert_eq!(&opts.rpc_port.unwrap(), RPC_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.timeout_ms, 0);
                assert_eq!(
                    opts.context,
                    Some(PathBuf::from("~/.wash/contexts/default.json"))
                );
                assert_eq!(data, Some(PathBuf::from(DATA_FNAME)));
                assert_eq!(save, Some(PathBuf::from(SAVE_FNAME)));
                assert_eq!(
                    cluster_seed.unwrap(),
                    "SCAMSVN4M2NZ65RWGYE42BZZ7VYEFEAAHGLIY7R4W7CRHORSMXTDJRKXLY"
                        .parse()
                        .unwrap()
                );
                assert!(test);
                assert_eq!(bin, '2');
                assert_eq!(actor_id, ModuleId::from_str(ACTOR_ID).unwrap());
                assert_eq!(operation, "HandleOperation");
                assert_eq!(payload, vec!["{ \"hello\": \"world\"}".to_string()])
            }
            #[allow(unreachable_patterns)]
            cmd => panic!("call constructed incorrect command: {cmd:?}"),
        }
        Ok(())
    }
}

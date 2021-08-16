use crate::util::Result;
use crate::util::{
    convert_rpc_error, extract_arg_value, format_output, json_str_to_msgpack_bytes,
    nats_client_from_opts, Output, OutputKind,
};
use serde_json::json;
use structopt::clap::AppSettings;
use structopt::StructOpt;
use wasmbus_rpc::{core::WasmCloudEntity, Message, RpcClient};

/// fake key (not a real public key)  used to construct origin for invoking actors
const WASH_ORIGIN_KEY: &str = "__WASH__";

#[derive(Debug, StructOpt, Clone)]
#[structopt(
    global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
    name = "call")]
pub(crate) struct CallCli {
    #[structopt(flatten)]
    command: CallCommand,
}

impl CallCli {
    pub(crate) fn command(self) -> CallCommand {
        self.command
    }
}

pub(crate) async fn handle_command(cmd: CallCommand) -> Result<String> {
    let output_kind = cmd.output.kind;
    let res = handle_call(cmd).await;
    //TODO: Evaluate from_utf8_lossy and use of format here
    Ok(call_output(res, &output_kind))
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct ConnectionOpts {
    /// RPC Host for connection, defaults to 0.0.0.0 for local nats
    #[structopt(
        short = "r",
        long = "rpc-host",
        default_value = "0.0.0.0",
        env = "WASH_RPC_HOST"
    )]
    rpc_host: String,

    /// RPC Port for connections, defaults to 4222 for local nats
    #[structopt(
        short = "p",
        long = "rpc-port",
        default_value = "4222",
        env = "WASH_RPC_PORT"
    )]
    rpc_port: String,

    /// JWT file for RPC authentication. Must be supplied with rpc_seed.
    #[structopt(long = "rpc-jwt", env = "WASH_RPC_JWT", hide_env_values = true)]
    rpc_jwt: Option<String>,

    /// Seed file or literal for RPC authentication. Must be supplied with rpc_jwt.
    #[structopt(long = "rpc-seed", env = "WASH_RPC_SEED", hide_env_values = true)]
    rpc_seed: Option<String>,

    /// Credsfile for RPC authentication. Combines rpc_seed and rpc_jwt.
    /// See https://docs.nats.io/developing-with-nats/security/creds for details.
    #[structopt(long = "rpc-credsfile", env = "WASH_RPC_CREDS", hide_env_values = true)]
    rpc_credsfile: Option<String>,

    /// Namespace prefix for wasmcloud command interface
    #[structopt(
        short = "n",
        long = "ns-prefix",
        default_value = "default",
        env = "WASH_RPC_NSPREFIX"
    )]
    ns_prefix: String,

    /// Timeout length for RPC, defaults to 1 second
    #[structopt(
        short = "t",
        long = "rpc-timeout",
        default_value = "1",
        env = "WASH_RPC_TIMEOUT"
    )]
    timeout: u64,
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct CallCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(flatten)]
    pub(crate) output: Output,

    /// Public key or OCI reference of actor
    #[structopt(name = "actor-id")]
    pub(crate) actor_id: String,

    /// Operation to invoke on actor
    #[structopt(name = "operation")]
    pub(crate) operation: String,

    /// Payload to send with operation (in the form of '{"field": "value"}' )
    #[structopt(name = "data")]
    pub(crate) data: Vec<String>,
}

pub(crate) async fn handle_call(cmd: CallCommand) -> Result<Vec<u8>> {
    log::debug!(
        "calling actor with operation: {}, data: {}",
        &cmd.operation,
        cmd.data.join("")
    );

    let origin = WasmCloudEntity::new_actor(WASH_ORIGIN_KEY)?;
    let target = WasmCloudEntity::new_actor(&cmd.actor_id)?;

    let seed = cmd
        .opts
        .rpc_seed
        .unwrap_or_else(|| nkeys::KeyPair::new_user().seed().unwrap());
    let nc = nats_client_from_opts(
        &cmd.opts.rpc_host,
        &cmd.opts.rpc_port,
        cmd.opts.rpc_jwt,
        Some(seed.to_string()),
        cmd.opts.rpc_credsfile,
    )
    .await?;

    let client = RpcClient::new_asynk(
        nc,
        &cmd.opts.ns_prefix,
        nkeys::KeyPair::from_seed(&extract_arg_value(&seed)?)?,
    );
    let bytes = json_str_to_msgpack_bytes(cmd.data)?;
    client
        .send(
            origin,
            target,
            Message {
                method: &cmd.operation,
                arg: bytes.into(),
            },
        )
        .await
        .map_err(convert_rpc_error)
}

// Helper output functions, used to ensure consistent output between ctl & standalone commands
pub(crate) fn call_output(response: Result<Vec<u8>>, output_kind: &OutputKind) -> String {
    match response {
        Ok(msg) => {
            //TODO(issue #32): String::from_utf8_lossy should be decoder only if one is not available
            let call_response = String::from_utf8_lossy(&msg);
            format_output(
                format!("\nCall response (raw): {}", call_response),
                json!({ "response": call_response }),
                output_kind,
            )
        }
        Err(e) => format_output(
            format!("\nError invoking actor: {}", e),
            json!({ "error": format!("{}", e) }),
            output_kind,
        ),
    }
}

#[cfg(test)]
mod test {
    use super::{CallCli, CallCommand};
    use crate::util::Result;
    use structopt::StructOpt;

    const RPC_HOST: &str = "0.0.0.0";
    const RPC_PORT: &str = "4222";
    const NS_PREFIX: &str = "default";

    const ACTOR_ID: &str = "MDPDJEYIAK6MACO67PRFGOSSLODBISK4SCEYDY3HEOY4P5CVJN6UCWUK";

    #[test]
    fn test_rpc_comprehensive() -> Result<()> {
        let call_all = CallCli::from_iter_safe(&[
            "call",
            "-o",
            "json",
            "--ns-prefix",
            NS_PREFIX,
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout",
            "0",
            ACTOR_ID,
            "HandleOperation",
            "{ \"hello\": \"world\"}",
        ])?;
        match call_all.command {
            CallCommand {
                opts,
                output,
                actor_id,
                operation,
                data,
            } => {
                assert_eq!(opts.rpc_host, RPC_HOST);
                assert_eq!(opts.rpc_port, RPC_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.timeout, 0);
                assert_eq!(output.kind, crate::util::OutputKind::Json);
                assert_eq!(actor_id, ACTOR_ID);
                assert_eq!(operation, "HandleOperation");
                assert_eq!(data, vec!["{ \"hello\": \"world\"}".to_string()])
            }
            #[allow(unreachable_patterns)]
            cmd => panic!("call constructed incorrect command: {:?}", cmd),
        }
        Ok(())
    }
}

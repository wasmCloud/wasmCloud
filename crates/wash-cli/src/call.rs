use core::time::Duration;

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{ensure, Context, Result};
use clap::Args;
use log::{debug, error};
use serde::Deserialize;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use wash_lib::cli::CommandOutput;
use wash_lib::config::{create_nats_client_from_opts, DEFAULT_LATTICE_PREFIX};
use wash_lib::context::{fs::ContextDir, ContextManager};
use wash_lib::id::{ClusterSeed, ModuleId};
use wasmcloud_core::{InvocationResponse, WasmCloudEntity};
use wasmcloud_provider_sdk::rpc_client::RpcClient;

use crate::util::{
    default_timeout_ms, extract_arg_value, json_str_to_msgpack_bytes, msgpack_to_json_val,
};

#[derive(Deserialize)]
struct TestResult {
    /// test case name
    #[serde(default)]
    pub name: String,
    /// true if the test case passed
    #[serde(default)]
    pub passed: bool,
    /// (optional) more detailed results, if available.
    /// data is snap-compressed json
    /// failed tests should have a firsts-level key called "error".
    #[serde(rename = "snapData")]
    #[serde(with = "serde_bytes")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snap_data: Option<Vec<u8>>,
}

/// Prints test results (with handy color!) to the terminal
// NOTE(thomastaylor312): We are unwrapping all writing IO errors (which matches the behavior in the
// println! macro) and swallowing the color change errors as there isn't much we can do if they fail
// (and a color change isn't the end of the world). We may want to update this function in the
// future to return an io::Result
fn print_test_results(results: &[TestResult]) {
    // structure for deserializing error results
    #[derive(Deserialize)]
    struct ErrorReport {
        error: String,
    }

    let mut passed = 0u32;
    let total = results.len() as u32;
    // TODO(thomastaylor312): We can probably improve this a bit by using the `atty` crate to choose
    // whether or not to colorize the text
    let mut stdout = StandardStream::stdout(ColorChoice::Always);
    let mut green = ColorSpec::new();
    green.set_fg(Some(Color::Green));
    let mut red = ColorSpec::new();
    red.set_fg(Some(Color::Red));
    for test in results.iter() {
        if test.passed {
            let _ = stdout.set_color(&green);
            write!(&mut stdout, "Pass").unwrap();
            let _ = stdout.reset();
            writeln!(&mut stdout, ": {}", test.name).unwrap();
            passed += 1;
        } else {
            let error_msg = test
                .snap_data
                .as_ref()
                .map(|bytes| {
                    serde_json::from_slice::<ErrorReport>(bytes)
                        .map(|r| r.error)
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let _ = stdout.set_color(&red);
            write!(&mut stdout, "Fail").unwrap();
            let _ = stdout.reset();
            writeln!(&mut stdout, ": {}", error_msg).unwrap();
        }
    }
    let status_color = if passed == total { green } else { red };
    write!(&mut stdout, "Test results: ").unwrap();
    let _ = stdout.set_color(&status_color);
    writeln!(&mut stdout, "{}/{} Passed", passed, total).unwrap();
    // Reset the color settings back to what the user configured
    let _ = stdout.set_color(&ColorSpec::new());
    writeln!(&mut stdout).unwrap();
}

/// hostname used for actor invocations
const WASH_HOST_ID: &str = "NAWASHVALZUZPZNXPIF6HGQ4OMJYLXQ4B2WZZ5AMBCXKWEQPYXDOIWMA"; // "a wash val" :)

#[derive(Debug, Args, Clone)]
#[clap(name = "call")]
pub struct CallCli {
    #[clap(flatten)]
    command: CallCommand,
}

impl CallCli {
    pub fn command(self) -> CallCommand {
        self.command
    }
}

pub async fn handle_command(cmd: CallCommand) -> Result<CommandOutput> {
    let is_test = cmd.test;
    let save_output = cmd.save.clone();
    let bin = cmd.bin;
    let res = handle_call(cmd).await?;
    call_output(res, save_output, bin, is_test)
}

#[derive(Debug, Clone, Args)]
pub struct ConnectionOpts {
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

    /// Name of the context to use for RPC connection, authentication, and cluster seed invocation signing
    #[clap(long = "context")]
    pub context: Option<String>,
}

#[derive(Args, Debug, Clone)]
pub struct CallCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,

    /// Optional json file to send as the operation payload
    #[clap(short, long)]
    pub data: Option<PathBuf>,

    /// Optional file for saving binary response
    #[clap(long)]
    pub save: Option<PathBuf>,

    /// When using json output, display binary as binary('b'), string('s'), or both('2')
    #[clap(long, default_value = "b")]
    pub bin: char,

    /// When invoking a test actor, interpret the response as TestResults
    #[clap(long)]
    pub test: bool,

    /// wasmCloud host cluster seed. This cluster seed must match the cluster seed used to
    /// launch the wasmCloud host in order to pass antiforgery checks made by the host
    /// This is only optional if a default context is available or a context is provided
    #[clap(
        short = 'c',
        long = "cluster-seed",
        env = "WASMCLOUD_CLUSTER_SEED",
        value_parser
    )]
    pub cluster_seed: Option<ClusterSeed>,

    /// Public key or OCI reference of actor
    #[clap(name = "actor-id")]
    pub actor_id: ModuleId,

    /// Operation to invoke on actor
    #[clap(name = "operation")]
    pub operation: String,

    /// Payload to send with operation (in the form of '{"field": "value"}' )
    #[clap(name = "payload")]
    pub payload: Vec<String>,
}

pub async fn handle_call(
    CallCommand {
        opts,
        data,
        bin,
        cluster_seed,
        actor_id,
        operation,
        payload,
        ..
    }: CallCommand,
) -> Result<Vec<u8>> {
    debug!(
        "calling actor with operation: {}, data: {}",
        &operation,
        payload.join("")
    );
    ensure!(
        "bs2".contains(bin),
        "'bin' parameter must be 'b', 's', or '2'"
    );
    ensure!(
        data.is_none() || payload.is_empty(),
        "you can use either -d/--data or the payload args, but not both."
    );
    ensure!(!actor_id.is_empty(), "actor ID may not be empty");

    let payload = if let Some(fname) = data {
        std::fs::read_to_string(fname)?
    } else {
        payload.join("")
    };
    debug!(
        "calling actor with operation: {}, data: {}",
        &operation, &payload
    );
    let bytes = json_str_to_msgpack_bytes(&payload)?;
    let (client, timeout_ms) = rpc_client_from_opts(opts, cluster_seed).await?;
    let InvocationResponse { msg, .. } = client
        .send_timeout(
            WasmCloudEntity {
                public_key: actor_id.to_string(), // This is "wrong" in the sense that an actor shouldn't be calling itself, but it ensures the receiving host has both the origin and target public keys in its claims
                ..Default::default()
            },
            WasmCloudEntity {
                public_key: actor_id.to_string(),
                ..Default::default()
            },
            operation,
            bytes,
            Duration::from_millis(timeout_ms),
        )
        .await?;
    Ok(msg)
}

// Helper output functions, used to ensure consistent output between call & standalone commands
pub fn call_output(
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
        let test_results: Vec<TestResult> =
            rmp_serde::from_slice(&response).with_context(|| {
                format!(
                    "Error interpreting response as TestResults. Response: {}",
                    String::from_utf8_lossy(&response)
                )
            })?;

        print_test_results(&test_results);
        return Ok(CommandOutput::new(
            "",
            HashMap::<String, serde_json::Value>::new(),
        ));
    }

    let json = HashMap::from([
        (
            "response".to_string(),
            msgpack_to_json_val(response.clone(), bin),
        ),
        ("success".to_string(), serde_json::json!(true)),
    ]);

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
    // Attempt to load a context, falling back on the default if not supplied
    let ctx_dir = ContextDir::new()?;
    let ctx = if let Some(context_name) = opts.context {
        ctx_dir
            .load_context(&context_name)
            .with_context(|| format!("failed to load context `{context_name}`"))?
    } else {
        ctx_dir
            .load_default_context()
            .context("failed to load default context")?
    };

    // Determine connection parameters, taking explicitly provided flags,
    // then provided context values, lastly using defaults

    let rpc_host = opts.rpc_host.unwrap_or_else(|| ctx.rpc_host.clone());
    let rpc_port = opts.rpc_port.unwrap_or_else(|| ctx.rpc_port.to_string());
    let rpc_jwt = opts.rpc_jwt.or_else(|| ctx.rpc_jwt.clone());
    let rpc_seed = opts.rpc_seed.or_else(|| ctx.rpc_seed.clone());
    let rpc_credsfile = opts.rpc_credsfile.or_else(|| ctx.rpc_credsfile.clone());

    // Cluster seed is optional on the CLI to allow for context to supply that variable.
    // If no context is supplied, and there is no default context, then the cluster seed
    // cannot be determined and the RPC will almost certainly fail, unless the antiforgery
    // check allows the invocation to be unsigned.
    let cluster_seed = cmd_cluster_seed.unwrap_or_else(|| {
        ctx.cluster_seed.clone().unwrap_or_else(|| {
            error!("No cluster seed provided and no context available, this RPC will fail.");
            ClusterSeed::default()
        })
    });

    let nc = create_nats_client_from_opts(&rpc_host, &rpc_port, rpc_jwt, rpc_seed, rpc_credsfile)
        .await?;

    let lattice_prefix = opts
        .lattice_prefix
        .as_deref()
        .unwrap_or(DEFAULT_LATTICE_PREFIX);
    Ok((
        RpcClient::new(
            nc,
            WASH_HOST_ID.to_string(),
            Some(Duration::from_millis(opts.timeout_ms)),
            Arc::new(nkeys::KeyPair::from_seed(&extract_arg_value(
                cluster_seed.as_ref(),
            )?)?),
            lattice_prefix,
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
            "some-context",
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
                assert_eq!(opts.context, Some("some-context".to_string()));
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

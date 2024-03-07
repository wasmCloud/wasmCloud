use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{bail, ensure, Context, Result};
use clap::Args;
use serde::Deserialize;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use tracing::debug;
use wash_lib::cli::CommandOutput;
use wash_lib::config::{create_nats_client_from_opts, DEFAULT_LATTICE};
use wrpc_transport::Client;

use crate::util::{default_timeout_ms, msgpack_to_json_val};

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
    let res = handle_call(cmd).await?;
    Ok(CommandOutput::new(
        res.clone(),
        HashMap::from_iter(vec![(
            "response".to_string(),
            serde_json::Value::String(res),
        )]),
    ))
}

#[derive(Debug, Clone, Args)]
pub struct ConnectionOpts {
    /// RPC Host for connection, defaults to 127.0.0.1 for local nats
    #[clap(
        short = 'r',
        long = "rpc-host",
        env = "WASMCLOUD_RPC_HOST",
        default_value = "127.0.0.1"
    )]
    rpc_host: String,

    /// RPC Port for connections, defaults to 4222 for local nats
    #[clap(
        short = 'p',
        long = "rpc-port",
        env = "WASMCLOUD_RPC_PORT",
        default_value = "4222"
    )]
    rpc_port: String,

    /// JWT file for RPC authentication. Must be supplied with rpc_seed.
    #[clap(
        long = "rpc-jwt",
        env = "WASMCLOUD_RPC_JWT",
        hide_env_values = true,
        requires = "rpc_seed"
    )]
    rpc_jwt: Option<String>,

    /// Seed file or literal for RPC authentication. Must be supplied with rpc_jwt.
    #[clap(
        long = "rpc-seed",
        env = "WASMCLOUD_RPC_SEED",
        hide_env_values = true,
        requires = "rpc_jwt"
    )]
    rpc_seed: Option<String>,

    /// Credsfile for RPC authentication. Combines rpc_seed and rpc_jwt.
    /// See https://docs.nats.io/using-nats/developer/connecting/creds for details.
    #[clap(long = "rpc-credsfile", env = "WASH_RPC_CREDS", hide_env_values = true)]
    rpc_credsfile: Option<PathBuf>,

    /// Lattice for wasmcloud command interface, defaults to "default"
    #[clap(short = 'x', long = "lattice", env = "WASMCLOUD_LATTICE")]
    lattice: Option<String>,

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

    /// The unique component identifier of the component to invoke
    #[clap(name = "component-id")]
    pub component_id: String,

    /// Fully qualified function to invoke on the actor, e.g. `wasi:cli/run.run`
    #[clap(name = "function")]
    pub function: String,
}

pub async fn handle_call(
    CallCommand {
        opts,
        component_id,
        function,
        ..
    }: CallCommand,
) -> Result<String> {
    ensure!(!component_id.is_empty(), "component ID may not be empty");
    debug!("calling component over wRPC with function: {function}, expecting String response");

    let nc = create_nats_client_from_opts(
        &opts.rpc_host,
        &opts.rpc_port,
        opts.rpc_jwt,
        opts.rpc_seed,
        opts.rpc_credsfile,
    )
    .await?;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("source-id", "wash");
    let lattice = opts.lattice.unwrap_or_else(|| DEFAULT_LATTICE.to_string());
    let wrpc_client =
        wasmcloud_core::wrpc::Client::new(nc, format!("{lattice}.{component_id}"), headers);

    let Some((instance, name)) = function.rsplit_once('.') else {
        bail!("Invalid function supplied. Must be in the form of `namespace:package/interface.function`")
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(opts.timeout_ms),
        wrpc_client.invoke_dynamic(instance, name, (), &[wrpc_types::Type::String]),
    )
    .await
    .context("Timeout while invoking component, ensure component {component_id} is running in lattice {lattice}")?;

    let out_str = match result {
        Ok((values, _tx)) => {
            if let Some(wrpc_transport::Value::String(result)) = values.get(0) {
                result.to_string()
            } else {
                bail!("Got something other than a string from the component")
            }
        }
        Err(e) if e.to_string().contains("transmission failed") => bail!("No component responsed to your request, ensure component {component_id} is running in lattice {lattice}"),
        Err(e) => bail!("Error invoking component: {e}"),
    };
    Ok(out_str)
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

#[cfg(test)]
mod test {
    use super::CallCommand;
    use anyhow::Result;
    use clap::Parser;

    const RPC_HOST: &str = "127.0.0.1";
    const RPC_PORT: &str = "4222";
    const DEFAULT_LATTICE: &str = "default";

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
            "--context",
            "some-context",
            "--lattice",
            DEFAULT_LATTICE,
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout-ms",
            "0",
            ACTOR_ID,
            "wasmcloud:test/handle.operation",
        ])?;
        match call_all.command {
            CallCommand {
                opts,
                component_id,
                function,
            } => {
                assert_eq!(&opts.rpc_host, RPC_HOST);
                assert_eq!(&opts.rpc_port, RPC_PORT);
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 0);
                assert_eq!(opts.context, Some("some-context".to_string()));
                assert_eq!(component_id, ACTOR_ID);
                assert_eq!(function, "wasmcloud:test/handle.operation");
            }
            #[allow(unreachable_patterns)]
            cmd => panic!("call constructed incorrect command: {cmd:?}"),
        }
        Ok(())
    }
}

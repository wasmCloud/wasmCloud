use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use bytes::BytesMut;
use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::json;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};
use tracing::debug;

use crate::lib::cli::{validate_component_id, CommandOutput};
use crate::lib::config::DEFAULT_LATTICE;
use wasmcloud_core::parse_wit_meta_from_operation;
use wit_bindgen_wrpc::wrpc_transport::InvokeExt as _;

use crate::util::{default_timeout_ms, extract_arg_value, msgpack_to_json_val};

const DEFAULT_HTTP_SCHEME: &str = "http";
const DEFAULT_HTTP_HOST: &str = "localhost";
/// Default port used by wasmCloud HTTP server provider
const DEFAULT_HTTP_PORT: u16 = 8080;

#[derive(Deserialize)]
struct TestResult<'a> {
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
    #[serde(default)]
    pub snap_data: &'a [u8],
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
    for test in results {
        if test.passed {
            let _ = stdout.set_color(&green);
            write!(&mut stdout, "Pass").unwrap();
            let _ = stdout.reset();
            writeln!(&mut stdout, ": {}", test.name).unwrap();
            passed += 1;
        } else {
            let error_msg = serde_json::from_slice::<ErrorReport>(test.snap_data)
                .map(|r| r.error)
                .unwrap_or_default();
            let _ = stdout.set_color(&red);
            write!(&mut stdout, "Fail").unwrap();
            let _ = stdout.reset();
            writeln!(&mut stdout, ": {error_msg}").unwrap();
        }
    }
    let status_color = if passed == total { green } else { red };
    write!(&mut stdout, "Test results: ").unwrap();
    let _ = stdout.set_color(&status_color);
    writeln!(&mut stdout, "{passed}/{total} Passed").unwrap();
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
    #[must_use] pub fn command(self) -> CallCommand {
        self.command
    }
}

pub async fn handle_command(
    CallCommand {
        component_id,
        function,
        opts,
        http_handler_invocation_opts,
        http_response_extract_json,
        ..
    }: CallCommand,
) -> Result<CommandOutput> {
    ensure!(!component_id.is_empty(), "component ID may not be empty");
    debug!(
        ?component_id,
        ?function,
        "calling component function over wRPC"
    );

    let lattice = opts
        .lattice
        .clone()
        .unwrap_or_else(|| DEFAULT_LATTICE.to_string());

    let nc = create_client_from_opts_wrpc(&opts)
        .await
        .context("failed to create async nats client")?;
    let wrpc_client =
        wrpc_transport_nats::Client::new(nc, format!("{}.{component_id}", &lattice), None).await?;

    let (namespace, package, interface, name) = parse_wit_meta_from_operation(&function).context(
        "Invalid function supplied. Must be in the form of `namespace:package/interface.function`",
    )?;
    let instance = format!("{namespace}:{package}/{interface}");
    let name = name.context(
        "Invalid function supplied. Must be in the form of `namespace:package/interface.function`",
    )?;
    debug!(
        ?component_id,
        ?instance,
        ?name,
        ?lattice,
        "invoking component"
    );

    match function.as_str() {
        // If we receive a HTTP call we must translate the provided data into a HTTP request that
        // can be used with wRPC and send that over the wire
        "wrpc:http/incoming-handler.handle" | "wasi:http/incoming-handler.handle" => {
            let request = http_handler_invocation_opts
                .to_request()
                .await
                .context("failed to invoke handler with HTTP request options")?;
            wrpc_invoke_http_handler(
                wrpc_client,
                &lattice,
                &component_id,
                opts.timeout_ms,
                request,
                http_response_extract_json,
            )
            .await
        }
        // Assume the call is a function that takes no input and produces a string
        _ => {
            wrpc_invoke_simple(
                wrpc_client,
                &lattice,
                &component_id,
                &instance,
                &name,
                opts.timeout_ms,
            )
            .await
        }
    }
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

    /// JWT file for RPC authentication. Must be supplied with `rpc_seed`.
    #[clap(
        long = "rpc-jwt",
        env = "WASMCLOUD_RPC_JWT",
        hide_env_values = true,
        requires = "rpc_seed"
    )]
    rpc_jwt: Option<String>,

    /// Seed file or literal for RPC authentication. Must be supplied with `rpc_jwt`.
    #[clap(
        long = "rpc-seed",
        env = "WASMCLOUD_RPC_SEED",
        hide_env_values = true,
        requires = "rpc_jwt"
    )]
    rpc_seed: Option<String>,

    /// Credsfile for RPC authentication. Combines `rpc_seed` and `rpc_jwt`.
    /// See <https://docs.nats.io/using-nats/developer/connecting/creds> for details.
    #[clap(long = "rpc-credsfile", env = "WASH_RPC_CREDS", hide_env_values = true)]
    rpc_credsfile: Option<PathBuf>,

    /// CA file for RPC authentication.
    /// See <https://docs.nats.io/using-nats/developer/security/securing_nats> for details.
    #[clap(
        long = "rpc-ca-file",
        env = "WASH_RPC_TLS_CA_FILE",
        hide_env_values = true
    )]
    rpc_ca_file: Option<PathBuf>,

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
    #[clap(name = "component-id", value_parser = validate_component_id)]
    pub component_id: String,

    /// Fully qualified WIT export to invoke on the component, e.g. `wasi:cli/run.run`
    #[clap(name = "function")]
    pub function: String,

    /// Whether the content of the HTTP response body should be parsed as JSON and returned directly
    #[clap(
        long = "http-response-extract-json",
        default_value_t = false,
        env = "WASH_CALL_HTTP_RESPONSE_EXTRACT_JSON"
    )]
    pub http_response_extract_json: bool,

    /// Customizable options related to the HTTP handler invocation (HTTP path, method, etc)
    #[clap(flatten)]
    pub http_handler_invocation_opts: HttpHandlerInvocationOpts,
}

/// Options that customize the HTTP request that is fed to a HTTP handler when using `wash call`
#[derive(Debug, Clone, Deserialize, Args)]
pub struct HttpHandlerInvocationOpts {
    /// Scheme to use when making the HTTP request
    #[clap(long = "http-scheme", env = "WASH_CALL_INVOKE_HTTP_SCHEME")]
    http_scheme: Option<String>,

    /// Host to use when making the HTTP request
    #[clap(long = "http-host", env = "WASH_CALL_INVOKE_HTTP_HOST")]
    http_host: Option<String>,

    /// Port on which to make the HTTP request
    #[clap(long = "http-port", env = "WASH_CALL_INVOKE_HTTP_PORT")]
    http_port: Option<u16>,

    /// Method to use when making the HTTP request
    #[clap(long = "http-method", env = "WASH_CALL_INVOKE_HTTP_METHOD")]
    http_method: Option<String>,

    /// Stringified body contents to use when making the HTTP request
    #[clap(
        long = "http-body",
        env = "WASH_CALL_INVOKE_HTTP_BODY",
        conflicts_with = "http_body_path"
    )]
    http_body: Option<String>,

    /// Path to a file to use as the body when making a HTTP request
    #[clap(
        long = "http-body-path",
        env = "WASH_CALL_INVOKE_HTTP_BODY_PATH",
        conflicts_with = "http_body"
    )]
    http_body_path: Option<PathBuf>,

    /// Content type header to pass with the request
    #[clap(long = "http-content-type", env = "WASH_CALL_INVOKE_HTTP_CONTENT_TYPE")]
    http_content_type: Option<String>,
}

impl HttpHandlerInvocationOpts {
    pub async fn to_request(self) -> Result<http::Request<String>> {
        let Self {
            http_scheme,
            http_host,
            http_port,
            http_method,
            http_body,
            http_body_path,
            http_content_type,
            ..
        } = self;

        let host = http_host.unwrap_or_else(|| DEFAULT_HTTP_HOST.into());
        let port = http_port.unwrap_or(DEFAULT_HTTP_PORT);
        let scheme = http_scheme.unwrap_or_else(|| DEFAULT_HTTP_SCHEME.into());
        let method =
            http::method::Method::from_str(http_method.unwrap_or_else(|| "GET".into()).as_str())
                .context("failed to read method from input")?;
        debug!(?host, ?port, ?scheme, ?method, content_type = ?http_content_type, "building request from options");

        let http_body = match (http_body, http_body_path) {
            (Some(s), _) => s,
            (_, Some(p)) => tokio::fs::read_to_string(p)
                .await
                .context("failed to read http body file")?,
            (None, None) => String::new(),
        };

        // Build the HTTP request
        let mut req = http::Request::builder()
            .uri(format!("{scheme}://{host}:{port}"))
            .method(method);
        if let Some(content_type) = http_content_type {
            req = req.header("Content-Type", content_type);
        }
        req.body(http_body)
            .context("failed to build HTTP request from handler invocation options")
    }
}

/// Utility type used mostly for printing HTTP responses to the console as JSON
#[derive(Debug, Clone, Serialize)]
struct HttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
}

/// Invoke a wRPC endpoint that takes a HTTP request (usually `wasi:http/incoming-handler.handle`);
async fn wrpc_invoke_http_handler(
    client: wrpc_transport_nats::Client,
    lattice: &str,
    component_id: &str,
    timeout_ms: u64,
    request: http::request::Request<String>,
    extract_json: bool,
) -> Result<CommandOutput> {
    use futures::StreamExt;
    use wrpc_interface_http::InvokeIncomingHandler as _;

    let result = tokio::time::timeout(
       std::time::Duration::from_millis(timeout_ms),
        client
            .invoke_handle_http(Some(gen_wash_call_headers()), request)
  )
   .await
   .with_context(|| format!("component invocation timeout, is component [{component_id}] running in lattice [{lattice}]?"))?
   .context("failed to perform HTTP request")?;

    match result {
        (Ok(mut resp), _errs, io) => {
            if let Some(io) = io {
                io.await.context("failed to complete async I/O")?;
            }

            let status = resp.status().as_u16();
            let headers =
                HashMap::<String, String>::from_iter(resp.headers().into_iter().map(|(k, v)| {
                    (
                        k.as_str().into(),
                        v.to_str().map(std::string::ToString::to_string).unwrap_or_default(),
                    )
                }));

            // Read the incoming body into a string
            let mut body = BytesMut::new();
            while let Some(bytes) = resp.body_mut().body.next().await {
                body.extend(bytes);
            }
            let body = body.freeze();

            // If the option for parsing the response as JSON was provided, parse it directly,
            // and return that as JSON
            let output = if extract_json {
                let body_json = serde_json::from_slice(&body)
                    .context("failed to parse response body bytes into a valid JSON object")?;
                CommandOutput::new(
                    serde_json::to_string_pretty(&body_json)
                        .context("failed to print http response JSON")?,
                    HashMap::from([("response".into(), body_json)]),
                )
            } else {
                let http_resp = HttpResponse {
                    status,
                    headers,
                    body: String::from_utf8(Vec::from(body))
                        .context("failed to parse returned bytes as string")?,
                };
                CommandOutput::new(
                    serde_json::to_string_pretty(&http_resp)
                        .context("failed to print http response JSON")?,
                    HashMap::from([(
                        "response".into(),
                        serde_json::to_value(&http_resp)
                            .context("failed to convert http response to value")?,
                    )]),
                )
            };

            Ok(output)
        }
        // For all other responses, something has gone wrong
        _ => bail!("unexpected response after HTTP wRPC invocation"),
    }
}

/// Invoke a wRPC endpoint that takes nothing and returns a string
async fn wrpc_invoke_simple(
    client: wrpc_transport_nats::Client,
    lattice: &str,
    component_id: &str,
    instance: &str,
    function_name: &str,
    timeout_ms: u64,
) -> Result<CommandOutput> {
    let result = client
           .timeout(Duration::from_millis(timeout_ms))
           .invoke_values_blocking::<_, ((),), (String,)>(
               Some(gen_wash_call_headers()),
               instance,
               function_name,
               ((),),
               &[[]; 0],
           )
   .await
   .with_context(|| format!("timed out invoking component, is component [{component_id}] running in lattice [{lattice}]?"));

    match result {
       Ok((result,)) => {
               Ok(CommandOutput::new(result.clone(), HashMap::from([("result".to_string(), json!(result))])))
       }
       Err(e) if e.to_string().contains("transmission failed") => bail!("No component responded to your request, ensure component {component_id} is running in lattice {lattice}"),
       Err(e) => bail!("Error invoking component: {e}"),
   }
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

/// Create an async nats client which is meant to work with [`async_nats_wrpc`]
///
/// Normally we would use `create_nats_client_from_opts` here, but until the schism between [`async_nats_wrpc`]
/// and [`async_nats`] is resolved, we must replicate that logic here, as upstream `async_nats` does not match.
async fn create_client_from_opts_wrpc(opts: &ConnectionOpts) -> Result<async_nats::Client> {
    let ConnectionOpts {
        rpc_host: host,
        rpc_port: port,
        rpc_jwt: jwt,
        rpc_seed: seed,
        rpc_credsfile: credsfile,
        rpc_ca_file: tls_ca_file,
        ..
    } = opts;

    let nats_url = format!("{host}:{port}");
    use async_nats::ConnectOptions;

    let nc = if let Some(jwt_file) = jwt {
        let jwt_contents = extract_arg_value(jwt_file)
            .with_context(|| format!("Failed to extract jwt contents from {}", &jwt_file))?;
        let kp = std::sync::Arc::new(if let Some(seed) = seed {
            nkeys::KeyPair::from_seed(
                &extract_arg_value(seed)
                    .with_context(|| format!("Failed to extract seed value {}", &seed))?,
            )
            .with_context(|| format!("Failed to create keypair from seed value {}", &seed))?
        } else {
            nkeys::KeyPair::new_user()
        });

        // You must provide the JWT via a closure
        let mut opts = async_nats::ConnectOptions::with_jwt(jwt_contents, move |nonce| {
            let key_pair = kp.clone();
            async move { key_pair.sign(&nonce).map_err(async_nats::AuthError::new) }
        });

        if let Some(ref ca_file) = tls_ca_file {
            opts = opts
                .add_root_certificates(ca_file.clone())
                .require_tls(true);
        }

        opts.connect(&nats_url).await.with_context(|| {
            format!(
                "Failed to connect to NATS server {}:{} while creating client",
                &host, &port
            )
        })?
    } else if let Some(credsfile_path) = credsfile {
        let mut opts = ConnectOptions::with_credentials_file(credsfile_path.clone())
            .await
            .with_context(|| {
                format!(
                    "Failed to authenticate to NATS with credentials file {:?}",
                    &credsfile_path
                )
            })?;

        if let Some(ca_file) = tls_ca_file {
            opts = opts
                .add_root_certificates(ca_file.clone())
                .require_tls(true);
        }

        opts.connect(&nats_url).await.with_context(|| {
            format!(
                "Failed to connect to NATS {} with credentials file {:?}",
                &nats_url, &credsfile_path
            )
        })?
    } else {
        let mut opts = ConnectOptions::new();

        if let Some(ca_file) = tls_ca_file {
            opts = opts
                .add_root_certificates(ca_file.clone())
                .require_tls(true);
        }

        opts.connect(&nats_url)
            .await
            .with_context(|| format!("Failed to connect to NATS {}", &nats_url))?
    };
    Ok(nc)
}

fn gen_wash_call_headers() -> async_nats::HeaderMap {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("source-id", "wash");
    headers
}

#[cfg(test)]
mod test {
    use super::CallCommand;
    use anyhow::Result;
    use clap::Parser;

    const RPC_HOST: &str = "127.0.0.1";
    const RPC_PORT: &str = "4222";
    const DEFAULT_LATTICE: &str = "default";

    const COMPONENT_ID: &str = "MDPDJEYIAK6MACO67PRFGOSSLODBISK4SCEYDY3HEOY4P5CVJN6UCWUK";

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
            COMPONENT_ID,
            "wasmcloud:test/handle.operation",
        ])?;
        match call_all.command {
            CallCommand {
                opts,
                component_id,
                function,
                ..
            } => {
                assert_eq!(&opts.rpc_host, RPC_HOST);
                assert_eq!(&opts.rpc_port, RPC_PORT);
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 0);
                assert_eq!(opts.context, Some("some-context".to_string()));
                assert_eq!(component_id, COMPONENT_ID);
                assert_eq!(function, "wasmcloud:test/handle.operation");
            }
            #[allow(unreachable_patterns)]
            cmd => panic!("call constructed incorrect command: {cmd:?}"),
        }
        Ok(())
    }
}

//! A common set of helpers for writing your own CLI with wash-lib. This functionality is entirely
//! optional. Also included in this module are several submodules with key bits of reusable
//! functionality. This functionality is only suitable for CLIs and not for normal code, but it is
//! likely to be helpful to anyone who doesn't want to rewrite things like the signing and claims
//! subcommand
//!
//! PLEASE NOTE: This module is the most likely to change of any of the modules. We may decide to
//! pull some of this code out and back in to the wash CLI only. We will try to communicate these
//! changes as clearly as possible

use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use clap::Args;
use nkeys::{KeyPair, KeyPairType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::info;

use crate::{
    config::{
        cfg_dir, WashConnectionOptions, DEFAULT_LATTICE_PREFIX, DEFAULT_NATS_HOST,
        DEFAULT_NATS_PORT, DEFAULT_NATS_TIMEOUT_MS,
    },
    context::{default_timeout_ms, fs::ContextDir, ContextManager},
    keys::{
        fs::{read_key, KeyDir},
        KeyManager,
    },
};

pub mod capture;
pub mod claims;
pub mod dev;
pub mod get;
pub mod inspect;
pub mod link;
pub mod output;
pub mod par;
pub mod registry;
pub mod scale;
pub mod spy;
pub mod start;
pub mod stop;
pub mod update;

/// Used for displaying human-readable output vs JSON format
#[derive(Debug, Copy, Clone, Eq, Serialize, Deserialize, PartialEq)]
pub enum OutputKind {
    Text,
    Json,
}

impl FromStr for OutputKind {
    type Err = OutputParseErr;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "json" => Ok(OutputKind::Json),
            "text" => Ok(OutputKind::Text),
            _ => Err(OutputParseErr),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputParseErr;

impl Error for OutputParseErr {}

impl std::fmt::Display for OutputParseErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "error parsing output type, see help for the list of accepted outputs"
        )
    }
}

pub struct CommandOutput {
    pub map: std::collections::HashMap<String, serde_json::Value>,
    pub text: String,
}

impl CommandOutput {
    pub fn new<S: Into<String>>(
        text: S,
        map: std::collections::HashMap<String, serde_json::Value>,
    ) -> Self {
        CommandOutput {
            map,
            text: text.into(),
        }
    }

    /// shorthand to create a new CommandOutput with a single key-value pair for JSON, and simply the text for text output.
    pub fn from_key_and_text<K: Into<String>, S: Into<String>>(key: K, text: S) -> Self {
        let text_string: String = text.into();
        let mut map = std::collections::HashMap::new();
        map.insert(key.into(), serde_json::Value::String(text_string.clone()));
        CommandOutput {
            map,
            text: text_string,
        }
    }
}

impl From<String> for CommandOutput {
    /// Create a basic CommandOutput from a String. Puts the string a a "result" key in the JSON output.
    fn from(text: String) -> Self {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "result".to_string(),
            serde_json::Value::String(text.clone()),
        );
        CommandOutput { map, text }
    }
}

impl From<&str> for CommandOutput {
    /// Create a basic CommandOutput from a &str. Puts the string a a "result" key in the JSON output.
    fn from(text: &str) -> Self {
        CommandOutput::from(text.to_string())
    }
}

impl Default for CommandOutput {
    fn default() -> Self {
        CommandOutput {
            map: std::collections::HashMap::new(),
            text: "".to_string(),
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct CliConnectionOpts {
    /// CTL Host for connection, defaults to 127.0.0.1 for local nats
    #[clap(short = 'r', long = "ctl-host", env = "WASMCLOUD_CTL_HOST")]
    pub ctl_host: Option<String>,

    /// CTL Port for connections, defaults to 4222 for local nats
    #[clap(short = 'p', long = "ctl-port", env = "WASMCLOUD_CTL_PORT")]
    pub ctl_port: Option<String>,

    /// JWT file for CTL authentication. Must be supplied with ctl_seed.
    #[clap(long = "ctl-jwt", env = "WASMCLOUD_CTL_JWT", hide_env_values = true)]
    pub ctl_jwt: Option<String>,

    /// Seed file or literal for CTL authentication. Must be supplied with ctl_jwt.
    #[clap(long = "ctl-seed", env = "WASMCLOUD_CTL_SEED", hide_env_values = true)]
    pub ctl_seed: Option<String>,

    /// Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt.
    /// See https://docs.nats.io/using-nats/developer/connecting/creds for details.
    #[clap(long = "ctl-credsfile", env = "WASH_CTL_CREDS", hide_env_values = true)]
    pub ctl_credsfile: Option<PathBuf>,

    /// JS domain for wasmcloud control interface. Defaults to None
    #[clap(
        long = "js-domain",
        alias = "domain",
        env = "WASMCLOUD_JS_DOMAIN",
        hide_env_values = true
    )]
    pub js_domain: Option<String>,

    /// Lattice prefix for wasmcloud control interface, defaults to "default"
    #[clap(short = 'x', long = "lattice-prefix", env = "WASMCLOUD_LATTICE_PREFIX")]
    pub lattice_prefix: Option<String>,

    /// Timeout length to await a control interface response, defaults to 2000 milliseconds
    #[clap(
        short = 't',
        long = "timeout-ms",
        default_value_t = default_timeout_ms(),
        env = "WASMCLOUD_CTL_TIMEOUT_MS"
    )]
    pub timeout_ms: u64,

    /// Name of a context to use for CTL connection and authentication
    #[clap(long = "context")]
    pub context: Option<String>,
}

impl Default for CliConnectionOpts {
    fn default() -> Self {
        CliConnectionOpts {
            ctl_host: Some(DEFAULT_NATS_HOST.to_string()),
            ctl_port: Some(DEFAULT_NATS_PORT.to_string()),
            ctl_jwt: None,
            ctl_seed: None,
            ctl_credsfile: None,
            js_domain: None,
            lattice_prefix: Some(DEFAULT_LATTICE_PREFIX.to_string()),
            timeout_ms: DEFAULT_NATS_TIMEOUT_MS,
            context: None,
        }
    }
}

impl TryFrom<CliConnectionOpts> for WashConnectionOptions {
    type Error = anyhow::Error;

    fn try_from(
        CliConnectionOpts {
            ctl_host,
            ctl_port,
            ctl_jwt,
            ctl_seed,
            ctl_credsfile,
            js_domain,
            lattice_prefix,
            timeout_ms,
            context,
        }: CliConnectionOpts,
    ) -> Result<WashConnectionOptions> {
        // Attempt to load a context, falling back on the default if not supplied
        let ctx_dir = ContextDir::new()?;
        let ctx = if let Some(context_name) = context {
            ctx_dir
                .load_context(&context_name)
                .with_context(|| format!("failed to load context `{context_name}`"))?
        } else {
            ctx_dir
                .load_default_context()
                .context("failed to load default context")?
        };

        Ok(WashConnectionOptions {
            ctl_host,
            ctl_port,
            ctl_jwt,
            ctl_seed,
            ctl_credsfile,
            js_domain,
            lattice_prefix,
            timeout_ms,
            ctx,
        })
    }
}

/// Helper function to locate and extract keypair from user input and generate a key for the user if
/// needed
///
/// Returns the loaded or generated keypair
pub fn extract_keypair(
    input: Option<String>,
    module_path: Option<String>,
    directory: Option<PathBuf>,
    keygen_type: KeyPairType,
    disable_keygen: bool,
    output_kind: OutputKind,
) -> Result<KeyPair> {
    if let Some(input_str) = input {
        match read_key(&input_str) {
            // User provided file path to seed as argument
            Ok(k) => Ok(k),
            // User provided seed as an argument
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => {
                KeyPair::from_seed(&input_str).map_err(anyhow::Error::from)
            }
            // There was an actual error reading the file
            Err(e) => Err(e.into()),
        }
    } else if let Some(module) = module_path {
        // No seed value provided, attempting to source from provided or default directory
        let key_dir = KeyDir::new(determine_directory(directory)?)?;

        // Account key should be re-used, and will attempt to generate based on the terminal USER
        let module_name = match keygen_type {
            KeyPairType::Account => std::env::var("USER").unwrap_or_else(|_| "user".to_string()),
            _ => PathBuf::from(module)
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string(),
        };
        let keyname = format!(
            "{}_{}",
            module_name,
            keypair_type_to_string(keygen_type.clone())
        );
        let path = key_dir.join(format!("{}{}", keyname, ".nk"));
        match key_dir.get(&keyname)? {
            // Default key found
            Some(k) => Ok(k),
            // No default key, generating for user
            None if !disable_keygen => {
                match output_kind {
                    OutputKind::Text => info!(
                        "No keypair found in \"{}\".
                    We will generate one for you and place it there.
                    If you'd like to use an existing key, you can supply it on the CLI as a flag.\n",
                        path.display()
                    ),
                    OutputKind::Json => {
                        info!(
                            "{}",
                            json!({"status": "No existing keypair found, automatically generated and stored a new one", "path": path, "keygen": "true"})
                        )
                    }
                }

                let kp = KeyPair::new(keygen_type);
                key_dir.save(&keyname, &kp)?;
                Ok(kp)
            }
            None => {
                anyhow::bail!(
                    "No keypair found in {}, please ensure key exists or supply one as a flag",
                    path.display()
                );
            }
        }
    } else {
        anyhow::bail!("Keypair path or string not supplied. Ensure provided keypair is valid");
    }
}

/// Transforms a list of labels in the form of (label=value) to a hashmap
pub fn labels_vec_to_hashmap(constraints: Vec<String>) -> Result<HashMap<String, String>> {
    let mut hm: HashMap<String, String> = HashMap::new();
    for constraint in constraints {
        match constraint.split_once('=') {
            Some((key, value)) => {
                hm.insert(key.to_string(), value.to_string());
            }
            None => {
                anyhow::bail!("Constraints were not properly formatted. Ensure they are formatted as label=value")
            }
        };
    }
    Ok(hm)
}

fn determine_directory(directory: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(d) = directory {
        Ok(d)
    } else {
        let d = cfg_dir()?.join("keys");
        Ok(d)
    }
}

fn keypair_type_to_string(keypair_type: KeyPairType) -> String {
    use KeyPairType::*;
    match keypair_type {
        Account => "account".to_string(),
        Cluster => "cluster".to_string(),
        Service => "service".to_string(),
        Module => "module".to_string(),
        Server => "server".to_string(),
        Operator => "operator".to_string(),
        User => "user".to_string(),
    }
}

pub(crate) fn configure_table_style(table: &mut term_table::Table<'_>) {
    table.style = empty_table_style();
    table.separate_rows = false;
}

fn empty_table_style() -> term_table::TableStyle {
    term_table::TableStyle {
        top_left_corner: ' ',
        top_right_corner: ' ',
        bottom_left_corner: ' ',
        bottom_right_corner: ' ',
        outer_left_vertical: ' ',
        outer_right_vertical: ' ',
        outer_bottom_horizontal: ' ',
        outer_top_horizontal: ' ',
        intersection: ' ',
        vertical: ' ',
        horizontal: ' ',
    }
}

pub const OCI_CACHE_DIR: &str = "wasmcloud_ocicache";

/// Given an oci reference, returns a path to a cache file for an artifact
pub fn cached_oci_file(img: &str) -> PathBuf {
    let path = std::env::temp_dir();
    let path = path.join(OCI_CACHE_DIR);
    let _ = ::std::fs::create_dir_all(&path);
    // should produce a file like wasmcloud_azurecr_io_kvcounter_v1.bin
    let mut path = path.join(img_name_to_file_name(img));
    path.set_extension("bin");

    path
}

fn img_name_to_file_name(img: &str) -> String {
    img.replace([':', '/', '.'], "_")
}

#[cfg(test)]
mod test {
    use anyhow::Result;
    use std::env;

    use crate::{
        config::{WashConnectionOptions, DEFAULT_CTX_DIR_NAME, DEFAULT_LATTICE_PREFIX, WASH_DIR},
        context::{fs::ContextDir, ContextManager, WashContext},
    };

    use super::CliConnectionOpts;

    #[tokio::test]
    #[cfg(not(target_family = "windows"))]
    async fn test_lattice_prefix() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        env::set_current_dir(&tempdir)?;
        env::set_var("HOME", tempdir.path());

        // when opts.lattice_prefix.is_none() && opts.context.is_none() && user didn't set a default context, use the lattice_prefix from the preset default context...
        let cli_opts = CliConnectionOpts::default();
        let wash_opts = WashConnectionOptions::try_from(cli_opts)?;
        assert_eq!(
            wash_opts.get_lattice_prefix(),
            DEFAULT_LATTICE_PREFIX.to_string()
        );

        // when opts.lattice_prefix.is_some() && opts.context.is_none(), use the specified lattice_prefix...
        let cli_opts = CliConnectionOpts {
            lattice_prefix: Some("hal9000".to_string()),
            ..Default::default()
        };
        let wash_opts = WashConnectionOptions::try_from(cli_opts)?;
        assert_eq!(wash_opts.get_lattice_prefix(), "hal9000".to_string());

        let context_dir = ContextDir::from_dir(Some(
            tempdir
                .path()
                .join(format!("{WASH_DIR}/{DEFAULT_CTX_DIR_NAME}")),
        ))?;

        // when opts.lattice_prefix.is_none() && opts.context.is_some(), use the lattice_prefix from the specified context...
        context_dir.save_context(&WashContext {
            name: "foo".to_string(),
            lattice_prefix: "iambatman".to_string(),
            ..Default::default()
        })?;
        let cli_opts = CliConnectionOpts {
            context: Some("foo".to_string()),
            lattice_prefix: None,
            ..Default::default()
        };
        let wash_opts = WashConnectionOptions::try_from(cli_opts)?;
        assert_eq!(wash_opts.get_lattice_prefix(), "iambatman".to_string());

        // when opts.lattice_prefix.is_none() && opts.context.is_none(), use the lattice_prefix from the specified default context...
        context_dir.save_context(&WashContext {
            name: "bar".to_string(),
            lattice_prefix: "iamironman".to_string(),
            ..Default::default()
        })?;
        context_dir.set_default_context("bar")?;
        let cli_opts = CliConnectionOpts {
            lattice_prefix: None,
            ..Default::default()
        };
        let wash_opts = WashConnectionOptions::try_from(cli_opts)?;
        assert_eq!(wash_opts.get_lattice_prefix(), "iamironman".to_string());

        Ok(())
    }
}

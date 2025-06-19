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

use anyhow::{bail, Context, Result};
use clap::Args;
use nkeys::{KeyPair, KeyPairType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::info;
use wasm_pkg_client::{
    caching::{CachingClient, FileCache},
    RegistryMapping,
};

use crate::lib::{
    config::{
        WashConnectionOptions, DEFAULT_LATTICE, DEFAULT_NATS_HOST, DEFAULT_NATS_PORT,
        DEFAULT_NATS_TIMEOUT_MS, WASH_DIRECTORIES,
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
pub mod label;
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
            "json" => Ok(Self::Json),
            "text" => Ok(Self::Text),
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

#[derive(Default)]
pub struct CommandOutput {
    pub map: std::collections::HashMap<String, serde_json::Value>,
    pub text: String,
}

impl CommandOutput {
    pub fn new<S: Into<String>>(
        text: S,
        map: std::collections::HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            map,
            text: text.into(),
        }
    }

    /// shorthand to create a new `CommandOutput` with a single key-value pair for JSON, and simply the text for text output.
    pub fn from_key_and_text<K: Into<String>, S: Into<String>>(key: K, text: S) -> Self {
        let text_string: String = text.into();
        let mut map = std::collections::HashMap::new();
        map.insert(key.into(), serde_json::Value::String(text_string.clone()));
        Self {
            map,
            text: text_string,
        }
    }
}

impl From<String> for CommandOutput {
    /// Create a basic `CommandOutput` from a String. Puts the string a a "result" key in the JSON output.
    fn from(text: String) -> Self {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "result".to_string(),
            serde_json::Value::String(text.clone()),
        );
        Self { map, text }
    }
}

impl From<&str> for CommandOutput {
    /// Create a basic `CommandOutput` from a &str. Puts the string a a "result" key in the JSON output.
    fn from(text: &str) -> Self {
        Self::from(text.to_string())
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

    /// JWT file for CTL authentication. Must be supplied with `ctl_seed`.
    #[clap(long = "ctl-jwt", env = "WASMCLOUD_CTL_JWT", hide_env_values = true)]
    pub ctl_jwt: Option<String>,

    /// Seed file or literal for CTL authentication. Must be supplied with `ctl_jwt`.
    #[clap(long = "ctl-seed", env = "WASMCLOUD_CTL_SEED", hide_env_values = true)]
    pub ctl_seed: Option<String>,

    /// Credsfile for CTL authentication. Combines `ctl_seed` and `ctl_jwt`.
    /// See <https://docs.nats.io/using-nats/developer/connecting/creds> for details.
    #[clap(long = "ctl-credsfile", env = "WASH_CTL_CREDS", hide_env_values = true)]
    pub ctl_credsfile: Option<PathBuf>,

    /// TLS CA file for CTL authentication. See <https://docs.nats.io/using-nats/developer/connecting/tls> for details.
    #[clap(
        long = "ctl-tls-ca-file",
        env = "WASH_CTL_TLS_CA_FILE",
        hide_env_values = true
    )]
    pub ctl_tls_ca_file: Option<PathBuf>,

    /// Perform TLS handshake before expecting the server greeting.
    #[clap(
        long = "ctl-tls-first",
        env = "WASH_CTL_TLS_FIRST",
        hide_env_values = true
    )]
    pub ctl_tls_first: Option<bool>,

    /// JS domain for wasmcloud control interface. Defaults to None
    #[clap(
        long = "js-domain",
        alias = "domain",
        env = "WASMCLOUD_JS_DOMAIN",
        hide_env_values = true
    )]
    pub js_domain: Option<String>,

    /// Lattice name for wasmcloud control interface, defaults to "default"
    #[clap(short = 'x', long = "lattice", env = "WASMCLOUD_LATTICE")]
    pub lattice: Option<String>,

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
        Self {
            ctl_host: Some(DEFAULT_NATS_HOST.to_string()),
            ctl_port: Some(DEFAULT_NATS_PORT.to_string()),
            ctl_jwt: None,
            ctl_seed: None,
            ctl_credsfile: None,
            ctl_tls_ca_file: None,
            ctl_tls_first: None,
            js_domain: None,
            lattice: Some(DEFAULT_LATTICE.to_string()),
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
            ctl_tls_ca_file,
            ctl_tls_first,
            js_domain,
            lattice,
            timeout_ms,
            context,
        }: CliConnectionOpts,
    ) -> Result<Self> {
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

        Ok(Self {
            ctl_host,
            ctl_port,
            ctl_jwt,
            ctl_seed,
            ctl_credsfile,
            ctl_tls_ca_file,
            ctl_tls_first,
            js_domain,
            lattice,
            timeout_ms,
            ctx,
        })
    }
}

// NOTE(thomastaylor312): This is copied from the `wkg` CLI until
// https://github.com/bytecodealliance/wasm-pkg-tools/issues/98 is worked on

/// Common arguments for wasm package tooling.
#[derive(Args, Debug, Clone, Default)]
pub struct CommonPackageArgs {
    /// The path to the configuration file.
    #[arg(long = "pkg-config", env = "WASH_PACKAGE_CONFIG_FILE")]
    config: Option<PathBuf>,
    /// The path to the cache directory. Defaults to the system cache directory.
    #[arg(long = "pkg-cache", env = "WASH_PACKAGE_CACHE_DIR")]
    cache: Option<PathBuf>,
}

impl CommonPackageArgs {
    /// Helper to load the config from the given path or other default paths
    pub async fn load_config(&self) -> anyhow::Result<wasm_pkg_client::Config> {
        // Get the default config so we have the default fallbacks
        let mut conf = wasm_pkg_client::Config::default();

        // We attempt to load config in the following order of preference:
        // 1. Path provided by the user via flag
        // 2. Path provided by the user via `WASH` prefixed environment variable
        // 3. Path provided by the users via `WKG` prefixed environment variable
        // 4. Default path to config file in wash dir
        // 5. Default path to config file from wkg
        match (self.config.as_ref(), std::env::var_os("WKG_CONFIG_FILE")) {
            // We have a config file provided by the user flag or WASH env var
            (Some(path), _) => {
                let loaded = wasm_pkg_client::Config::from_file(&path)
                    .await
                    .context(format!("error loading config file {path:?}"))?;
                // Merge the two configs
                conf.merge(loaded);
            }
            // We have a config file provided by the user via `WKG` env var
            (None, Some(path)) => {
                let loaded = wasm_pkg_client::Config::from_file(&path)
                    .await
                    .context(format!("error loading config file from {path:?}"))?;
                // Merge the two configs
                conf.merge(loaded);
            }
            // Otherwise we got nothing and attempt to load the default config locations
            (None, None) => {
                let path = WASH_DIRECTORIES.create_in_config_dir("package_config.toml")?;
                // Check if the config file exists before loading so we can error properly
                if tokio::fs::metadata(&path).await.is_ok() {
                    let loaded = wasm_pkg_client::Config::from_file(&path)
                        .await
                        .context(format!("error loading config file {path:?}"))?;
                    // Merge the two configs
                    conf.merge(loaded);
                } else if let Ok(Some(c)) = wasm_pkg_client::Config::read_global_config().await {
                    // This means the global config exists, so we merge that in instead
                    conf.merge(c);
                }
            }
        };
        let wasmcloud_label = "wasmcloud".parse().unwrap();
        // If they don't have a config set for the wasmcloud namespace, set it to the default defined here
        if conf.namespace_registry(&wasmcloud_label).is_none() {
            conf.set_namespace_registry(
                wasmcloud_label,
                RegistryMapping::Registry("wasmcloud.com".parse().unwrap()),
            );
        }
        // Same for wrpc
        let wrpc_label = "wrpc".parse().unwrap();
        if conf.namespace_registry(&wrpc_label).is_none() {
            conf.set_namespace_registry(
                wrpc_label,
                RegistryMapping::Registry("bytecodealliance.org".parse().unwrap()),
            );
        }
        Ok(conf)
    }

    /// Helper for loading the [`FileCache`]
    pub async fn load_cache(&self) -> anyhow::Result<FileCache> {
        // We attempt to setup a cache dir in the following order of preference:
        // 1. Path provided by the user via flag
        // 2. Path provided by the user via `WASH` prefixed environment variable
        // 3. Path provided by the users via `WKG` prefixed environment variable
        // 4. Default path to cache in wash dir
        let dir = match (self.cache.as_ref(), std::env::var_os("WKG_CACHE_DIR")) {
            // We have a cache dir provided by the user flag or WASH env var
            (Some(path), _) => path.to_owned(),
            // We have a cache dir provided by the user via `WKG` env var
            (None, Some(path)) => PathBuf::from(path),
            // Otherwise we got nothing and attempt to load the default cache dir
            (None, None) => WASH_DIRECTORIES.package_cache_dir(),
        };
        FileCache::new(dir).await
    }

    /// Helper for loading a caching client.
    ///
    /// This should be the most commonly used method for
    /// loading a client, but if you need to modify the config or use your own cache, you can use
    /// the [`CommonPackageArgs::load_config`] and [`CommonPackageArgs::load_cache`] methods.
    pub async fn get_client(&self) -> anyhow::Result<CachingClient<FileCache>> {
        self.get_client_with_config(self.load_config().await?).await
    }

    /// Helper for loading a caching client, given a configuration.
    pub async fn get_client_with_config(
        &self,
        config: wasm_pkg_client::Config,
    ) -> anyhow::Result<CachingClient<FileCache>> {
        let cache = self.load_cache().await?;
        let client = wasm_pkg_client::Client::new(config);
        Ok(CachingClient::new(Some(client), cache))
    }

    #[must_use]
    pub const fn config_path(&self) -> Option<&PathBuf> {
        self.config.as_ref()
    }
}

/// Helper function to locate and extract keypair from user input and generate a key for the user if
/// needed
///
/// Returns the loaded or generated keypair
pub fn extract_keypair(
    input: Option<&str>,
    module_path: Option<&str>,
    directory: Option<PathBuf>,
    keygen_type: KeyPairType,
    disable_keygen: bool,
    output_kind: OutputKind,
) -> Result<KeyPair> {
    if let Some(input_str) = input {
        match read_key(input_str) {
            // User provided file path to seed as argument
            Ok(k) => Ok(k),
            // User provided seed as an argument
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => {
                KeyPair::from_seed(input_str).map_err(anyhow::Error::from)
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
        let keyname = format!("{module_name}_{}", keypair_type_to_str(&keygen_type));
        let path = key_dir.join(format!("{keyname}.nk"));
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
                        );
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

/// Transforms a list of key in the form of (key=value) to a hashmap
pub fn input_vec_to_hashmap(values: Vec<String>) -> Result<HashMap<String, String>> {
    let mut hm: HashMap<String, String> = HashMap::new();
    for constraint in values {
        match constraint.split_once('=') {
            Some((key, value)) => {
                hm.insert(key.to_string(), value.to_string());
            }
            None => {
                anyhow::bail!("Input values were not properly formatted. Ensure they are formatted as key=value")
            }
        };
    }
    Ok(hm)
}

/// This function is a simple helper to ensure that a component ID is a valid
/// string containing only alphanumeric characters, underscores or dashes
pub fn validate_component_id(id: &str) -> anyhow::Result<String> {
    if id.chars().all(valid_component_char) {
        Ok(id.to_string())
    } else {
        bail!("Component ID must contain only alphanumeric characters and underscores")
    }
}

/// This function is a simple helper to ensure that a component ID is a valid
/// by transforming any non-alphanumeric characters to underscores
#[must_use]
pub fn sanitize_component_id(id: &str) -> String {
    id.chars()
        .map(|c| if valid_component_char(c) { c } else { '_' })
        .collect()
}

fn valid_component_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

fn determine_directory(directory: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(d) = directory {
        Ok(d)
    } else {
        let d = WASH_DIRECTORIES.keys_dir();
        Ok(d)
    }
}

const fn keypair_type_to_str(keypair_type: &KeyPairType) -> &'static str {
    use KeyPairType::{Account, Cluster, Curve, Module, Operator, Server, Service, User};
    match keypair_type {
        Account => "account",
        Cluster => "cluster",
        Service => "service",
        Module => "module",
        Server => "server",
        Operator => "operator",
        User => "user",
        Curve => "curve",
    }
}

pub(crate) fn configure_table_style(table: &mut term_table::Table<'_>) {
    table.style = empty_table_style();
    table.separate_rows = false;
}

const fn empty_table_style() -> term_table::TableStyle {
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
#[must_use]
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
#[cfg(not(target_family = "windows"))]
mod test {
    use std::{
        env,
        ffi::{OsStr, OsString},
        path::{Path, PathBuf},
    };

    use anyhow::Result;
    use serial_test::serial;

    use crate::lib::{
        config::{WashConnectionOptions, DEFAULT_LATTICE, WASH_DIRECTORIES},
        context::{fs::ContextDir, ContextManager, WashContext},
    };

    use super::{CliConnectionOpts, CommonPackageArgs};

    struct CurDir {
        cwd: PathBuf,
    }

    impl CurDir {
        fn cwd(path: impl AsRef<Path>) -> Result<Self> {
            let cwd = env::current_dir()?;
            env::set_current_dir(path)?;
            Ok(Self { cwd })
        }
    }

    impl Drop for CurDir {
        fn drop(&mut self) {
            env::set_current_dir(&self.cwd).unwrap();
        }
    }

    struct EnvVar {
        key: OsString,
        value: Option<OsString>,
    }

    impl EnvVar {
        fn set(key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
            let old_val = env::var_os(&key);
            env::set_var(&key, value);
            Self {
                key: key.as_ref().into(),
                value: old_val,
            }
        }
    }

    impl Drop for EnvVar {
        fn drop(&mut self) {
            if let Some(value) = &self.value {
                env::set_var(&self.key, value);
            } else {
                env::remove_var(&self.key);
            }
        }
    }

    // These tests MUST be run serially because they modify the environment and current working dir

    #[tokio::test]
    #[serial]
    async fn test_lattice_name() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let _dir = CurDir::cwd(&tempdir)?;
        let _home_var = EnvVar::set("HOME", tempdir.path());
        let _xdg_config_home = EnvVar::set("XDG_CONFIG_HOME", tempdir.path().join(".config"));
        let _xdg_data_home = EnvVar::set("XDG_DATA_HOME", tempdir.path().join(".local/share"));

        // when opts.lattice.is_none() && opts.context.is_none() && user didn't set a default context, use the lattice from the preset default context...
        let cli_opts = CliConnectionOpts::default();
        let wash_opts = WashConnectionOptions::try_from(cli_opts)?;
        assert_eq!(wash_opts.get_lattice(), DEFAULT_LATTICE.to_string());

        // when opts.lattice.is_some() && opts.context.is_none(), use the specified lattice...
        let cli_opts = CliConnectionOpts {
            lattice: Some("hal9000".to_string()),
            ..Default::default()
        };
        let wash_opts = WashConnectionOptions::try_from(cli_opts)?;
        assert_eq!(wash_opts.get_lattice(), "hal9000".to_string());

        let context_dir = ContextDir::from_dir(Some(tempdir.path().join(".config/wash/contexts")))?;

        // when opts.lattice.is_none() && opts.context.is_some(), use the lattice from the specified context...
        context_dir.save_context(&WashContext {
            name: "foo".to_string(),
            lattice: "iambatman".to_string(),
            ..Default::default()
        })?;
        let cli_opts = CliConnectionOpts {
            context: Some("foo".to_string()),
            lattice: None,
            ..Default::default()
        };
        let wash_opts = WashConnectionOptions::try_from(cli_opts)?;
        assert_eq!(wash_opts.get_lattice(), "iambatman".to_string());

        // when opts.lattice.is_none() && opts.context.is_none(), use the lattice from the specified default context...
        context_dir.save_context(&WashContext {
            name: "bar".to_string(),
            lattice: "iamironman".to_string(),
            ..Default::default()
        })?;
        context_dir.set_default_context("bar")?;
        let cli_opts = CliConnectionOpts {
            lattice: None,
            ..Default::default()
        };
        let wash_opts = WashConnectionOptions::try_from(cli_opts)?;
        assert_eq!(wash_opts.get_lattice(), "iamironman".to_string());

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_config_loading() {
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");

        let wash_config_path = tempdir.path().join(".config/wash");
        tokio::fs::create_dir_all(&wash_config_path)
            .await
            .expect("failed to create wash config dir");
        let wash_data_path = tempdir.path().join(".local/share/wash");
        tokio::fs::create_dir_all(&wash_data_path)
            .await
            .expect("failed to create wash data dir");
        let wkg_conf = tempdir.path().join("wkg");
        tokio::fs::create_dir_all(&wkg_conf)
            .await
            .expect("failed to create wkg dir");

        // First, test a default config with no env vars
        let mut common = CommonPackageArgs::default();

        let config = common
            .load_config()
            .await
            .expect("Should be able to load a default config");
        assert_default_registries(&config);
        assert!(
            config.namespace_registry(&"foo".parse().unwrap()).is_none(),
            "Should not have a namespace set for foo"
        );

        let _home_env = EnvVar::set("HOME", tempdir.path());
        let _xdg_config_home = EnvVar::set("XDG_CONFIG_HOME", tempdir.path().join(".config"));
        let _xdg_data_home = EnvVar::set("XDG_DATA_HOME", tempdir.path().join(".local/share"));

        let expected_reg =
            wasm_pkg_client::RegistryMapping::Registry("hellothere.com".parse().unwrap());
        // Create some configs for testing
        let mut config_for_wash = wasm_pkg_client::Config::default();
        config_for_wash.set_namespace_registry("foo".parse().unwrap(), expected_reg.clone());
        config_for_wash
            .to_file(wash_config_path.join("package_config.toml"))
            .await
            .expect("failed to write config");

        let mut config_for_wkg = wasm_pkg_client::Config::default();
        let wkg_config_path = wkg_conf.join("config.toml");
        config_for_wkg.set_namespace_registry("bar".parse().unwrap(), expected_reg.clone());
        config_for_wkg
            .to_file(&wkg_config_path)
            .await
            .expect("failed to write config");

        let mut config_for_home = wasm_pkg_client::Config::default();
        let home_config_path = tempdir.path().join("config.toml");
        config_for_home.set_namespace_registry("baz".parse().unwrap(), expected_reg.clone());
        // Force a custom wasmcloud one
        config_for_home.set_namespace_registry(
            "wasmcloud".parse().unwrap(),
            wasm_pkg_client::RegistryMapping::Registry("adifferentone.com".parse().unwrap()),
        );
        config_for_home
            .to_file(&home_config_path)
            .await
            .expect("failed to write config");

        // Now try loading the config again, this time with a config file in the default location
        let config = common
            .load_config()
            .await
            .expect("Should be able to load a default config");
        assert_default_registries(&config);
        let foo_registry = config
            .namespace_registry(&"foo".parse().unwrap())
            .expect("Should have a namespace set for foo");

        assert!(
            assert_registry_mapping(foo_registry, "hellothere.com"),
            "Should have the proper registry for foo",
        );

        // Set the WKG env var and make sure it overrides the config file
        let _wkg_env = EnvVar::set("WKG_CONFIG_FILE", &wkg_config_path);
        let config = common
            .load_config()
            .await
            .expect("Should be able to load config");
        assert_default_registries(&config);
        let bar_registry = config
            .namespace_registry(&"bar".parse().unwrap())
            .expect("Should have a namespace set for bar");
        assert!(
            assert_registry_mapping(bar_registry, "hellothere.com"),
            "Should have the proper registry for bar",
        );

        // Now set an actual path in the common args and make sure that gets loaded, even with other env vars set
        common.config = Some(home_config_path.clone());

        let config = common
            .load_config()
            .await
            .expect("Should be able to load config");
        let baz_registry = config
            .namespace_registry(&"baz".parse().unwrap())
            .expect("Should have a namespace set for baz");
        assert!(
            assert_registry_mapping(baz_registry, "hellothere.com"),
            "Should have the proper registry for baz",
        );
        // Double check that our override of the wasmcloud namespace works
        let wasmcloud_registry = config
            .namespace_registry(&"wasmcloud".parse().unwrap())
            .expect("Should have a namespace set for wasmcloud");
        assert!(
            assert_registry_mapping(wasmcloud_registry, "adifferentone.com"),
            "Should have the proper registry for wasmcloud",
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_config_directory_overrides() {
        use etcetera::AppStrategy;
        let tempdir = tempfile::tempdir().expect("failed to create tempdir");
        let _xdg_config_home = EnvVar::set("XDG_CONFIG_HOME", tempdir.path().join(".config"));
        assert_eq!(
            WASH_DIRECTORIES.config_dir(),
            tempdir.path().join(".config/wash"),
            "Should retrieve XDG conform wash config directory"
        );
        assert_eq!(
            WASH_DIRECTORIES.context_dir(),
            tempdir.path().join(".config/wash/contexts"),
            "Should retrieve XDG conform wash context directory"
        );
        let _wash_config_dir = EnvVar::set("WASH_CONFIG_DIR", tempdir.path().join(".wash"));
        assert_eq!(
            WASH_DIRECTORIES.config_dir(),
            tempdir.path().join(".wash"),
            "Should retrieve custom wash config directory"
        );
        assert_eq!(
            WASH_DIRECTORIES.context_dir(),
            tempdir.path().join(".wash/contexts"),
            "Should retrieve contexts directory in custom wash config directory"
        );
        let _wash_context_dir = EnvVar::set("WASH_CONTEXT_DIR", tempdir.path().join("contexts"));
        assert_eq!(
            WASH_DIRECTORIES.context_dir(),
            tempdir.path().join("contexts"),
            "Should retrieve custom contexts config directory"
        );
    }

    fn assert_registry_mapping(
        registry_mapping: &wasm_pkg_client::RegistryMapping,
        expected_registry: &str,
    ) -> bool {
        match registry_mapping {
            wasm_pkg_client::RegistryMapping::Registry(registry) => {
                registry.to_string() == expected_registry
            }
            _ => false,
        }
    }

    fn assert_default_registries(config: &wasm_pkg_client::Config) {
        config
            .resolve_registry(&"wasi:http".parse().unwrap())
            .expect("Should have a namespace set for wasi");
        config
            .namespace_registry(&"wasmcloud".parse().unwrap())
            .expect("Should have a namespace set for wasmcloud");
        config
            .namespace_registry(&"wrpc".parse().unwrap())
            .expect("Should have a namespace set for wrpc");
    }
}

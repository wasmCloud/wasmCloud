use std::path::PathBuf;

use crate::lib::{
    cli::{registry::AuthOpts, CommandOutput, OutputKind},
    registry::{pull_oci_artifact, OciPullOptions},
};
use anyhow::Context;
use clap::{Parser, Subcommand};
use futures::TryStreamExt;
use oci_client::Reference;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::{
    appearance::spinner::Spinner,
    ctl::plugins_table,
    util::{ensure_plugin_dir, load_plugins},
};

#[derive(Debug, Clone, Subcommand)]
pub enum PluginCommand {
    /// Install a wash plugin
    #[clap(name = "install")]
    Install(PluginInstallCommand),
    /// Uninstall a plugin
    #[clap(name = "uninstall", alias = "delete", alias = "rm")]
    Uninstall(PluginUninstallCommand),
    /// List installed plugins
    #[clap(name = "list", alias = "ls")]
    List(PluginListCommand),
}

#[derive(Parser, Debug, Clone)]
pub struct PluginCommonOpts {
    /// Path to plugin directory. Defaults to $HOME/.wash/plugins.
    #[clap(long = "plugin-dir", env = "WASH_PLUGIN_DIR")]
    pub plugin_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Parser)]
pub struct PluginInstallCommand {
    #[clap(flatten)]
    pub oci_auth: AuthOpts,

    /// URL of the plugin to install. Can be a file://, http://, https://, or oci:// URL.
    #[clap(name = "url")]
    pub url: String,

    /// Digest to verify plugin against. For OCI manifests, this is the digest format used in the
    /// manifest. For other types of plugins, this is the SHA256 digest of the plugin binary.
    #[clap(short = 'd', long = "digest")]
    pub digest: Option<String>,

    /// Allow latest artifact tags (if pulling from OCI registry)
    #[clap(long = "allow-latest")]
    pub allow_latest: bool,

    /// Whether or not to update the plugin if it is already installed. Defaults to false
    #[clap(long = "update")]
    pub update: bool,

    #[clap(flatten)]
    pub opts: PluginCommonOpts,
}

#[derive(Debug, Clone, Parser)]
pub struct PluginUninstallCommand {
    /// ID of the plugin to uninstall
    #[clap(name = "id")]
    pub plugin: String,

    #[clap(flatten)]
    pub opts: PluginCommonOpts,
}

#[derive(Debug, Clone, Parser)]
pub struct PluginListCommand {
    #[clap(flatten)]
    pub opts: PluginCommonOpts,
}

pub async fn handle_command(
    cmd: PluginCommand,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    match cmd {
        PluginCommand::Install(cmd) => handle_install(cmd, output_kind).await,
        PluginCommand::Uninstall(cmd) => handle_uninstall(cmd, output_kind).await,
        PluginCommand::List(cmd) => handle_list(cmd, output_kind).await,
    }
}

pub async fn handle_install(
    cmd: PluginInstallCommand,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    let plugin_dir = ensure_plugin_dir(cmd.opts.plugin_dir).await?;
    let spinner = Spinner::new(&output_kind)?;
    // Write the data to a temp file that will be cleaned and then we can move it to its real
    // location if everything is successful.
    let tempdir =
        tempfile::tempdir().context("Unable to create temp directory for plugin download")?;
    let temp_location = tempdir.path().join("temp_plugin.wasm");
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .read(true)
        .open(&temp_location)
        .await
        .context("Unable to create temp file for plugin download")?;

    let (scheme, rest) = cmd
        .url
        .split_once("://")
        .context("Invalid URL. It should contain a scheme (e.g. file://)")?;

    // OCI checks the digest on pull, so we have to return whether to check the digest for the others
    let compute_digest = match scheme {
        "file" => {
            let path = PathBuf::from(rest);
            spinner.update_spinner_message(format!(" Opening plugin from {}", path.display()));
            let mut existing_file = tokio::fs::File::open(&path)
                .await
                .context(format!("Unable to open plugin file at {}", path.display()))?;
            // NOTE(thomastaylor312): This is less efficient than just opening the file as a plugin,
            // but simplifies the logic so we can just move it at the end with all of the other
            // checks we have to do. We could also just read bytes in and load those, but that also
            // results in a whole bunch of extra code in the plugin runner code that we would only
            // need for this specific subcommand. We can improve this later if we need to.
            tokio::io::copy(&mut existing_file, &mut file)
                .await
                .context("Unable to copy plugin file")?;
            cmd.digest
        }
        "http" | "https" => {
            spinner.update_spinner_message(format!(" Downloading plugin from URL {}", cmd.url));
            let resp = reqwest::get(&cmd.url)
                .await
                .context("Unable to perform http request")?;
            if !resp.status().is_success() {
                anyhow::bail!(
                    "Unable to fetch plugin from {}. HTTP status code: {}",
                    cmd.url,
                    resp.status()
                );
            }
            let mut stream_reader = tokio_util::io::StreamReader::new(
                resp.bytes_stream()
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err)),
            );
            tokio::io::copy(&mut stream_reader, &mut file)
                .await
                .context("Unable to save plugin file to disk")?;

            cmd.digest
        }
        "oci" => {
            spinner.update_spinner_message(format!(" Downloading plugin from registry {rest}"));
            let image: Reference = rest
                .trim()
                .to_ascii_lowercase()
                .parse()
                .context("Invalid image reference")?;

            // TODO: Add support for pulling via stream to wash::lib
            let image_data = pull_oci_artifact(
                &image,
                OciPullOptions {
                    digest: cmd.digest.clone(),
                    allow_latest: cmd.allow_latest,
                    user: cmd.oci_auth.user,
                    password: cmd.oci_auth.password,
                    insecure: cmd.oci_auth.insecure,
                    insecure_skip_tls_verify: cmd.oci_auth.insecure_skip_tls_verify,
                },
            )
            .await
            .context("Unable to pull plugin from registry")?;
            file.write_all(&image_data)
                .await
                .context("Unable to write plugin to file")?;

            None
        }
        _ => {
            anyhow::bail!("Invalid URL scheme: {}", scheme);
        }
    };

    // Flush the file to make sure we're done writing to it.
    file.flush()
        .await
        .context("Unable to flush plugin file to disk")?;
    file.shutdown()
        .await
        .context("Unable to shutdown plugin file")?;

    // Check the digest if we have one
    if let Some(expected_digest) = compute_digest {
        spinner.update_spinner_message(" Computing digest");
        let mut digest = Sha256::new();
        let data = tokio::fs::read(&temp_location)
            .await
            .context("Unable to read plugin data for digest computation")?;
        digest.update(data);
        let hash = format!("{:x}", digest.finalize());
        let sanitized = expected_digest.trim().to_lowercase();
        anyhow::ensure!(
            hash != sanitized,
            "Digest mismatch. Expected {sanitized}, got {hash}"
        );
    }

    spinner.update_spinner_message(" Loading existing plugins");
    // Load existing plugins so we can check for duplicates.
    let mut plugins = load_plugins(&plugin_dir)
        .await
        .context("Unable to load existing plugins")?;

    spinner.update_spinner_message(" Validating plugin");
    let metadata = if cmd.update {
        plugins.update_plugin(&temp_location).await
    } else {
        plugins.add_plugin(&temp_location).await
    }
    .context("Unable to add plugin")?;

    spinner.update_spinner_message(" Installing plugin");

    // We already have ensured that this plugin is valid, so we can overwrite it even if it already
    // exists in the plugin dir.
    let final_location = plugin_dir.join(metadata.id.clone());
    tokio::fs::rename(temp_location, final_location)
        .await
        .context("Unable to install plugin in the plugin directory")?;
    spinner.finish_and_clear();

    Ok(CommandOutput {
        text: format!(
            "Plugin {} (version {}) installed",
            metadata.name, metadata.version
        ),
        map: [
            ("name".to_string(), metadata.name.into()),
            ("version".to_string(), metadata.version.into()),
            ("description".to_string(), metadata.description.into()),
        ]
        .into(),
    })
}

pub async fn handle_uninstall(
    cmd: PluginUninstallCommand,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    let plugin_dir = ensure_plugin_dir(cmd.opts.plugin_dir).await?;
    let spinner = Spinner::new(&output_kind)?;

    spinner.update_spinner_message(" Loading plugins");
    let plugins = load_plugins(plugin_dir)
        .await
        .context("Unable to load plugins")?;

    let metadata = if let Some(metadata) = plugins.metadata(&cmd.plugin) {
        metadata
    } else {
        let message = format!("Plugin {} is not currently installed", cmd.plugin);
        return Ok(CommandOutput {
            text: message.clone(),
            map: [
                ("uninstalled".to_string(), false.into()),
                ("message".to_string(), message.into()),
            ]
            .into(),
        });
    };

    spinner.update_spinner_message(" Uninstalling plugin");
    // Ok to unwrap because we know the plugin is installed from previous checks
    let path = plugins.path(&cmd.plugin).unwrap();
    tokio::fs::remove_file(path)
        .await
        .context("Unable to remove plugin")?;
    spinner.finish_and_clear();

    Ok(CommandOutput {
        text: format!(
            "Plugin {} (version {}) uninstalled",
            cmd.plugin, metadata.version
        ),
        map: [("uninstalled".to_string(), true.into())].into(),
    })
}

pub async fn handle_list(
    cmd: PluginListCommand,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    let plugin_dir = ensure_plugin_dir(cmd.opts.plugin_dir).await?;
    let spinner = Spinner::new(&output_kind)?;
    spinner.update_spinner_message(" Loading plugins");
    let plugins = load_plugins(plugin_dir)
        .await
        .context("Unable to load plugins")?;

    spinner.finish_and_clear();

    let data = plugins.all_metadata();

    Ok(CommandOutput {
        text: plugins_table(data.clone()),
        map: data
            .into_iter()
            .map(|m| {
                (
                    m.name.clone(),
                    serde_json::json!({
                        "version": m.version,
                        "description": m.description,
                        "id": m.id,
                        "name": m.name,
                        "author": m.author,
                    }),
                )
            })
            .collect(),
    })
}

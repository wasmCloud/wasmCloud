extern crate oci_distribution;

use crate::appearance::spinner::Spinner;
use crate::util::{cached_file, labels_vec_to_hashmap, CommandOutput, OutputKind};
use anyhow::{anyhow, bail, Result};
use clap::{Parser, Subcommand};
use log::{debug, warn};
use oci_distribution::manifest::OciImageManifest;
use oci_distribution::{client::*, secrets::RegistryAuth, Reference};
use provider_archive::ProviderArchive;
use serde_json::json;
use std::{collections::HashMap, fs::File, io::prelude::*};

const PROVIDER_ARCHIVE_MEDIA_TYPE: &str = "application/vnd.wasmcloud.provider.archive.layer.v1+par";
const PROVIDER_ARCHIVE_CONFIG_MEDIA_TYPE: &str =
    "application/vnd.wasmcloud.provider.archive.config";
const PROVIDER_ARCHIVE_FILE_EXTENSION: &str = ".par.gz";
const WASM_MEDIA_TYPE: &str = "application/vnd.module.wasm.content.layer.v1+wasm";
const WASM_CONFIG_MEDIA_TYPE: &str = "application/vnd.wasmcloud.actor.archive.config";
const OCI_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar";
const WASM_FILE_EXTENSION: &str = ".wasm";

pub(crate) const SHOWER_EMOJI: &str = "\u{1F6BF}";

pub(crate) enum SupportedArtifacts {
    Par,
    Wasm,
}

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum RegCliCommand {
    /// Pull an artifact from an OCI compliant registry
    #[clap(name = "pull")]
    Pull(PullCommand),
    /// Push an artifact to an OCI compliant registry
    #[clap(name = "push")]
    Push(PushCommand),
    /// Ping (test url) to see if the OCI url has an artifact
    #[clap(name = "ping")]
    Ping(PingCommand),
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct PullCommand {
    /// URL of artifact
    #[clap(name = "url")]
    pub(crate) url: String,

    /// File destination of artifact
    #[clap(long = "destination")]
    pub(crate) destination: Option<String>,

    /// Digest to verify artifact against
    #[clap(short = 'd', long = "digest")]
    pub(crate) digest: Option<String>,

    /// Allow latest artifact tags
    #[clap(long = "allow-latest")]
    pub(crate) allow_latest: bool,

    #[clap(flatten)]
    pub(crate) opts: AuthOpts,
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct PushCommand {
    /// URL to push artifact to
    #[clap(name = "url")]
    pub(crate) url: String,

    /// Path to artifact to push
    #[clap(name = "artifact")]
    pub(crate) artifact: String,

    /// Path to config file, if omitted will default to a blank configuration
    #[clap(short = 'c', long = "config")]
    pub(crate) config: Option<String>,

    /// Allow latest artifact tags
    #[clap(long = "allow-latest")]
    pub(crate) allow_latest: bool,

    /// Optional set of annotations to apply to the OCI artifact manifest
    #[clap(short = 'a', long = "annotation", name = "annotations")]
    pub(crate) annotations: Option<Vec<String>>,

    #[clap(flatten)]
    pub(crate) opts: AuthOpts,
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct PingCommand {
    /// URL of artifact
    #[clap(name = "url")]
    pub(crate) url: String,

    #[clap(flatten)]
    pub(crate) opts: AuthOpts,
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct AuthOpts {
    /// OCI username, if omitted anonymous authentication will be used
    #[clap(
        short = 'u',
        long = "user",
        env = "WASH_REG_USER",
        hide_env_values = true
    )]
    pub(crate) user: Option<String>,

    /// OCI password, if omitted anonymous authentication will be used
    #[clap(
        short = 'p',
        long = "password",
        env = "WASH_REG_PASSWORD",
        hide_env_values = true
    )]
    pub(crate) password: Option<String>,

    /// Allow insecure (HTTP) registry connections
    #[clap(long = "insecure")]
    pub(crate) insecure: bool,
}

pub(crate) async fn handle_command(
    command: RegCliCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    match command {
        RegCliCommand::Pull(cmd) => handle_pull(cmd, output_kind).await,
        RegCliCommand::Push(cmd) => handle_push(cmd, output_kind).await,
        RegCliCommand::Ping(cmd) => handle_ping(cmd).await,
    }
}

pub(crate) async fn handle_pull(
    cmd: PullCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let artifact_url = cmd.url.to_ascii_lowercase();
    let image: Reference = artifact_url.parse()?;

    let spinner = Spinner::new(&output_kind)?;
    spinner.update_spinner_message(format!(" Downloading {} ...", image.whole()));

    let artifact = pull_artifact(
        artifact_url,
        cmd.digest,
        cmd.allow_latest,
        cmd.opts.user,
        cmd.opts.password,
        cmd.opts.insecure,
    )
    .await?;

    let outfile = write_artifact(&artifact, &image, cmd.destination).await?;

    spinner.finish_and_clear();

    let mut map = HashMap::new();
    map.insert("file".to_string(), json!(outfile));
    Ok(CommandOutput::new(
        format!(
            "\n{} Successfully pulled and validated {}",
            SHOWER_EMOJI, outfile
        ),
        map,
    ))
}

/// Attempts to return a local artifact, then a cached one.
/// Falls back to pull from registry if neither is found.
pub(crate) async fn get_artifact(
    url: String,
    digest: Option<String>,
    allow_latest: bool,
    user: Option<String>,
    password: Option<String>,
    insecure: bool,
    no_cache: bool,
) -> Result<Vec<u8>> {
    if let Ok(mut local_artifact) = File::open(url.clone()) {
        let mut buf = Vec::new();
        local_artifact.read_to_end(&mut buf)?;
        Ok(buf)
    } else if let (Ok(mut cached_artifact), false) = (File::open(cached_file(&url)), no_cache) {
        let mut buf = Vec::new();
        cached_artifact.read_to_end(&mut buf)?;
        Ok(buf)
    } else {
        pull_artifact(url.clone(), digest, allow_latest, user, password, insecure).await
    }
}

pub(crate) async fn pull_artifact(
    url: String,
    digest: Option<String>,
    allow_latest: bool,
    user: Option<String>,
    password: Option<String>,
    insecure: bool,
) -> Result<Vec<u8>> {
    let image: Reference = url.parse()?;

    if image.tag().unwrap_or("latest") == "latest" && !allow_latest {
        bail!(
            "Pulling artifacts with tag 'latest' is prohibited. This can be overriden with the flag --allow-latest"
        );
    };

    let mut client = Client::new(ClientConfig {
        protocol: if insecure {
            ClientProtocol::Http
        } else {
            ClientProtocol::Https
        },
        ..Default::default()
    });

    let auth = match (user, password) {
        (Some(user), Some(password)) => RegistryAuth::Basic(user, password),
        _ => RegistryAuth::Anonymous,
    };

    let image_data = client
        .pull(
            &image,
            &auth,
            vec![PROVIDER_ARCHIVE_MEDIA_TYPE, WASM_MEDIA_TYPE, OCI_MEDIA_TYPE],
        )
        .await?;

    // Reformatting digest in case the sha256: prefix is left off
    let digest = match digest {
        Some(d) if d.starts_with("sha256:") => Some(d),
        Some(d) => Some(format!("sha256:{}", d)),
        None => None,
    };

    match (digest, image_data.digest) {
        (Some(digest), Some(image_digest)) if digest != image_digest => Err(anyhow!(
            "Image digest did not match provided digest, aborting"
        )),
        _ => {
            debug!("Image digest validated against provided digest");
            Ok(())
        }
    }?;

    Ok(image_data
        .layers
        .iter()
        .flat_map(|l| l.data.clone())
        .collect::<Vec<_>>())
}

pub(crate) async fn handle_ping(cmd: PingCommand) -> Result<CommandOutput> {
    let image: Reference = cmd.url.parse()?;
    let mut client = Client::new(ClientConfig {
        protocol: if cmd.opts.insecure {
            ClientProtocol::Http
        } else {
            ClientProtocol::Https
        },
        ..Default::default()
    });
    let auth = match (cmd.opts.user, cmd.opts.password) {
        (Some(user), Some(password)) => RegistryAuth::Basic(user, password),
        _ => RegistryAuth::Anonymous,
    };
    let (_, _) = client.pull_manifest(&image, &auth).await?;
    Ok(CommandOutput::from("Pong!"))
}

pub(crate) async fn write_artifact(
    artifact: &[u8],
    image: &Reference,
    output: Option<String>,
) -> Result<String> {
    let file_extension = match validate_artifact(artifact, image.repository()).await? {
        SupportedArtifacts::Par => PROVIDER_ARCHIVE_FILE_EXTENSION,
        SupportedArtifacts::Wasm => WASM_FILE_EXTENSION,
    };
    // Output to provided file, or use artifact_name.file_extension
    let outfile = output.unwrap_or(format!(
        "{}{}",
        image
            .repository()
            .to_string()
            .split('/')
            .collect::<Vec<_>>()
            .pop()
            .unwrap(),
        file_extension
    ));
    let mut f = File::create(outfile.clone())?;
    f.write_all(artifact)?;
    Ok(outfile)
}

/// Helper function to determine artifact type and validate that it is
/// a valid artifact of that type
pub(crate) async fn validate_artifact(artifact: &[u8], name: &str) -> Result<SupportedArtifacts> {
    match validate_actor_module(artifact, name) {
        Ok(_) => Ok(SupportedArtifacts::Wasm),
        Err(_) => match validate_provider_archive(artifact, name).await {
            Ok(_) => Ok(SupportedArtifacts::Par),
            Err(_) => bail!("Unsupported artifact type"),
        },
    }
}

/// Attempts to inspect the claims of an actor module
/// Will fail without actor claims, or if the artifact is invalid
fn validate_actor_module(artifact: &[u8], module: &str) -> Result<()> {
    match wascap::wasm::extract_claims(artifact) {
        Ok(Some(_token)) => Ok(()),
        Ok(None) => bail!("No capabilities discovered in actor module : {}", &module),
        Err(e) => Err(anyhow!("{}", e)),
    }
}

/// Attempts to unpack a provider archive
/// Will fail without claims or if the archive is invalid
async fn validate_provider_archive(artifact: &[u8], archive: &str) -> Result<()> {
    match ProviderArchive::try_load(artifact).await {
        Ok(_par) => Ok(()),
        Err(_e) => bail!("Invalid provider archive : {}", archive),
    }
}

pub(crate) async fn handle_push(
    cmd: PushCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let artifact_url = cmd.url.to_ascii_lowercase();
    if artifact_url.starts_with("localhost:") && !cmd.opts.insecure {
        warn!(" Unless an SSL certificate has been installed, pushing to localhost without the --insecure option will fail")
    }

    let spinner = Spinner::new(&output_kind)?;
    spinner.update_spinner_message(format!(" Pushing {} to {} ...", cmd.artifact, artifact_url));

    push_artifact(
        artifact_url.clone(),
        cmd.artifact,
        cmd.config,
        cmd.allow_latest,
        cmd.opts.user,
        cmd.opts.password,
        cmd.opts.insecure,
        cmd.annotations,
    )
    .await?;

    spinner.finish_and_clear();

    let mut map = HashMap::new();
    map.insert("url".to_string(), json!(cmd.url));
    Ok(CommandOutput::new(
        format!(
            "{} Successfully validated and pushed to {}",
            SHOWER_EMOJI, artifact_url
        ),
        map,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn push_artifact(
    url: String,
    artifact: String,
    config: Option<String>,
    allow_latest: bool,
    user: Option<String>,
    password: Option<String>,
    insecure: bool,
    annotations: Option<Vec<String>>,
) -> Result<()> {
    let image: Reference = url.parse()?;

    if image.tag().unwrap() == "latest" && !allow_latest {
        bail!(
            "Pushing artifacts with tag 'latest' is prohibited. This can be overriden with the flag --allow-latest"
        );
    };

    let mut artifact_buf = vec![];
    let mut f = File::open(artifact.clone())?;
    f.read_to_end(&mut artifact_buf)?;

    let (artifact_media_type, config_media_type) =
        match validate_artifact(&artifact_buf, &artifact).await? {
            SupportedArtifacts::Wasm => (WASM_MEDIA_TYPE, WASM_CONFIG_MEDIA_TYPE),
            SupportedArtifacts::Par => (
                PROVIDER_ARCHIVE_MEDIA_TYPE,
                PROVIDER_ARCHIVE_CONFIG_MEDIA_TYPE,
            ),
        };

    let mut config_buf = vec![];
    match config {
        Some(config_file) => {
            let mut f = File::open(config_file)?;
            f.read_to_end(&mut config_buf)?;
        }
        None => {
            // If no config provided, send blank config
            config_buf = b"{}".to_vec();
        }
    };
    let config = Config {
        data: config_buf,
        media_type: config_media_type.to_string(),
        annotations: None,
    };

    let layer = vec![ImageLayer {
        data: artifact_buf,
        media_type: artifact_media_type.to_string(),
        annotations: None,
    }];

    let mut client = Client::new(ClientConfig {
        protocol: if insecure {
            ClientProtocol::Http
        } else {
            ClientProtocol::Https
        },
        ..Default::default()
    });

    let auth = match (user, password) {
        (Some(user), Some(password)) => RegistryAuth::Basic(user, password),
        _ => RegistryAuth::Anonymous,
    };

    let manifest = OciImageManifest::build(
        &layer,
        &config,
        labels_vec_to_hashmap(annotations.unwrap_or_default()).ok(),
    );

    client
        .push(&image, &layer, config, &auth, Some(manifest))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PullCommand, PushCommand, RegCliCommand};
    use clap::Parser;

    const ECHO_WASM: &str = "wasmcloud.azurecr.io/echo:0.2.0";
    const LOCAL_REGISTRY: &str = "localhost:5000";

    #[derive(Debug, Parser)]
    struct Cmd {
        #[clap(subcommand)]
        reg: RegCliCommand,
    }

    #[test]
    /// Enumerates multiple options of the `pull` command to ensure API doesn't
    /// change between versions. This test will fail if `wash reg pull`
    /// changes syntax, ordering of required elements, or flags.
    fn test_pull_comprehensive() {
        // Not explicitly used, just a placeholder for a directory
        const TESTDIR: &str = "./tests/fixtures";

        let pull_basic: Cmd = Parser::try_parse_from(&["reg", "pull", ECHO_WASM]).unwrap();
        let pull_all_flags: Cmd =
            Parser::try_parse_from(&["reg", "pull", ECHO_WASM, "--allow-latest", "--insecure"])
                .unwrap();
        let pull_all_options: Cmd = Parser::try_parse_from(&[
            "reg",
            "pull",
            ECHO_WASM,
            "--destination",
            TESTDIR,
            "--digest",
            "sha256:a17a163afa8447622055deb049587641a9e23243a6cc4411eb33bd4267214cf3",
            "--password",
            "password",
            "--user",
            "user",
        ])
        .unwrap();
        match pull_basic.reg {
            RegCliCommand::Pull(PullCommand { url, .. }) => {
                assert_eq!(url, ECHO_WASM);
            }
            _ => panic!("`reg pull` constructed incorrect command"),
        };

        match pull_all_flags.reg {
            RegCliCommand::Pull(PullCommand {
                url,
                allow_latest,
                opts,
                ..
            }) => {
                assert_eq!(url, ECHO_WASM);
                assert!(allow_latest);
                assert!(opts.insecure);
            }
            _ => panic!("`reg pull` constructed incorrect command"),
        };

        match pull_all_options.reg {
            RegCliCommand::Pull(PullCommand {
                url,
                destination,
                digest,
                opts,
                ..
            }) => {
                assert_eq!(url, ECHO_WASM);
                assert_eq!(destination.unwrap(), TESTDIR);
                assert_eq!(
                    digest.unwrap(),
                    "sha256:a17a163afa8447622055deb049587641a9e23243a6cc4411eb33bd4267214cf3"
                );
                assert_eq!(opts.user.unwrap(), "user");
                assert_eq!(opts.password.unwrap(), "password");
            }
            _ => panic!("`reg pull` constructed incorrect command"),
        };
    }

    #[test]
    /// Enumerates multiple options of the `push` command to ensure API doesn't
    /// change between versions. This test will fail if `wash reg push`
    /// changes syntax, ordering of required elements, or flags.
    fn test_push_comprehensive() {
        // Not explicitly used, just a placeholder for a directory
        const TESTDIR: &str = "./tests/fixtures";

        // Push echo.wasm and pull from local registry
        let echo_push_basic = &format!("{}/echo:pushbasic", LOCAL_REGISTRY);
        let push_basic: Cmd = Parser::try_parse_from(&[
            "reg",
            "push",
            echo_push_basic,
            &format!("{}/echopush.wasm", TESTDIR),
            "--insecure",
        ])
        .unwrap();
        match push_basic.reg {
            RegCliCommand::Push(PushCommand {
                url,
                artifact,
                opts,
                ..
            }) => {
                assert_eq!(&url, echo_push_basic);
                assert_eq!(artifact, format!("{}/echopush.wasm", TESTDIR));
                assert!(opts.insecure);
            }
            _ => panic!("`reg push` constructed incorrect command"),
        };

        // Push logging.par.gz and pull from local registry
        let logging_push_all_flags = &format!("{}/logging:allflags", LOCAL_REGISTRY);
        let push_all_flags: Cmd = Parser::try_parse_from(&[
            "reg",
            "push",
            logging_push_all_flags,
            &format!("{}/logging.par.gz", TESTDIR),
            "--insecure",
            "--allow-latest",
        ])
        .unwrap();
        match push_all_flags.reg {
            RegCliCommand::Push(PushCommand {
                url,
                artifact,
                opts,
                allow_latest,
                ..
            }) => {
                assert_eq!(&url, logging_push_all_flags);
                assert_eq!(artifact, format!("{}/logging.par.gz", TESTDIR));
                assert!(opts.insecure);
                assert!(allow_latest);
            }
            _ => panic!("`reg push` constructed incorrect command"),
        };

        // Push logging.par.gz to different tag and pull to confirm successful push
        let logging_push_all_options = &format!("{}/logging:alloptions", LOCAL_REGISTRY);
        let push_all_options: Cmd = Parser::try_parse_from(&[
            "reg",
            "push",
            logging_push_all_options,
            &format!("{}/logging.par.gz", TESTDIR),
            "--allow-latest",
            "--insecure",
            "--config",
            &format!("{}/config.json", TESTDIR),
            "--password",
            "supers3cr3t",
            "--user",
            "localuser",
        ])
        .unwrap();
        match push_all_options.reg {
            RegCliCommand::Push(PushCommand {
                url,
                artifact,
                opts,
                allow_latest,
                config,
                ..
            }) => {
                assert_eq!(&url, logging_push_all_options);
                assert_eq!(artifact, format!("{}/logging.par.gz", TESTDIR));
                assert!(opts.insecure);
                assert!(allow_latest);
                assert_eq!(config.unwrap(), format!("{}/config.json", TESTDIR));
                assert_eq!(opts.user.unwrap(), "localuser");
                assert_eq!(opts.password.unwrap(), "supers3cr3t");
            }
            _ => panic!("`reg push` constructed incorrect command"),
        };
    }
}

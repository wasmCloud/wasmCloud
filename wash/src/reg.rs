extern crate oci_distribution;
use crate::util::{format_output, Output, OutputKind};
use log::{debug, info, warn};
use oci_distribution::client::*;
use oci_distribution::secrets::RegistryAuth;
use oci_distribution::Reference;
use provider_archive::ProviderArchive;
use serde_json::json;
use spinners::{Spinner, Spinners};
use std::fs::File;
use std::io::prelude::*;
use structopt::clap::AppSettings;
use structopt::StructOpt;

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

#[derive(Debug, StructOpt, Clone)]
#[structopt(
    global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
    name = "reg")]
pub(crate) struct RegCli {
    #[structopt(flatten)]
    command: RegCliCommand,
}

impl RegCli {
    pub(crate) fn command(self) -> RegCliCommand {
        self.command
    }
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) enum RegCliCommand {
    /// Pull an artifact from an OCI compliant registry
    #[structopt(name = "pull")]
    Pull(PullCommand),
    /// Push an artifact to an OCI compliant registry
    #[structopt(name = "push")]
    Push(PushCommand),
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct PullCommand {
    /// URL of artifact
    #[structopt(name = "url")]
    pub(crate) url: String,

    /// File destination of artifact
    #[structopt(long = "destination")]
    pub(crate) destination: Option<String>,

    /// Digest to verify artifact against
    #[structopt(short = "d", long = "digest")]
    pub(crate) digest: Option<String>,

    /// Allow latest artifact tags
    #[structopt(long = "allow-latest")]
    pub(crate) allow_latest: bool,

    #[structopt(flatten)]
    pub(crate) output: Output,

    #[structopt(flatten)]
    pub(crate) opts: AuthOpts,
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct PushCommand {
    /// URL to push artifact to
    #[structopt(name = "url")]
    pub(crate) url: String,

    /// Path to artifact to push
    #[structopt(name = "artifact")]
    pub(crate) artifact: String,

    /// Path to config file, if omitted will default to a blank configuration
    #[structopt(short = "c", long = "config")]
    pub(crate) config: Option<String>,

    /// Allow latest artifact tags
    #[structopt(long = "allow-latest")]
    pub(crate) allow_latest: bool,

    #[structopt(flatten)]
    pub(crate) output: Output,

    #[structopt(flatten)]
    pub(crate) opts: AuthOpts,
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct AuthOpts {
    /// OCI username, if omitted anonymous authentication will be used
    #[structopt(
        short = "u",
        long = "user",
        env = "WASH_REG_USER",
        hide_env_values = true
    )]
    pub(crate) user: Option<String>,

    /// OCI password, if omitted anonymous authentication will be used
    #[structopt(
        short = "p",
        long = "password",
        env = "WASH_REG_PASSWORD",
        hide_env_values = true
    )]
    pub(crate) password: Option<String>,

    /// Allow insecure (HTTP) registry connections
    #[structopt(long = "insecure")]
    pub(crate) insecure: bool,
}

pub(crate) async fn handle_command(
    command: RegCliCommand,
) -> Result<String, Box<dyn ::std::error::Error>> {
    match command {
        RegCliCommand::Pull(cmd) => handle_pull(cmd).await,
        RegCliCommand::Push(cmd) => handle_push(cmd).await,
    }
}

pub(crate) async fn handle_pull(cmd: PullCommand) -> Result<String, Box<dyn ::std::error::Error>> {
    let image: Reference = cmd.url.parse().unwrap();
    let spinner = match cmd.output.kind {
        OutputKind::Text => Some(Spinner::new(
            &Spinners::Dots12,
            format!(" Downloading {} ...", image.whole()),
        )),
        _ => None,
    };
    info!("Downloading {}", image.whole());
    let artifact = pull_artifact(
        cmd.url,
        cmd.digest,
        cmd.allow_latest,
        cmd.opts.user,
        cmd.opts.password,
        cmd.opts.insecure,
    )
    .await?;

    let outfile = write_artifact(&artifact, &image, cmd.destination)?;

    if spinner.is_some() {
        spinner.unwrap().stop();
    }

    Ok(format_output(
        format!(
            "\n{} Successfully pulled and validated {}",
            SHOWER_EMOJI, outfile
        ),
        json!({"result": "success", "file": outfile}),
        &cmd.output.kind,
    ))
}

pub(crate) async fn pull_artifact(
    url: String,
    digest: Option<String>,
    allow_latest: bool,
    user: Option<String>,
    password: Option<String>,
    insecure: bool,
) -> Result<Vec<u8>, Box<dyn ::std::error::Error>> {
    let image: Reference = url.parse()?;

    if image.tag().unwrap_or("latest") == "latest" && !allow_latest {
        return Err(
            "Pulling artifacts with tag 'latest' is prohibited. This can be overriden with a flag"
                .into(),
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
        (Some(digest), Some(image_digest)) if digest != image_digest => {
            Err("Image digest did not match provided digest, aborting")
        }
        _ => {
            debug!("Image digest validated against provided digest");
            Ok(())
        }
    }?;

    Ok(image_data
        .layers
        .iter()
        .map(|l| l.data.clone())
        .flatten()
        .collect::<Vec<_>>())
}

pub(crate) fn write_artifact(
    artifact: &[u8],
    image: &Reference,
    output: Option<String>,
) -> Result<String, Box<dyn ::std::error::Error>> {
    let file_extension = match validate_artifact(artifact, image.repository())? {
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
            .unwrap()
            .to_string(),
        file_extension
    ));
    let mut f = File::create(outfile.clone())?;
    f.write_all(artifact)?;
    Ok(outfile)
}

/// Helper function to determine artifact type and validate that it is
/// a valid artifact of that type
pub(crate) fn validate_artifact(
    artifact: &[u8],
    name: &str,
) -> Result<SupportedArtifacts, Box<dyn ::std::error::Error>> {
    match validate_actor_module(artifact, name) {
        Ok(_) => Ok(SupportedArtifacts::Wasm),
        Err(_) => match validate_provider_archive(artifact, name) {
            Ok(_) => Ok(SupportedArtifacts::Par),
            Err(_) => Err("Unsupported artifact type".into()),
        },
    }
}

/// Attempts to inspect the claims of an actor module
/// Will fail without actor claims, or if the artifact is invalid
fn validate_actor_module(
    artifact: &[u8],
    module: &str,
) -> Result<(), Box<dyn ::std::error::Error>> {
    match wascap::wasm::extract_claims(&artifact) {
        Ok(Some(_token)) => Ok(()),
        Ok(None) => Err(format!("No capabilities discovered in actor module : {}", &module).into()),
        Err(e) => Err(Box::new(e)),
    }
}

/// Attempts to unpack a provider archive
/// Will fail without claims or if the archive is invalid
fn validate_provider_archive(
    artifact: &[u8],
    archive: &str,
) -> Result<(), Box<dyn ::std::error::Error>> {
    match ProviderArchive::try_load(artifact) {
        Ok(_par) => Ok(()),
        Err(_e) => Err(format!("Invalid provider archive : {}", archive).into()),
    }
}

pub(crate) async fn handle_push(cmd: PushCommand) -> Result<String, Box<dyn ::std::error::Error>> {
    if cmd.url.starts_with("localhost:") && !cmd.opts.insecure {
        warn!(" Unless an SSL certificate has been installed, pushing to localhost without the --insecure option will fail")
    }

    let spinner = match cmd.output.kind {
        OutputKind::Text => Some(Spinner::new(
            &Spinners::Dots12,
            format!(" Pushing {} to {} ...", cmd.artifact, cmd.url),
        )),
        _ => None,
    };
    info!(" Pushing {} to {} ...", cmd.artifact, cmd.url);

    push_artifact(
        cmd.url.clone(),
        cmd.artifact,
        cmd.config,
        cmd.allow_latest,
        cmd.opts.user,
        cmd.opts.password,
        cmd.opts.insecure,
    )
    .await?;

    if spinner.is_some() {
        spinner.unwrap().stop();
    }
    Ok(format_output(
        format!(
            "\n{} Successfully validated and pushed to {}",
            SHOWER_EMOJI, cmd.url
        ),
        json!({"result": "success", "url": cmd.url}),
        &cmd.output.kind,
    ))
}

pub(crate) async fn push_artifact(
    url: String,
    artifact: String,
    config: Option<String>,
    allow_latest: bool,
    user: Option<String>,
    password: Option<String>,
    insecure: bool,
) -> Result<(), Box<dyn ::std::error::Error>> {
    let image: Reference = url.parse().unwrap();

    if image.tag().unwrap() == "latest" && !allow_latest {
        return Err(
            "Pushing artifacts with tag 'latest' is prohibited. This can be overriden with a flag"
                .into(),
        );
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

    let mut artifact_buf = vec![];
    let mut f = File::open(artifact.clone())?;
    f.read_to_end(&mut artifact_buf)?;

    let (artifact_media_type, config_media_type) =
        match validate_artifact(&artifact_buf, &artifact)? {
            SupportedArtifacts::Wasm => (WASM_MEDIA_TYPE, WASM_CONFIG_MEDIA_TYPE),
            SupportedArtifacts::Par => (
                PROVIDER_ARCHIVE_MEDIA_TYPE,
                PROVIDER_ARCHIVE_CONFIG_MEDIA_TYPE,
            ),
        };

    let image_data = ImageData {
        layers: vec![ImageLayer {
            data: artifact_buf,
            media_type: artifact_media_type.to_string(),
        }],
        digest: None,
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

    client
        .push(
            &image,
            &image_data,
            &config_buf,
            config_media_type,
            &auth,
            None,
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PullCommand, PushCommand, RegCli, RegCliCommand};
    use crate::util::OutputKind;
    use structopt::StructOpt;

    const ECHO_WASM: &str = "wasmcloud.azurecr.io/echo:0.2.0";
    const LOCAL_REGISTRY: &str = "localhost:5000";

    #[test]
    /// Enumerates multiple options of the `pull` command to ensure API doesn't
    /// change between versions. This test will fail if `wash reg pull`
    /// changes syntax, ordering of required elements, or flags.
    fn test_pull_comprehensive() {
        // Not explicitly used, just a placeholder for a directory
        const TESTDIR: &str = "./tests/fixtures";

        let pull_basic = RegCli::from_iter(&["reg", "pull", ECHO_WASM]);
        let pull_all_flags =
            RegCli::from_iter(&["reg", "pull", ECHO_WASM, "--allow-latest", "--insecure"]);
        let pull_all_options = RegCli::from_iter(&[
            "reg",
            "pull",
            ECHO_WASM,
            "--destination",
            TESTDIR,
            "--digest",
            "sha256:a17a163afa8447622055deb049587641a9e23243a6cc4411eb33bd4267214cf3",
            "--output",
            "text",
            "--password",
            "password",
            "--user",
            "user",
        ]);
        match pull_basic.command {
            RegCliCommand::Pull(PullCommand { url, .. }) => {
                assert_eq!(url, ECHO_WASM);
            }
            _ => panic!("`reg pull` constructed incorrect command"),
        };

        match pull_all_flags.command {
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

        match pull_all_options.command {
            RegCliCommand::Pull(PullCommand {
                url,
                destination,
                digest,
                output,
                opts,
                ..
            }) => {
                assert_eq!(url, ECHO_WASM);
                assert_eq!(destination.unwrap(), TESTDIR);
                assert_eq!(
                    digest.unwrap(),
                    "sha256:a17a163afa8447622055deb049587641a9e23243a6cc4411eb33bd4267214cf3"
                );
                assert_eq!(output.kind, OutputKind::Text);
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
        let push_basic = RegCli::from_iter(&[
            "reg",
            "push",
            echo_push_basic,
            &format!("{}/echopush.wasm", TESTDIR),
            "--insecure",
        ]);
        match push_basic.command {
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
        let push_all_flags = RegCli::from_iter(&[
            "reg",
            "push",
            logging_push_all_flags,
            &format!("{}/logging.par.gz", TESTDIR),
            "--insecure",
            "--allow-latest",
        ]);
        match push_all_flags.command {
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
        let push_all_options = RegCli::from_iter(&[
            "reg",
            "push",
            logging_push_all_options,
            &format!("{}/logging.par.gz", TESTDIR),
            "--allow-latest",
            "--insecure",
            "--config",
            &format!("{}/config.json", TESTDIR),
            "--output",
            "json",
            "--password",
            "supers3cr3t",
            "--user",
            "localuser",
        ]);
        match push_all_options.command {
            RegCliCommand::Push(PushCommand {
                url,
                artifact,
                opts,
                allow_latest,
                config,
                output,
                ..
            }) => {
                assert_eq!(&url, logging_push_all_options);
                assert_eq!(artifact, format!("{}/logging.par.gz", TESTDIR));
                assert!(opts.insecure);
                assert!(allow_latest);
                assert_eq!(config.unwrap(), format!("{}/config.json", TESTDIR));
                assert_eq!(opts.user.unwrap(), "localuser");
                assert_eq!(opts.password.unwrap(), "supers3cr3t");
                assert_eq!(output.kind, OutputKind::Json);
            }
            _ => panic!("`reg push` constructed incorrect command"),
        };
    }
}

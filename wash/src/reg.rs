use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use log::warn;
use oci_distribution::{
    client::{Client, ClientConfig, ClientProtocol},
    secrets::RegistryAuth,
    Reference,
};
use serde_json::json;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use wash_lib::cli::{labels_vec_to_hashmap, CommandOutput, OutputKind};
use wash_lib::registry::{
    pull_oci_artifact, push_oci_artifact, validate_artifact, OciPullOptions, OciPushOptions,
    SupportedArtifacts,
};

use crate::appearance::spinner::Spinner;

const PROVIDER_ARCHIVE_FILE_EXTENSION: &str = ".par.gz";
const WASM_FILE_EXTENSION: &str = ".wasm";

pub(crate) const SHOWER_EMOJI: &str = "\u{1F6BF}";

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

    let artifact = pull_oci_artifact(
        artifact_url,
        OciPullOptions {
            digest: cmd.digest,
            allow_latest: cmd.allow_latest,
            user: cmd.opts.user,
            password: cmd.opts.password,
            insecure: cmd.opts.insecure,
        },
    )
    .await?;

    let outfile = write_artifact(&artifact, &image, cmd.destination).await?;

    spinner.finish_and_clear();

    let mut map = HashMap::new();
    map.insert("file".to_string(), json!(outfile));
    Ok(CommandOutput::new(
        format!("\n{SHOWER_EMOJI} Successfully pulled and validated {outfile}"),
        map,
    ))
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
    let file_extension = match validate_artifact(artifact).await? {
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
    let mut f = File::create(outfile.clone()).await?;
    f.write_all(artifact).await?;
    Ok(outfile)
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

    let annotations = labels_vec_to_hashmap(cmd.annotations.unwrap_or_default())?;

    push_oci_artifact(
        artifact_url.clone(),
        cmd.artifact,
        OciPushOptions {
            config: cmd.config.map(PathBuf::from),
            allow_latest: cmd.allow_latest,
            user: cmd.opts.user,
            password: cmd.opts.password,
            insecure: cmd.opts.insecure,
            annotations: Some(annotations),
        },
    )
    .await?;

    spinner.finish_and_clear();

    let mut map = HashMap::new();
    map.insert("url".to_string(), json!(cmd.url));
    Ok(CommandOutput::new(
        format!("{SHOWER_EMOJI} Successfully validated and pushed to {artifact_url}"),
        map,
    ))
}

#[cfg(test)]
mod tests {
    use super::{PullCommand, PushCommand, RegCliCommand};
    use clap::Parser;

    const ECHO_WASM: &str = "wasmcloud.azurecr.io/echo:0.2.0";
    const LOCAL_REGISTRY: &str = "localhost:5001";

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

        let pull_basic: Cmd = Parser::try_parse_from(["reg", "pull", ECHO_WASM]).unwrap();
        let pull_all_flags: Cmd =
            Parser::try_parse_from(["reg", "pull", ECHO_WASM, "--allow-latest", "--insecure"])
                .unwrap();
        let pull_all_options: Cmd = Parser::try_parse_from([
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
        let echo_push_basic = &format!("{LOCAL_REGISTRY}/echo:pushbasic");
        let push_basic: Cmd = Parser::try_parse_from([
            "reg",
            "push",
            echo_push_basic,
            &format!("{TESTDIR}/echopush.wasm"),
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
                assert_eq!(artifact, format!("{TESTDIR}/echopush.wasm"));
                assert!(opts.insecure);
            }
            _ => panic!("`reg push` constructed incorrect command"),
        };

        // Push logging.par.gz and pull from local registry
        let logging_push_all_flags = &format!("{LOCAL_REGISTRY}/logging:allflags");
        let push_all_flags: Cmd = Parser::try_parse_from([
            "reg",
            "push",
            logging_push_all_flags,
            &format!("{TESTDIR}/logging.par.gz"),
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
                assert_eq!(artifact, format!("{TESTDIR}/logging.par.gz"));
                assert!(opts.insecure);
                assert!(allow_latest);
            }
            _ => panic!("`reg push` constructed incorrect command"),
        };

        // Push logging.par.gz to different tag and pull to confirm successful push
        let logging_push_all_options = &format!("{LOCAL_REGISTRY}/logging:alloptions");
        let push_all_options: Cmd = Parser::try_parse_from([
            "reg",
            "push",
            logging_push_all_options,
            &format!("{TESTDIR}/logging.par.gz"),
            "--allow-latest",
            "--insecure",
            "--config",
            &format!("{TESTDIR}/config.json"),
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
                assert_eq!(artifact, format!("{TESTDIR}/logging.par.gz"));
                assert!(opts.insecure);
                assert!(allow_latest);
                assert_eq!(config.unwrap(), format!("{TESTDIR}/config.json"));
                assert_eq!(opts.user.unwrap(), "localuser");
                assert_eq!(opts.password.unwrap(), "supers3cr3t");
            }
            _ => panic!("`reg push` constructed incorrect command"),
        };
    }
}

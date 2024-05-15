use std::{collections::HashMap, path::PathBuf};

use anyhow::{bail, Context, Result};
use oci_distribution::Reference;
use serde_json::json;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::warn;
use wash_lib::registry::{
    parse_and_validate_artifact, pull_oci_artifact, push_oci_artifact, OciPullOptions,
    OciPushOptions, SupportedArtifacts,
};
use wash_lib::{
    cli::{
        input_vec_to_hashmap,
        registry::{RegistryPullCommand, RegistryPushCommand},
        CommandOutput, OutputKind,
    },
    parser::get_config,
};
use wasmcloud_control_interface::RegistryCredential;

use crate::appearance::spinner::Spinner;

pub const SHOWER_EMOJI: &str = "\u{1F6BF}";
pub const PROVIDER_ARCHIVE_FILE_EXTENSION: &str = ".par.gz";
pub const WASM_FILE_EXTENSION: &str = ".wasm";

pub async fn registry_pull(
    cmd: RegistryPullCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let image: Reference = resolve_artifact_ref(&cmd.url, &cmd.registry.unwrap_or_default(), None)?;
    let spinner = Spinner::new(&output_kind)?;
    spinner.update_spinner_message(format!(" Downloading {} ...", image.whole()));

    let credentials = match (cmd.opts.user, cmd.opts.password) {
        (Some(user), Some(password)) => Ok(RegistryCredential {
            username: Some(user),
            password: Some(password),
            ..Default::default()
        }),
        _ => resolve_registry_credentials(image.registry()).await,
    }?;

    let artifact = pull_oci_artifact(
        &image,
        OciPullOptions {
            digest: cmd.digest,
            allow_latest: cmd.allow_latest,
            user: credentials.username,
            password: credentials.password,
            insecure: cmd.opts.insecure,
            insecure_skip_tls_verify: cmd.opts.insecure_skip_tls_verify,
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

pub async fn write_artifact(
    artifact: &[u8],
    image: &Reference,
    output: Option<String>,
) -> Result<String> {
    let file_extension = match parse_and_validate_artifact(artifact.to_vec()).await? {
        SupportedArtifacts::Par(..) => PROVIDER_ARCHIVE_FILE_EXTENSION,
        SupportedArtifacts::Wasm(..) => WASM_FILE_EXTENSION,
    };
    // Output to provided file, or use artifact_name.file_extension
    let outfile = output.unwrap_or_else(|| {
        format!(
            "{}{file_extension}",
            image.repository().split('/').last().unwrap(),
        )
    });
    let mut f = File::create(&outfile).await?;
    f.write_all(artifact).await?;
    // https://github.com/wasmCloud/wash/issues/382 resolved by this
    // Files must be synced to ensure all bytes are written to disk
    f.sync_all().await?;
    Ok(outfile)
}

pub async fn registry_push(
    cmd: RegistryPushCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let image: Reference = resolve_artifact_ref(
        &cmd.url,
        &cmd.registry.unwrap_or_default(),
        cmd.config.clone(),
    )?;
    let artifact_url = image.whole();
    if artifact_url.starts_with("localhost:") && !cmd.opts.insecure {
        warn!(" Unless an SSL certificate has been installed, pushing to localhost without the --insecure option will fail")
    }

    let spinner = Spinner::new(&output_kind)?;
    spinner.update_spinner_message(format!(" Pushing {} to {} ...", cmd.artifact, artifact_url));

    let credentials = match (cmd.opts.user, cmd.opts.password) {
        (Some(user), Some(password)) => Ok(RegistryCredential {
            username: Some(user),
            password: Some(password),
            ..Default::default()
        }),
        _ => resolve_registry_credentials(image.registry()).await,
    }?;

    let annotations = match cmd.annotations {
        Some(annotations) => input_vec_to_hashmap(annotations).ok(),
        None => None,
    };

    let (maybe_tag, digest) = push_oci_artifact(
        artifact_url.clone(),
        cmd.artifact,
        OciPushOptions {
            config: cmd.config.map(PathBuf::from),
            allow_latest: cmd.allow_latest,
            user: credentials.username,
            password: credentials.password,
            insecure: cmd.opts.insecure,
            insecure_skip_tls_verify: cmd.opts.insecure_skip_tls_verify,
            annotations,
        },
    )
    .await?;

    spinner.finish_and_clear();

    let mut map = HashMap::from_iter([
        ("url".to_string(), json!(artifact_url)),
        ("digest".to_string(), json!(digest)),
    ]);
    let text = if let Some(tag) = maybe_tag {
        map.insert("tag".to_string(), json!(tag));
        format!("{SHOWER_EMOJI} Successfully pushed {artifact_url}\n{tag}: digest: {digest}")
    } else {
        format!("{SHOWER_EMOJI} Successfully pushed {artifact_url}\ndigest: {digest}")
    };
    Ok(CommandOutput::new(text, map))
}

fn resolve_artifact_ref(
    url: &str,
    registry: &str,
    project_config: Option<PathBuf>,
) -> Result<Reference> {
    // NOTE: Image URLs must be all lower case for `oci_distribution::Reference` to parse them properly
    let url = url.trim().to_ascii_lowercase();
    let registry = registry.trim().to_ascii_lowercase();

    let image: Reference = url
        .parse()
        .context("failed to parse artifact url into oci image reference")?;

    if url == image.whole() {
        return Ok(image);
    }

    if !url.is_empty() && !registry.is_empty() {
        let image: Reference = format!("{}/{}", registry, url)
            .parse()
            .context("failed to parse artifact url from specified registry and repository")?;

        return Ok(image);
    }

    if !url.is_empty() && registry.is_empty() {
        let project_config = get_config(project_config, Some(true))?;
        let registry = project_config
            .common
            .registry
            .url
            .clone()
            .unwrap_or_default();

        if registry.is_empty() {
            bail!("Missing or invalid registry url configuration")
        }

        let image: Reference = format!("{}/{}", registry, url).parse().context(
            "failed to parse artifact url from specified repository and registry url configuration",
        )?;

        return Ok(image);
    }

    bail!("Unable to resolve artifact url from specified registry and repository")
}

async fn resolve_registry_credentials(registry: &str) -> Result<RegistryCredential> {
    let Ok(project_config) = get_config(None, Some(true)) else {
        return Ok(RegistryCredential::default());
    };

    project_config.resolve_registry_credentials(registry).await
}

#[cfg(test)]
mod tests {
    use anyhow::{ensure, Context as _, Result};
    use clap::Parser;
    use wash_lib::cli::registry::{RegistryCommand, RegistryPullCommand};

    use crate::common::registry_cmd::RegistryPushCommand;

    const ECHO_WASM: &str = "wasmcloud.azurecr.io/echo:0.2.0";
    const LOCAL_REGISTRY: &str = "localhost:5001";
    const TESTDIR: &str = "./tests/fixtures";

    // Partial wash command
    #[derive(Debug, Parser)]
    struct Cmd {
        #[clap(subcommand)]
        sub: RegistryCommand,
    }

    #[test]
    /// Enumerates multiple options of the `pull` command to ensure API doesn't
    /// change between versions. This test will fail if `wash reg pull`
    /// changes syntax, ordering of required elements, or flags.
    fn test_pull_comprehensive() -> Result<()> {
        // test basic `wash reg pull`
        let pull_basic: Cmd = Parser::try_parse_from(["wash", "pull", ECHO_WASM])
            .context("failed to perform reg pull")?;
        ensure!(matches!(
            pull_basic.sub,
            RegistryCommand::Pull(RegistryPullCommand { url, .. }) if url == ECHO_WASM,
        ));

        // test `wash reg pull`
        let pull_all_flags: Cmd =
            Parser::try_parse_from(["wash", "pull", ECHO_WASM, "--allow-latest", "--insecure"])
                .context("failed to pull with all flags")?;
        ensure!(matches!(
            pull_all_flags.sub,
            RegistryCommand::Pull(RegistryPullCommand {
                url,
                allow_latest,
                opts,
                ..
            }) if url == ECHO_WASM && allow_latest && opts.insecure
        ));

        // test `wash pull`
        let pull_all_options: Cmd = Parser::try_parse_from([
            "wash",
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
        .context("wash pull with all options failed")?;
        ensure!(matches!(
            pull_all_options.sub,
            RegistryCommand::Pull(RegistryPullCommand {
                url,
                destination,
                digest,
                opts,
                ..
            }) if url == ECHO_WASM
                && destination == Some(TESTDIR.into())
                && digest == Some("sha256:a17a163afa8447622055deb049587641a9e23243a6cc4411eb33bd4267214cf3".into())
                && opts.user == Some("user".into())
                && opts.password == Some("password".into())
        ));

        Ok(())
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
            "wash",
            "push",
            echo_push_basic,
            &format!("{TESTDIR}/echopush.wasm"),
            "--insecure",
        ])
        .unwrap();
        match push_basic.sub {
            RegistryCommand::Push(RegistryPushCommand {
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
            "wash",
            "push",
            logging_push_all_flags,
            &format!("{TESTDIR}/logging.par.gz"),
            "--insecure",
            "--allow-latest",
        ])
        .unwrap();
        match push_all_flags.sub {
            RegistryCommand::Push(RegistryPushCommand {
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
            "wash",
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
        match push_all_options.sub {
            RegistryCommand::Push(RegistryPushCommand {
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
                assert_eq!(
                    format!("{}", config.unwrap().as_path().display()),
                    format!("{TESTDIR}/config.json")
                );
                assert_eq!(opts.user.unwrap(), "localuser");
                assert_eq!(opts.password.unwrap(), "supers3cr3t");
            }
            _ => panic!("`reg push` constructed incorrect command"),
        };
    }
}

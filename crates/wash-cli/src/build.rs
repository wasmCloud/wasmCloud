use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use serde_json::json;

use wash_lib::{
    build::{build_project, sign_actor_wasm, SignConfig},
    cli::CommandOutput,
    parser::{get_config, TypeConfig},
};

/// Build (and sign) a wasmCloud actor, provider, or interface
#[derive(Debug, Parser, Clone)]
#[clap(name = "build")]
pub struct BuildCommand {
    /// Path to the wasmcloud.toml file or parent folder to use for building
    #[clap(short = 'p', long = "config-path")]
    config_path: Option<PathBuf>,

    /// Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
    #[clap(long = "keys-directory", env = "WASH_KEYS", hide_env_values = true)]
    pub keys_directory: Option<PathBuf>,

    /// Path to issuer seed key (account). If this flag is not provided, the seed will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 'i',
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    pub issuer: Option<String>,

    /// Path to subject seed key (module or service). If this flag is not provided, the seed will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 's',
        long = "subject",
        env = "WASH_SUBJECT_KEY",
        hide_env_values = true
    )]
    pub subject: Option<String>,

    /// Disables autogeneration of keys if seed(s) are not provided
    #[clap(long = "disable-keygen")]
    pub disable_keygen: bool,

    /// Skip signing the artifact and only use the native toolchain to build
    #[clap(long = "build-only", conflicts_with = "sign_only")]
    pub build_only: bool,

    /// Skip building the artifact and only use configuration to sign
    #[clap(long = "sign-only", conflicts_with = "build_only")]
    pub sign_only: bool,
}

pub async fn handle_command(command: BuildCommand) -> Result<CommandOutput> {
    let config = get_config(command.config_path, Some(true))?;

    match config.project_type {
        TypeConfig::Actor(ref actor_config) => {
            let sign_config = if command.build_only {
                None
            } else {
                Some(SignConfig {
                    keys_directory: command
                        .keys_directory
                        .clone()
                        .or(Some(actor_config.key_directory.to_path_buf())),
                    issuer: command.issuer,
                    subject: command.subject,
                    disable_keygen: command.disable_keygen,
                })
            };

            let component_path = if command.sign_only {
                std::env::set_current_dir(&config.common.path)?;
                let actor_wasm_path = if let Some(path) = actor_config.build_artifact.clone() {
                    path
                } else {
                    PathBuf::from(format!("build/{}.wasm", config.common.wasm_bin_name()))
                };
                let signed_path = sign_actor_wasm(
                    &config.common,
                    actor_config,
                    // We prevent supplying both fields in the CLI parser, so this `context` is just a safety fallback
                    &sign_config.context("cannot supply --build-only and --sign-only")?,
                    actor_wasm_path,
                )?;
                config.common.path.join(signed_path)
            } else {
                build_project(&config, sign_config.as_ref()).await?
            };

            let json_output = HashMap::from([
                ("component_path".to_string(), json!(component_path)),
                ("built".to_string(), json!(!command.sign_only)),
                ("signed".to_string(), json!(!command.build_only)),
            ]);
            Ok(CommandOutput::new(
                if command.build_only {
                    format!("Component built and can be found at {component_path:?}")
                } else if command.sign_only {
                    format!("Component signed and can be found at {component_path:?}")
                } else {
                    format!("Component built and signed and can be found at {component_path:?}")
                },
                json_output,
            ))
        }
        TypeConfig::Provider(ref provider_config) => {
            let path = build_project(
                &config,
                Some(&SignConfig {
                    keys_directory: command
                        .keys_directory
                        .clone()
                        .or(Some(provider_config.key_directory.to_path_buf())),
                    issuer: command.issuer,
                    subject: command.subject,
                    disable_keygen: command.disable_keygen,
                }),
            )
            .await?;
            Ok(CommandOutput::new(
                format!("Built artifact can be found at {path:?}"),
                HashMap::from([("path".to_string(), json!(path))]),
            ))
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use clap::Parser;

    #[test]
    fn test_build_comprehensive() {
        let cmd: BuildCommand = Parser::try_parse_from(["build"]).unwrap();
        assert!(cmd.config_path.is_none());
        assert!(!cmd.disable_keygen);
        assert!(cmd.issuer.is_none());
        assert!(cmd.subject.is_none());
        assert!(cmd.keys_directory.is_none());

        let cmd: BuildCommand = Parser::try_parse_from([
            "build",
            "-p",
            "/",
            "--disable-keygen",
            "--issuer",
            "/tmp/iss.nk",
            "--subject",
            "/tmp/sub.nk",
            "--keys-directory",
            "/tmp",
        ])
        .unwrap();
        assert_eq!(cmd.config_path, Some(PathBuf::from("/")));
        assert!(cmd.disable_keygen);
        assert_eq!(cmd.issuer, Some("/tmp/iss.nk".to_string()));
        assert_eq!(cmd.subject, Some("/tmp/sub.nk".to_string()));
        assert_eq!(cmd.keys_directory, Some(PathBuf::from("/tmp")));
    }
}

use std::io::ErrorKind;
use std::path::PathBuf;
use std::process;

use anyhow::{anyhow, bail, Context, Result};
use nkeys::KeyPairType;
use tracing::{trace, warn};

use crate::build::SignConfig;
use crate::cli::par::{create_provider_archive, detect_arch, ParCreateArgs};
use crate::cli::{extract_keypair, OutputKind};
use crate::parser::{CommonConfig, GoConfig, LanguageConfig, ProviderConfig, RustConfig};

/// Build a capability provider for the current machine's architecture
/// and operating system using provided configuration.
pub(crate) async fn build_provider(
    provider_config: &ProviderConfig,
    language_config: &LanguageConfig,
    common_config: &CommonConfig,
    signing_config: Option<&SignConfig>,
) -> Result<PathBuf> {
    let (provider_path_buf, bin_name) = match language_config {
        LanguageConfig::Rust(rust_config) => {
            build_rust_provider(provider_config, rust_config, common_config)?
        }
        LanguageConfig::Go(go_config) => {
            build_go_provider(provider_config, go_config, common_config)?
        }
        _ => bail!("Unsupported language for provider: {:?}", language_config),
    };

    trace!("Retrieving provider binary from {:?}", provider_path_buf);

    let provider_bytes = tokio::fs::read(&provider_path_buf).await?;

    let mut par = create_provider_archive(
        ParCreateArgs {
            vendor: provider_config.vendor.to_string(),
            revision: Some(common_config.revision),
            version: Some(common_config.version.to_string()),
            schema: None,
            name: common_config.name.to_string(),
            arch: detect_arch(),
        },
        &provider_bytes,
    )
    .context("failed to create initial provider archive with built provider")?;

    // If no signing config supplied, just return the path to the provider
    let Some(sign_config) = signing_config else {
        warn!("No signing configuration supplied, could only build provider");
        return Ok(provider_path_buf);
    };

    let destination = common_config
        .path
        .join("build")
        .join(format!("{bin_name}.par.gz"));
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let issuer = extract_keypair(
        sign_config.issuer.as_deref(),
        Some(&provider_path_buf.to_string_lossy()),
        sign_config.keys_directory.clone(),
        KeyPairType::Account,
        sign_config.disable_keygen,
        OutputKind::Json,
    )?;
    let subject = extract_keypair(
        sign_config.subject.as_deref(),
        Some(&provider_path_buf.to_string_lossy()),
        sign_config.keys_directory.clone(),
        KeyPairType::Service,
        sign_config.disable_keygen,
        OutputKind::Json,
    )?;
    par.write(destination.as_path(), &issuer, &subject, true)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    Ok(if destination.is_absolute() {
        destination
    } else {
        common_config.path.join(destination)
    })
}

/// Build a Rust provider for the current machine's architecture
///
/// Returns a tuple of the path to the built provider binary and the binary name
fn build_rust_provider(
    provider_config: &ProviderConfig,
    rust_config: &RustConfig,
    common_config: &CommonConfig,
) -> Result<(PathBuf, String)> {
    let mut command = match rust_config.cargo_path.as_ref() {
        Some(path) => process::Command::new(path),
        None => process::Command::new("cargo"),
    };

    // Change directory into the project directory
    std::env::set_current_dir(&common_config.path)?;
    trace!("Building provider in {:?}", common_config.path);

    // Build for a specified target if provided, or the default rust target
    let mut build_args = Vec::with_capacity(4);
    build_args.push("build");

    if !rust_config.debug {
        build_args.push("--release");
    }
    
    if let Some(override_target) = &provider_config.rust_target {
        build_args.extend_from_slice(&["--target", override_target]);
    };

    let result = command.args(build_args).status().map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            anyhow!("{:?} command is not found", command.get_program())
        } else {
            anyhow!(e)
        }
    })?;

    if !result.success() {
        bail!("Compiling provider failed: {result}")
    }

    let metadata = cargo_metadata::MetadataCommand::new().no_deps().exec()?;
    let bin_name = if let Some(bin_name) = &provider_config.bin_name {
        bin_name.to_string()
    } else {
        // Discover the binary name from the metadata
        metadata
            .packages
            .iter()
            .find_map(|p| {
                p.targets.iter().find_map(|t| {
                    if t.kind.iter().any(|k| k == "bin") {
                        Some(t.name.clone())
                    } else {
                        None
                    }
                })
            }).context("Could not infer provider binary name in metadata, please specify under provider.bin_name")?
    };
    let mut provider_path_buf = rust_config
        .target_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(metadata.target_directory.as_std_path()));
    if let Some(override_target) = &provider_config.rust_target {
        provider_path_buf.push(override_target);
    }

    provider_path_buf.push("release");
    provider_path_buf.push(&bin_name);

    Ok((provider_path_buf, bin_name))
}

/// Build a Go provider for the current machine's architecture
///
/// Returns a tuple of the path to the built provider binary and the binary name
fn build_go_provider(
    provider_config: &ProviderConfig,
    go_config: &GoConfig,
    common_config: &CommonConfig,
) -> Result<(PathBuf, String)> {
    let mut generate_command = match go_config.go_path.as_ref() {
        Some(path) => process::Command::new(path),
        None => process::Command::new("go"),
    };

    // Change directory into the project directory
    std::env::set_current_dir(&common_config.path)?;
    trace!("Building provider in {:?}", common_config.path);

    // Generate interfaces, if not disabled
    if !go_config.disable_go_generate {
        let result = generate_command
            .args(["generate", "./..."])
            .status()
            .map_err(|e| {
                if e.kind() == ErrorKind::NotFound {
                    anyhow!("{:?} command is not found", generate_command.get_program())
                } else {
                    anyhow!(e)
                }
            })?;

        if !result.success() {
            bail!("Generating interfaces failed: {result}")
        }
    }

    let bin_name = if let Some(bin_name) = &provider_config.bin_name {
        bin_name.to_string()
    } else {
        bail!("Could not infer provider binary name, please specify in wasmcloud.toml under provider.bin_name")
    };

    let mut build_command = match go_config.go_path.as_ref() {
        Some(path) => process::Command::new(path),
        None => process::Command::new("go"),
    };
    // Build for a specified target
    let result = build_command
        .args(["build", "-o", &bin_name])
        .status()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                anyhow!("{:?} command is not found", build_command.get_program())
            } else {
                anyhow!(e)
            }
        })?;

    if !result.success() {
        bail!("Compiling provider failed: {result}")
    }

    Ok((PathBuf::from(&bin_name), bin_name))
}

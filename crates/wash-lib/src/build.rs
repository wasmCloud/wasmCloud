//! Build (and sign) a wasmCloud actor, provider, or interface. Depends on the "cli" feature

use std::{fs, io::ErrorKind, path::PathBuf, process, str::FromStr};

use anyhow::{anyhow, bail, Result};

use crate::cli::{
    claims::{sign_file, ActorMetadata, SignCommand},
    OutputKind,
};
use crate::parser::{
    ActorConfig, CommonConfig, InterfaceConfig, LanguageConfig, ProjectConfig, ProviderConfig,
    RustConfig, TinyGoConfig, TypeConfig,
};

/// Configuration for signing an artifact (actor or provider) including issuer and subject key, the path to where keys can be found, and an option to
/// disable automatic key generation if keys cannot be found.
#[derive(Debug, Clone)]
pub struct SignConfig {
    /// Location of key files for signing
    pub keys_directory: Option<PathBuf>,

    /// Path to issuer seed key (account). If this flag is not provided, the seed will be sourced from ($HOME/.wash/keys) or generated for you if it cannot be found.
    pub issuer: Option<String>,

    /// Path to subject seed key (module or service). If this flag is not provided, the seed will be sourced from ($HOME/.wash/keys) or generated for you if it cannot be found.
    pub subject: Option<String>,

    /// Disables autogeneration of keys if seed(s) are not provided
    pub disable_keygen: bool,
}

/// Using a [ProjectConfig], usually parsed from a `wasmcloud.toml` file, build the project
/// with the installed language toolchain. This will delegate to [build_actor] when the project is an actor,
/// or return an error when trying to build providers or interfaces. This functionality is planned in a future release.
///
/// This function returns the path to the compiled artifact, a signed Wasm module, signed provider archive, or compiled
/// interface library file.
///
/// # Usage
/// ```no_run
/// use wash_lib::{build::build_project, parser::get_config};
/// let config = get_config(None, Some(true))?;
/// let artifact_path = build_project(config)?;
/// println!("Here is the signed artifact: {}", artifact_path.to_string_lossy());
/// ```
/// # Arguments
/// * `config`: [ProjectConfig] for required information to find, build, and sign an actor
/// * `signing`: Optional [SignConfig] with information for signing the project artifact. If omitted, the artifact will only be built
pub fn build_project(config: &ProjectConfig, signing: Option<SignConfig>) -> Result<PathBuf> {
    match &config.project_type {
        TypeConfig::Actor(actor_config) => {
            build_actor(actor_config, &config.language, &config.common, signing)
        }
        TypeConfig::Provider(_provider_config) => Err(anyhow!(
            "wash build has not be implemented for providers yet. Please use `make` for now!"
        )),
        TypeConfig::Interface(_interface_config) => Err(anyhow!(
            "wash build has not be implemented for interfaces yet. Please use `make` for now!"
        )),
    }
}

/// Builds a wasmCloud actor using the installed language toolchain, then signs the actor with
/// keys, capability claims, and additional friendly information like name, version, revision, etc.
///
/// # Arguments
/// * `actor_config`: [ActorConfig] for required information to find, build, and sign an actor
/// * `language_config`: [LanguageConfig] specifying which language the actor is written in
/// * `common_config`: [CommonConfig] specifying common parameters like [CommonConfig::name] and [CommonConfig::version]
/// * `signing`: Optional [SignConfig] with information for signing the actor. If omitted, the actor will only be built
pub fn build_actor(
    actor_config: &ActorConfig,
    language_config: &LanguageConfig,
    common_config: &CommonConfig,
    signing_config: Option<SignConfig>,
) -> Result<PathBuf> {
    // Build actor based on language toolchain
    let file_path = match language_config {
        LanguageConfig::Rust(rust_config) => {
            build_rust_actor(common_config, rust_config, actor_config)
        }
        LanguageConfig::TinyGo(tinygo_config) => build_tinygo_actor(common_config, tinygo_config),
    }?;

    if let Some(config) = signing_config {
        let source = file_path
            .to_str()
            .ok_or_else(|| anyhow!("Could not convert file path to string"))?
            .to_string();

        // Output the signed file in the same directory with a _s suffix
        let destination = source.replace(".wasm", "_s.wasm");
        let destination_file = PathBuf::from_str(&destination)?;

        let sign_options = SignCommand {
            source,
            destination: Some(destination),
            metadata: ActorMetadata {
                name: common_config.name.clone(),
                ver: Some(common_config.version.to_string()),
                custom_caps: actor_config.claims.clone(),
                call_alias: actor_config.call_alias.clone(),
                issuer: config.issuer,
                subject: config.subject,
                ..Default::default()
            },
        };
        sign_file(sign_options, OutputKind::Json)?;

        Ok(destination_file)
    } else {
        // Exit without signing
        Ok(file_path)
    }
}

/// Builds a rust actor and returns the path to the file.
fn build_rust_actor(
    common_config: &CommonConfig,
    rust_config: &RustConfig,
    actor_config: &ActorConfig,
) -> Result<PathBuf> {
    let mut command = match rust_config.cargo_path.as_ref() {
        Some(path) => process::Command::new(path),
        None => process::Command::new("cargo"),
    };

    // Change directory into the project directory
    std::env::set_current_dir(&common_config.path)?;

    let metadata = cargo_metadata::MetadataCommand::new().exec()?;
    let target_path = metadata.target_directory.as_path();

    let result = command.args(["build", "--release"]).status().map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            anyhow!("{:?} command is not found", command.get_program())
        } else {
            anyhow!(e)
        }
    })?;

    if !result.success() {
        bail!("Compiling actor failed: {}", result.to_string())
    }

    // Determine the wasm binary name
    let wasm_bin_name = common_config
        .wasm_bin_name
        .as_ref()
        .unwrap_or(&common_config.name);

    let wasm_file = PathBuf::from(format!(
        "{}/{}/release/{}.wasm",
        rust_config
            .target_path
            .clone()
            .unwrap_or_else(|| PathBuf::from(target_path))
            .to_string_lossy(),
        actor_config.wasm_target,
        wasm_bin_name,
    ));

    if !wasm_file.exists() {
        bail!(
            "Could not find compiled wasm file, please ensure {} exists",
            wasm_file.display()
        );
    }

    // move the file out into the build/ folder for parity with tinygo and convienience for users.
    let copied_wasm_file = PathBuf::from(format!("build/{}.wasm", wasm_bin_name));
    if let Some(p) = copied_wasm_file.parent() {
        fs::create_dir_all(p)?;
    }
    fs::copy(&wasm_file, &copied_wasm_file)?;
    fs::remove_file(&wasm_file)?;

    // Return the full path to the compiled Wasm file
    Ok(common_config.path.join(&copied_wasm_file))
}

/// Builds a tinygo actor and returns the path to the file.
fn build_tinygo_actor(
    common_config: &CommonConfig,
    tinygo_config: &TinyGoConfig,
) -> Result<PathBuf> {
    let filename = format!("build/{}.wasm", common_config.name);

    // Change directory into the project directory
    std::env::set_current_dir(&common_config.path)?;

    let mut command = match &tinygo_config.tinygo_path {
        Some(path) => process::Command::new(path),
        None => process::Command::new("tinygo"),
    };

    if let Some(p) = PathBuf::from(&filename).parent() {
        fs::create_dir_all(p)?;
    }

    let result = command
        .args([
            "build",
            "-o",
            filename.as_str(),
            "-target",
            "wasm",
            "-scheduler",
            "none",
            "-no-debug",
            ".",
        ])
        .status()
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                anyhow!("{:?} command is not found", command.get_program())
            } else {
                anyhow!(e)
            }
        })?;

    if !result.success() {
        bail!("Compiling actor failed: {}", result.to_string())
    }

    let wasm_file = PathBuf::from(filename);

    if !wasm_file.exists() {
        bail!(
            "Could not find compiled wasm file to sign: {}",
            wasm_file.display()
        );
    }

    Ok(common_config.path.join(wasm_file))
}

/// Placeholder for future functionality for building providers
#[allow(unused)]
fn build_provider(
    _provider_config: ProviderConfig,
    _language_config: LanguageConfig,
    _common_config: CommonConfig,
    _no_sign: bool,
) -> Result<()> {
    Ok(())
}

/// Placeholder for future functionality for building interfaces
#[allow(unused)]
fn build_interface(
    _interface_config: InterfaceConfig,
    _language_config: LanguageConfig,
    _common_config: CommonConfig,
) -> Result<()> {
    Ok(())
}

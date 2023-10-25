//! Build (and sign) a wasmCloud actor, provider, or interface. Depends on the "cli" feature

use std::{
    borrow::Cow,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process,
    str::FromStr,
};

use anyhow::{anyhow, bail, Context, Result};
use wasm_encoder::{Encode, Section};
use wit_component::{ComponentEncoder, StringEncoding};

use crate::{
    cli::{
        claims::{sign_file, ActorMetadata, SignCommand},
        OutputKind,
    },
    parser::{
        ActorConfig, CommonConfig, InterfaceConfig, LanguageConfig, ProjectConfig, ProviderConfig,
        RustConfig, TinyGoConfig, TypeConfig, WasmTarget,
    },
};

/// This tag indicates that a Wasm module uses experimental features of wasmCloud
/// and/or the surrounding ecosystem.
///
/// This tag is normally embedded in a Wasm module as a custom section
const WASMCLOUD_WASM_TAG_EXPERIMENTAL: &str = "wasmcloud.com/experimental";

/// Configuration for signing an artifact (actor or provider) including issuer and subject key, the path to where keys can be found, and an option to
/// disable automatic key generation if keys cannot be found.
#[derive(Debug, Clone, Default)]
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
/// * `adapter_bytes`: Optional [&[u8]] bytes that represent a wasm component adapter that should be used, if present.
pub fn build_project(config: &ProjectConfig, signing: Option<SignConfig>) -> Result<PathBuf> {
    match &config.project_type {
        TypeConfig::Actor(actor_config) => {
            build_actor(actor_config, &config.language, &config.common, signing)
        }
        TypeConfig::Provider(_provider_config) => {
            bail!("wash build has not be implemented for providers yet. Please use `make` for now!")
        }
        TypeConfig::Interface(_interface_config) => bail!(
            "wash build has not be implemented for interfaces yet. Please use `make` for now!"
        ),
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
    let actor_wasm_path = match language_config {
        LanguageConfig::Rust(rust_config) => {
            build_rust_actor(common_config, rust_config, actor_config)
        }
        LanguageConfig::TinyGo(tinygo_config) => {
            build_tinygo_actor(common_config, tinygo_config, actor_config)
        }
    }?;

    // Perform embedding, if necessary
    if let WasmTarget::WasiPreview1 | WasmTarget::WasiPreview2 = &actor_config.wasm_target {
        embed_wasm_component_metadata(
            &common_config.path,
            actor_config
                .wit_world
                .as_ref()
                .context("`wit_world` must be specified in wasmcloud.toml for creating preview1 or preview2 components")?,
            &actor_wasm_path,
            &actor_wasm_path,
        )?;
    };

    // If the actor has been configured as WASI Preview2, adapt it from preview1
    if actor_config.wasm_target == WasmTarget::WasiPreview2 {
        let adapter_wasm_bytes = get_wasi_preview2_adapter_bytes(actor_config)?;
        // Adapt the component, using the adapter that is available locally
        let wasm_bytes = adapt_wasi_preview1_component(&actor_wasm_path, adapter_wasm_bytes)
            .with_context(|| {
                format!(
                    "failed to adapt component at [{}] to WASI preview2",
                    actor_wasm_path.display(),
                )
            })?;

        // Write the adapted file out to disk
        fs::write(&actor_wasm_path, wasm_bytes).with_context(|| {
            format!(
                "failed to write WASI preview2 adapted bytes to path [{}]",
                actor_wasm_path.display(),
            )
        })?;
    }

    // Sign the wasm file (if configured)
    if let Some(cfg) = signing_config {
        sign_actor_wasm(common_config, actor_config, cfg, actor_wasm_path)
    } else {
        Ok(actor_wasm_path)
    }
}

/// Sign actor wasm configuration, if necessary
fn sign_actor_wasm(
    common_config: &CommonConfig,
    actor_config: &ActorConfig,
    signing_config: SignConfig,
    actor_wasm_path: impl AsRef<Path>,
) -> Result<PathBuf> {
    // If we're building for WASI preview1 or preview2, we're targeting components-first
    // functionality, and the signed module should be marked as experimental
    let tags: Vec<String> =
        if let WasmTarget::WasiPreview1 | WasmTarget::WasiPreview2 = &actor_config.wasm_target {
            Vec::from([WASMCLOUD_WASM_TAG_EXPERIMENTAL.into()])
        } else {
            Vec::new()
        };

    let source = actor_wasm_path
        .as_ref()
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
            issuer: signing_config.issuer,
            subject: signing_config.subject,
            tags,
            ..Default::default()
        },
    };
    sign_file(sign_options, OutputKind::Json)?;

    Ok(destination_file)
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
    let build_target = rust_config.build_target(&actor_config.wasm_target);

    let result = command
        .args(["build", "--release", "--target", build_target])
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
        build_target,
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
    actor_config: &ActorConfig,
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
            tinygo_config.build_target(&actor_config.wasm_target),
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

/// Adapt a core module/preview1 component to a preview2 wasm component
/// returning the bytes that are the adapted wasm module
fn adapt_wasi_preview1_component(
    wasm_path: impl AsRef<Path>,
    adapter_wasm_bytes: impl AsRef<[u8]>,
) -> Result<Vec<u8>> {
    let wasm_bytes = fs::read(&wasm_path).with_context(|| {
        format!(
            "failed to read wasm file from path [{}]",
            wasm_path.as_ref().display()
        )
    })?;

    // Build a component encoder
    let mut encoder = ComponentEncoder::default()
        .validate(true)
        .module(&wasm_bytes)
        .with_context(|| {
            format!(
                "failed to encode wasm component @ [{}]",
                wasm_path.as_ref().display()
            )
        })?;

    // Adapt the module
    encoder = encoder
        .adapter("wasi_snapshot_preview1", adapter_wasm_bytes.as_ref())
        .context("failed to set adapter during encoding")?;

    // Return the encoded module bytes
    encoder
        .encode()
        .context("failed to serialize encoded component")
}

/// Retrieve bytes for WASI preview2 adapter given a project configuration,
/// if required by project configuration
pub(crate) fn get_wasi_preview2_adapter_bytes(config: &ActorConfig) -> Result<Vec<u8>> {
    if let ActorConfig {
        wasm_target: WasmTarget::WasiPreview2,
        wasi_preview2_adapter_path: Some(path),
        ..
    } = config
    {
        return std::fs::read(path)
            .with_context(|| format!("failed to read wasm bytes from [{}]", path.display()));
    }
    Ok(wasmcloud_component_adapters::WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER.into())
}

/// Embed required component metadata to a given WebAssembly binary
fn embed_wasm_component_metadata(
    project_path: impl AsRef<Path>,
    wit_world: impl AsRef<str>,
    input_wasm: impl AsRef<Path>,
    output_wasm: impl AsRef<Path>,
) -> Result<()> {
    // Find the the WIT directory for the project
    let wit_dir = project_path.as_ref().join("wit");
    if !wit_dir.is_dir() {
        bail!(
            "expected 'wit' directory under project path at [{}] is missing",
            wit_dir.display()
        );
    };

    // Resolve the WIT directory packages & worlds
    let mut resolve = wit_parser::Resolve::default();
    let (package_id, _paths) = resolve
        .push_dir(&wit_dir)
        .context("failed to add WIT deps directory")?;
    log::info!("successfully loaded WIT @ [{}]", wit_dir.display());

    // Select the target world that was specified by the user
    let world = resolve
        .select_world(package_id, wit_world.as_ref().into())
        .context("failed to select world from built resolver")?;

    // Encode the metadata
    let encoded_metadata =
        wit_component::metadata::encode(&resolve, world, StringEncoding::UTF8, None)
            .context("failed to encode WIT metadata for component")?;

    // Load the wasm binary
    let mut wasm_bytes = wat::parse_file(input_wasm.as_ref()).with_context(|| {
        format!(
            "failed to read wasm bytes from [{}]",
            input_wasm.as_ref().display()
        )
    })?;

    // Build & encode a custom sections at the end of the Wasm module
    let custom_sections = [
        wasm_encoder::CustomSection {
            name: "component-type".into(),
            data: Cow::Borrowed(&encoded_metadata),
        },
        wasm_encoder::CustomSection {
            name: WASMCLOUD_WASM_TAG_EXPERIMENTAL.into(),
            data: Cow::Borrowed(b"true"),
        },
    ];
    for section in custom_sections {
        wasm_bytes.push(section.id());
        section.encode(&mut wasm_bytes);
        log::debug!(
            "successfully embedded component metadata section [{}] in WASM",
            section.name
        );
    }

    // Output the WASM to disk (possibly overwriting the original path)
    std::fs::write(output_wasm.as_ref(), wasm_bytes).with_context(|| {
        format!(
            "failed to write updated wasm to disk at [{}]",
            output_wasm.as_ref().display()
        )
    })?;

    log::info!(
        "successfully wrote component w/ metadata to [{}]",
        output_wasm.as_ref().display()
    );

    Ok(())
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use anyhow::{Context, Result};
    use semver::Version;
    use wascap::{jwt::Token, wasm::extract_claims};
    use wasmparser::{Parser, Payload};

    use crate::{
        build::{embed_wasm_component_metadata, WASMCLOUD_WASM_TAG_EXPERIMENTAL},
        parser::{ActorConfig, CommonConfig, WasmTarget},
    };

    use super::{sign_actor_wasm, SignConfig};

    const MODULE_WAT: &str = "(module)";
    const COMPONENT_WORLD_WIT: &str = r#"
package washlib:test;

interface foo {
    bar: func() -> string;
}

world test-world {
   import foo;
}
"#;

    /// Set up a component that should be built
    ///
    /// This function returns the path to a mock project directory
    /// which includes a `test.wasm` file along with a `wit` directory
    fn setup_build_component(base_dir: impl AsRef<Path>) -> Result<PathBuf> {
        // Write the test wit world
        let wit_dir = base_dir.as_ref().join("wit");
        fs::create_dir_all(&wit_dir)?;
        fs::write(wit_dir.join("world.wit"), COMPONENT_WORLD_WIT)?;

        // Write and build the wasm module itself
        let wasm_path = base_dir.as_ref().join("test.wasm");
        fs::write(&wasm_path, wat::parse_str(MODULE_WAT)?)?;

        Ok(wasm_path)
    }

    /// Ensure that components which get component metadata embedded into them
    /// contain the right experimental tags in Wasm custom sections
    #[test]
    fn embed_wasm_component_metadata_includes_experimental() -> Result<()> {
        // Build project path, including WIT dir
        let project_dir = tempfile::tempdir()?;
        let wasm_path = setup_build_component(&project_dir)?;

        // Embed component metadata into the wasm module, to build a component
        embed_wasm_component_metadata(&project_dir, "test-world", &wasm_path, &wasm_path)
            .context("failed to embed wasm component metadata")?;

        let wasm_bytes = fs::read(&wasm_path)
            .with_context(|| format!("failed to read test wasm @ [{}]", wasm_path.display()))?;

        // Check that the Wasm module contains the custom section indicating experimental behavior
        assert!(Parser::default()
            .parse_all(&wasm_bytes)
            .any(|payload| match payload {
                Ok(Payload::CustomSection(cs_reader))
                    if cs_reader.name() == WASMCLOUD_WASM_TAG_EXPERIMENTAL
                        && cs_reader.data() == b"true" =>
                    true,
                _ => false,
            }));

        Ok(())
    }

    /// Ensure that components which get signed contain the right experimental tags
    /// in claims when preview1 or preview2 targets are signed
    #[test]
    fn sign_actor_component_includes_experimental() -> Result<()> {
        // Build project path, including WIT dir
        let project_dir = tempfile::tempdir()?;
        let wasm_path = setup_build_component(&project_dir)?;

        // Check targets that should have experimental tag set
        for wasm_target in [
            WasmTarget::CoreModule,
            WasmTarget::WasiPreview1,
            WasmTarget::WasiPreview2,
        ] {
            let updated_wasm_path = sign_actor_wasm(
                &CommonConfig {
                    name: "test".into(),
                    version: Version::parse("0.1.0")?,
                    path: project_dir.path().into(),
                    wasm_bin_name: Some("test.wasm".into()),
                },
                &ActorConfig {
                    wasm_target: wasm_target.clone(),
                    wit_world: Some("test".into()),
                    ..ActorConfig::default()
                },
                SignConfig::default(),
                &wasm_path,
            )?;

            // Check that the experimental tag is present
            let Token { claims, .. } = extract_claims(
                fs::read(updated_wasm_path).context("failed to read updated wasm")?,
            )?
            .context("failed to extract claims")?;

            // Check wasm targets
            match wasm_target {
                WasmTarget::CoreModule => assert!(
                    claims
                        .metadata
                        .context("failed to get claim metadata")?
                        .tags
                        .context("failed to get tags")?
                        .iter()
                        .all(|t| t != WASMCLOUD_WASM_TAG_EXPERIMENTAL),
                    "experimental tag should not be present on core modules",
                ),
                WasmTarget::WasiPreview1 | WasmTarget::WasiPreview2 => assert!(
                    claims
                        .metadata
                        .context("failed to get claim metadata")?
                        .tags
                        .context("failed to get tags")?
                        .iter()
                        .any(|t| t == WASMCLOUD_WASM_TAG_EXPERIMENTAL),
                    "experimental tag should be present on preview1/preview2 components"
                ),
            }
        }

        Ok(())
    }
}

use std::{
    borrow::Cow,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{self},
};

use anyhow::{anyhow, bail, Context, Result};
use normpath::PathExt;
use tracing::{debug, info, warn};
use wasm_encoder::{Encode, Section};
use wit_bindgen_core::Files;
use wit_bindgen_go::Opts as WitBindgenGoOpts;
use wit_component::{ComponentEncoder, StringEncoding};

use crate::{
    build::{convert_wit_dir_to_world, SignConfig, WASMCLOUD_WASM_TAG_EXPERIMENTAL},
    cli::{
        claims::{sign_file, ComponentMetadata, GenerateCommon, SignCommand},
        OutputKind,
    },
    parser::{CommonConfig, ComponentConfig, LanguageConfig, RustConfig, TinyGoConfig, WasmTarget},
};

/// Builds a wasmCloud component using the installed language toolchain, then signs the component with
/// keys, capability claims, and additional friendly information like name, version, revision, etc.
///
/// # Arguments
/// * `component_config`: [`ComponentConfig`] for required information to find, build, and sign an component
/// * `language_config`: [`LanguageConfig`] specifying which language the component is written in
/// * `common_config`: [`CommonConfig`] specifying common parameters like [`CommonConfig::name`] and [`CommonConfig::version`]
/// * `signing`: Optional [`SignConfig`] with information for signing the component. If omitted, the component will only be built
pub fn build_component(
    component_config: &ComponentConfig,
    language_config: &LanguageConfig,
    common_config: &CommonConfig,
    signing_config: Option<&SignConfig>,
) -> Result<PathBuf> {
    let component_wasm_path = if let Some(raw_command) = component_config.build_command.as_ref() {
        build_custom_component(common_config, component_config, raw_command)?
    } else {
        // Build component based on language toolchain
        let component_wasm_path = match language_config {
            LanguageConfig::Rust(rust_config) => {
                build_rust_component(common_config, rust_config, component_config)?
            }
            LanguageConfig::TinyGo(tinygo_config) => {
                let component_wasm_path =
                    build_tinygo_component(common_config, tinygo_config, component_config)?;

                // Perform embedding, if necessary
                if let WasmTarget::WasiPreview1 | WasmTarget::WasiPreview2 =
                    &component_config.wasm_target
                {
                    embed_wasm_component_metadata(
                        &common_config.path,
                        component_config
                        .wit_world
                        .as_ref()
                        .context("missing `wit_world` in wasmcloud.toml ([component] section) for creating preview1 or preview2 components")?,
                        &component_wasm_path,
                        &component_wasm_path,
                )?;
                };
                component_wasm_path
            }
            LanguageConfig::Go(_) | LanguageConfig::Other(_)
                if component_config.build_command.is_some() =>
            {
                // SAFETY: We checked that the build command is not None above
                build_custom_component(
                    common_config,
                    component_config,
                    component_config.build_command.as_ref().unwrap(),
                )?
            }
            LanguageConfig::Go(_) => {
                bail!("build command is required for unsupported language go");
            }
            LanguageConfig::Other(other) => {
                bail!("build command is required for unsupported language {other}");
            }
        };

        // If the component has been configured as WASI Preview2, adapt it from preview1
        if component_config.wasm_target == WasmTarget::WasiPreview2 {
            let adapter_wasm_bytes = get_wasi_preview2_adapter_bytes(component_config)?;
            // Adapt the component, using the adapter that is available locally
            let wasm_bytes = adapt_wasi_preview1_component(&component_wasm_path, adapter_wasm_bytes)
                .with_context(|| {
                    format!(
                        "failed to adapt component at [{}] to WASI preview2",
                        component_wasm_path.display(),
                    )
                })?;

            // Write the adapted file out to disk
            fs::write(&component_wasm_path, wasm_bytes).with_context(|| {
                format!(
                    "failed to write WASI preview2 adapted bytes to path [{}]",
                    component_wasm_path.display(),
                )
            })?;
        }
        component_wasm_path
    };

    // Sign the wasm file (if configured)
    if let Some(cfg) = signing_config {
        sign_component_wasm(common_config, component_config, cfg, component_wasm_path)
    } else {
        Ok(component_wasm_path)
    }
}

/// Sign the component at `component_wasm_path` using the provided configuration
pub fn sign_component_wasm(
    common_config: &CommonConfig,
    component_config: &ComponentConfig,
    signing_config: &SignConfig,
    component_wasm_path: impl AsRef<Path>,
) -> Result<PathBuf> {
    // If we're building for WASI preview1 or preview2, we're targeting components-first
    // functionality, and the signed module should be marked as experimental
    let mut tags = component_config.tags.clone().unwrap_or_default();
    if let WasmTarget::WasiPreview1 | WasmTarget::WasiPreview2 = &component_config.wasm_target {
        tags.insert(WASMCLOUD_WASM_TAG_EXPERIMENTAL.into());
    };

    let source = component_wasm_path
        .as_ref()
        .to_str()
        .ok_or_else(|| anyhow!("Could not convert file path to string"))?
        .to_string();

    // Output the signed file in the same directory with a _s suffix
    let destination = if let Some(destination) = component_config.destination.clone() {
        destination
    } else {
        PathBuf::from(source.replace(".wasm", "_s.wasm"))
    };

    let sign_options = SignCommand {
        source,
        destination: Some(destination.to_string_lossy().to_string()),
        metadata: ComponentMetadata {
            name: Some(common_config.name.clone()),
            ver: Some(common_config.version.to_string()),
            rev: Some(common_config.revision),
            call_alias: component_config.call_alias.clone(),
            issuer: signing_config.issuer.clone(),
            subject: signing_config.subject.clone(),
            common: GenerateCommon {
                disable_keygen: signing_config.disable_keygen,
                directory: signing_config.keys_directory.clone(),
                ..Default::default()
            },
            tags: tags.into_iter().collect(),
        },
    };
    sign_file(sign_options, OutputKind::Json)?;

    Ok(if destination.is_absolute() {
        destination
    } else {
        common_config.path.join(destination)
    })
}

/// Builds a rust component and returns the path to the file.
fn build_rust_component(
    common_config: &CommonConfig,
    rust_config: &RustConfig,
    component_config: &ComponentConfig,
) -> Result<PathBuf> {
    let mut command = match rust_config.cargo_path.as_ref() {
        Some(path) => process::Command::new(path),
        None => process::Command::new("cargo"),
    };

    // Change directory into the project directory
    std::env::set_current_dir(&common_config.path)?;

    let build_target: &str = rust_config.build_target(&component_config.wasm_target);
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
        bail!("Compiling component failed: {}", result.to_string())
    }

    // Determine the wasm binary name
    let wasm_bin_name = common_config
        .wasm_bin_name
        .as_ref()
        .unwrap_or(&common_config.name);

    // NOTE: Windows paths are tricky.
    // We're using a third-party library normpath to ensure that the paths are normalized.
    // Once out of nightly, we should be able to use std::path::absolute
    // https://github.com/rust-lang/rust/pull/91673
    let metadata = cargo_metadata::MetadataCommand::new().exec()?;
    let mut wasm_path_buf = rust_config
        .target_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(metadata.target_directory.as_std_path()));
    wasm_path_buf.push(build_target);
    wasm_path_buf.push("release");
    wasm_path_buf.push(format!("{wasm_bin_name}.wasm"));

    // Ensure the file exists, normalize uses the fs and file must exist
    let wasm_file = match wasm_path_buf.normalize() {
        Ok(p) => p,
        Err(e) => bail!(
            "Could not find compiled wasm file, please ensure {:?} exists. Error: {:?}",
            wasm_path_buf,
            e
        ),
    };

    // move the file out into the build/ folder for parity with tinygo and convienience for users.
    let copied_wasm_file = PathBuf::from(format!("build/{wasm_bin_name}.wasm"));
    if let Some(p) = copied_wasm_file.parent() {
        fs::create_dir_all(p)?;
    }
    fs::copy(&wasm_file, &copied_wasm_file)?;
    fs::remove_file(&wasm_file)?;

    // Return the full path to the compiled Wasm file
    Ok(common_config.path.join(&copied_wasm_file))
}

/// Builds a tinygo component and returns the path to the file.
fn build_tinygo_component(
    common_config: &CommonConfig,
    tinygo_config: &TinyGoConfig,
    component_config: &ComponentConfig,
) -> Result<PathBuf> {
    let filename = format!("build/{}.wasm", common_config.name);
    let file_path = PathBuf::from(&filename);

    // Change directory into the project directory
    std::env::set_current_dir(&common_config.path)?;

    let mut command = match &tinygo_config.tinygo_path {
        Some(path) => process::Command::new(path),
        None => process::Command::new("tinygo"),
    };

    // Ensure the target directory which will contain the eventual filename exists
    // this usually means creating the build folder in the golang project root
    let parent_dir = file_path.parent().unwrap_or(&common_config.path);
    if !parent_dir.exists() {
        fs::create_dir_all(parent_dir)?;
    }

    // Ensure the output directory exists
    let output_dir = common_config.path.join(GOLANG_BINDGEN_FOLDER_NAME);
    if !output_dir.exists() {
        fs::create_dir_all(&output_dir)?;
    }

    // Generate wit-bindgen code for Golang components which are components-first
    //
    // Running wit-bindgen via go generate is only for WIT-enabled projects, so we must limit
    // to only projects that have their WIT defined in the expected top level wit directory
    //
    // While wasmcloud and its tooling is WIT-first, it is possible to build preview1/preview2
    // components that are *not* WIT enabled. To determine whether the project is WIT-enabled
    // we check for the `wit` directory which would be passed through to bindgen.
    if component_config.wit_world.is_some() && !tinygo_config.disable_go_generate {
        generate_tinygo_bindgen(
            &output_dir,
            common_config.path.join("wit"),
            component_config.wit_world.as_ref().context(
                "missing `wit_world` in wasmcloud.toml ([component] section) to run go bindgen generate",
            )?,
        )
                .context("generating golang bindgen code failed")?;
    }

    let result = command
        .args([
            "build",
            "-o",
            filename.as_str(),
            "-target",
            tinygo_config.build_target(&component_config.wasm_target),
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
        bail!("Compiling component failed: {}", result.to_string())
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

/// Builds a wasmCloud component using a custom override command, then returns the path to the file.
fn build_custom_component(
    common_config: &CommonConfig,
    component_config: &ComponentConfig,
    raw_command: &str,
) -> Result<PathBuf> {
    // Change directory into the project directory
    std::env::set_current_dir(&common_config.path)?;
    let (command, args) = parse_custom_command(raw_command)?;
    let mut command = process::Command::new(command);
    // All remaining elements of the split command are interpreted as arguments
    command
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = command.output().map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            anyhow!("`{:?}` was not found", command.get_program())
        } else {
            anyhow!(format!("failed to run `{:?}`: {e}", command.get_program()))
        }
    })?;
    if !output.status.success() {
        bail!(
            "failed to build component with custom command: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let component_path = component_config
        .build_artifact
        .clone()
        .map(|p| {
            // This outputs the path if it's absolute, or joins it with the project path if it's relative
            if p.is_absolute() {
                p
            } else {
                common_config.path.join(p)
            }
        })
        .unwrap_or_else(|| {
            common_config
                .path
                .join(format!("build/{}.wasm", common_config.wasm_bin_name()))
        });
    if std::fs::metadata(component_path.as_path()).is_err() {
        warn!(
            "Component built with custom command but not found in expected path [{}]",
            component_path.display()
        );
    }
    Ok(component_path)
}

/// The folder that golang bindgen code will be placed in, normally
/// from the top level golang project directory
const GOLANG_BINDGEN_FOLDER_NAME: &str = "gen";

/// Generate the bindgen code that `TinyGo` components need
fn generate_tinygo_bindgen(
    bindgen_dir: impl AsRef<Path>,
    wit_dir: impl AsRef<Path>,
    wit_world: impl AsRef<str>,
) -> Result<()> {
    if !bindgen_dir.as_ref().exists() {
        bail!(
            "bindgen directory @ [{}] does not exist",
            bindgen_dir.as_ref().display(),
        );
    }

    if !wit_dir.as_ref().exists() {
        bail!(
            "top level WIT directory @ [{}] does not exist",
            wit_dir.as_ref().display(),
        );
    }

    // Resolve the wit world
    let (resolver, world_id) = convert_wit_dir_to_world(wit_dir, wit_world)?;

    // Build the golang wit-bindgen generator
    let mut generator = WitBindgenGoOpts::default().build();

    // Run the generator
    let mut files = Files::default();
    generator
        .generate(&resolver, world_id, &mut files)
        .context("failed to run golang wit-bindgen generator")?;
    info!("successfully ran golang wit-bindgen generator");

    // Write all generated files to disk
    for (file_name, content) in files.iter() {
        let full_path = bindgen_dir.as_ref().join(file_name);

        // Ensure the parent directory path is created
        if let Some(parent_path) = PathBuf::from(&full_path).parent() {
            if !parent_path.exists() {
                fs::create_dir_all(parent_path).with_context(|| {
                    format!("failed to create dir for path [{}]", parent_path.display())
                })?;
            }
        }

        // Write out the file's contents to disk
        fs::write(&full_path, content).with_context(|| {
            format!(
                "failed to write content for file @ path [{}]",
                full_path.display()
            )
        })?;
    }
    info!(
        "successfully wrote wit-bindgen generated golang files to [{}]",
        bindgen_dir.as_ref().display()
    );

    Ok(())
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
pub(crate) fn get_wasi_preview2_adapter_bytes(config: &ComponentConfig) -> Result<Vec<u8>> {
    if let ComponentConfig {
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

    let (resolver, world_id) =
        convert_wit_dir_to_world(wit_dir, wit_world).context("failed to resolve WIT world")?;

    // Encode the metadata
    let encoded_metadata =
        wit_component::metadata::encode(&resolver, world_id, StringEncoding::UTF8, None)
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
        debug!(
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

    info!(
        "successfully wrote component w/ metadata to [{}]",
        output_wasm.as_ref().display()
    );

    Ok(())
}

fn parse_custom_command(command: &str) -> Result<(&str, Vec<&str>)> {
    let mut split_command = command.split_ascii_whitespace();
    let command = split_command
        .next()
        .context("build command is supplied but empty")?;
    let args = split_command.collect::<Vec<_>>();
    Ok((command, args))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::fs::DirEntry;
    use std::path::Path;
    use std::path::PathBuf;

    use anyhow::{Context, Result};
    use semver::Version;
    use wascap::{jwt::Token, wasm::extract_claims};
    use wasmparser::{Parser, Payload};

    use crate::parser::RegistryConfig;
    use crate::{
        build::WASMCLOUD_WASM_TAG_EXPERIMENTAL,
        parser::{CommonConfig, ComponentConfig, WasmTarget},
    };

    use super::{
        embed_wasm_component_metadata, generate_tinygo_bindgen, sign_component_wasm, SignConfig,
    };

    const MODULE_WAT: &str = "(module)";
    const COMPONENT_BASIC_WIT: &str = r"
package washlib:test;

interface foo {
    bar: func() -> string;
}

world test-world {
   import foo;
}
";
    const EXPECTED_COMPONENT_BASIC_GOLANG_FILES: [&str; 3] =
        ["test_world.h", "test_world.c", "test_world.go"];

    const COMPONENT_UPSTREAM_WIT: &str = r"
package washlib:multi;

interface foo {
    bar: func() -> string;
}

world upstream {
   import foo;
}
";

    const COMPONENT_DOWNSTREAM_WIT: &str = r"
package washlib:multi;

interface bar {
    baz: func() -> string;
}

world downstream {
   include upstream;
   import bar;
}
";
    const EXPECTED_COMPONENT_DOWNSTREAM_GOLANG_FILES: [&str; 3] =
        ["downstream.h", "downstream.c", "downstream.go"];

    /// Set up a component that should be built
    ///
    /// This function returns the path to a mock project directory
    /// which includes a `test.wasm` file along with a `wit` directory
    fn setup_build_component(base_dir: impl AsRef<Path>) -> Result<PathBuf> {
        // Write the test wit world
        let wit_dir = base_dir.as_ref().join("wit");
        fs::create_dir_all(&wit_dir)?;
        fs::write(wit_dir.join("world.wit"), COMPONENT_BASIC_WIT)?;

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
            .any(|payload| matches!(payload,
                Ok(Payload::CustomSection(cs_reader))
                    if cs_reader.name() == WASMCLOUD_WASM_TAG_EXPERIMENTAL
                        && cs_reader.data() == b"true"
            )));

        Ok(())
    }

    /// Ensure that components which get signed contain any tags specified
    /// *and* experimental tag in claims when preview1 or preview2 targets are signed
    #[test]
    fn sign_component_includes_experimental() -> Result<()> {
        // Build project path, including WIT dir
        let project_dir = tempfile::tempdir()?;
        let wasm_path = setup_build_component(&project_dir)?;

        // Check targets that should have experimental tag set
        for wasm_target in [
            WasmTarget::CoreModule,
            WasmTarget::WasiPreview1,
            WasmTarget::WasiPreview2,
        ] {
            let updated_wasm_path = sign_component_wasm(
                &CommonConfig {
                    name: "test".into(),
                    version: Version::parse("0.1.0")?,
                    revision: 0,
                    path: project_dir.path().into(),
                    wasm_bin_name: Some("test.wasm".into()),
                    registry: RegistryConfig::default(),
                },
                &ComponentConfig {
                    wasm_target: wasm_target.clone(),
                    wit_world: Some("test".into()),
                    tags: Some(HashSet::from(["test-tag".into()])),
                    ..ComponentConfig::default()
                },
                &SignConfig::default(),
                &wasm_path,
            )?;

            // Check that the experimental tag is present
            let Token { claims, .. } = extract_claims(
                fs::read(updated_wasm_path).context("failed to read updated wasm")?,
            )?
            .context("failed to extract claims")?;

            // Check wasm targets
            let tags = claims
                .metadata
                .context("failed to get claim metadata")?
                .tags
                .context("missing tags")?;
            assert!(
                tags.contains(&String::from("test-tag")),
                "test-tag should be present"
            );

            match wasm_target {
                WasmTarget::CoreModule => assert!(
                    !tags.contains(&String::from(WASMCLOUD_WASM_TAG_EXPERIMENTAL)),
                    "experimental tag should not be present on core modules"
                ),
                WasmTarget::WasiPreview1 | WasmTarget::WasiPreview2 => assert!(
                    tags.contains(&String::from(WASMCLOUD_WASM_TAG_EXPERIMENTAL)),
                    "experimental tag should be present on preview1/preview2 components"
                ),
            }
        }

        Ok(())
    }

    /// Ensure that golang component generation works with a bindgen'd component
    #[test]
    fn golang_generate_bindgen_component_basic() -> Result<()> {
        let project_dir = tempfile::tempdir()?;

        // Set up directories
        let wit_dir = project_dir.path().join("wit");
        let output_dir = project_dir.path().join("generated");
        std::fs::create_dir(&wit_dir).context("failed to create WIT dir")?;
        std::fs::create_dir(&output_dir).context("failed to create output dir")?;

        // Write WIT for Golang code
        std::fs::write(wit_dir.join("test.wit"), COMPONENT_BASIC_WIT)
            .context("failed to write test WIT file")?;

        // Run bindgen generation process
        generate_tinygo_bindgen(&output_dir, &wit_dir, "test-world")
            .context("failed to run tinygo bindgen")?;

        let dir_contents = fs::read_dir(output_dir)
            .context("failed to read dir")?
            .collect::<Result<Vec<DirEntry>, std::io::Error>>()?;

        assert!(!dir_contents.is_empty(), "files were generated");
        assert!(
            EXPECTED_COMPONENT_BASIC_GOLANG_FILES.iter().all(|f| {
                dir_contents.iter().any(|de| {
                    de.path()
                        .file_name()
                        .is_some_and(|v| v.to_string_lossy() == **f)
                })
            }),
            "expected bindgen go files are present"
        );

        Ok(())
    }

    /// Ensure that golang component generation works with a bindgen'd component
    /// which has multiple worlds
    #[test]
    fn golang_generate_bindgen_component_multi_world() -> Result<()> {
        let project_dir = tempfile::tempdir()?;

        // Set up directories
        let wit_dir = project_dir.path().join("wit");
        let output_dir = project_dir.path().join("generated");
        std::fs::create_dir(&wit_dir).context("failed to create WIT dir")?;
        std::fs::create_dir(&output_dir).context("failed to create output dir")?;

        // Write WIT for Golang code
        std::fs::write(wit_dir.join("upstream.wit"), COMPONENT_UPSTREAM_WIT)
            .context("failed to write test WIT file")?;
        std::fs::write(wit_dir.join("downstream.wit"), COMPONENT_DOWNSTREAM_WIT)
            .context("failed to write test WIT file")?;

        // Run bindgen generation process
        generate_tinygo_bindgen(&output_dir, &wit_dir, "downstream")
            .context("failed to run tinygo bindgen")?;

        let dir_contents = fs::read_dir(output_dir)
            .context("failed to read dir")?
            .collect::<Result<Vec<DirEntry>, std::io::Error>>()?;

        assert!(!dir_contents.is_empty(), "files were generated");
        assert!(
            EXPECTED_COMPONENT_DOWNSTREAM_GOLANG_FILES.iter().all(|f| {
                dir_contents.iter().any(|de| {
                    de.path()
                        .file_name()
                        .is_some_and(|v| v.to_string_lossy() == **f)
                })
            }),
            "expected bindgen go files are present"
        );

        Ok(())
    }

    #[test]
    fn can_parse_custom_command() {
        let cargo_component_build = "cargo component build --release --target wasm32-wasi";
        let tinygo_build =
            "tinygo build -o build/test.wasm -target wasm32-wasi -scheduler none -no-debug .";
        // Raw strings because backslashes are used and shouldn't trigger escape sequences
        let some_other_language = r"zig build-exe .\tiny-hello.zig -O ReleaseSmall -fstrip -fsingle-threaded -target aarch64-linux";

        let (command, args) = super::parse_custom_command(cargo_component_build)
            .expect("should be able to parse cargo command");
        assert_eq!(command, "cargo");
        assert_eq!(
            args,
            vec!["component", "build", "--release", "--target", "wasm32-wasi"]
        );

        let (command, args) = super::parse_custom_command(tinygo_build)
            .expect("should be able to parse tinygo command");
        assert_eq!(command, "tinygo");
        assert_eq!(
            args,
            vec![
                "build",
                "-o",
                "build/test.wasm",
                "-target",
                "wasm32-wasi",
                "-scheduler",
                "none",
                "-no-debug",
                "."
            ]
        );

        let (command, args) = super::parse_custom_command(some_other_language)
            .expect("should be able to parse some other language command");
        assert_eq!(command, "zig");
        assert_eq!(
            args,
            vec![
                "build-exe",
                r".\tiny-hello.zig",
                "-O",
                "ReleaseSmall",
                "-fstrip",
                "-fsingle-threaded",
                "-target",
                "aarch64-linux"
            ]
        );
    }
}

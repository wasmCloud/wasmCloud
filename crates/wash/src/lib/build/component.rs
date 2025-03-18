use std::{
    borrow::Cow,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use normpath::PathExt;
use tracing::{debug, info, warn};
use wasi_preview1_component_adapter_provider::{
    WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME, WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
};
use wasm_encoder::{Encode, Section};
use wit_component::{ComponentEncoder, StringEncoding};

use crate::lib::{
    build::{convert_wit_dir_to_world, SignConfig, WASMCLOUD_WASM_TAG_EXPERIMENTAL},
    cli::{
        claims::{sign_file, ComponentMetadata, GenerateCommon, SignCommand},
        OutputKind,
    },
    parser::{CommonConfig, ComponentConfig, LanguageConfig, RustConfig, TinyGoConfig, WasmTarget},
};

/// Builds a wasmCloud component using the installed language toolchain, then signs the component
/// with keys, capability claims, and additional friendly information like name, version, revision,
/// etc.
///
/// # Arguments
/// * `component_config`: [`ComponentConfig`] for required information to find, build, and sign a
///   component
/// * `language_config`: [`LanguageConfig`] specifying which language the component is written in
/// * `common_config`: [`CommonConfig`] specifying common parameters like [`CommonConfig::name`] and
///   [`CommonConfig::version`]
/// * `signing`: Optional [`SignConfig`] with information for signing the component. If omitted, the
///   component will only be built
/// * `package_args`: Optional overrides for loading wasm packages
pub async fn build_component(
    component_config: &ComponentConfig,
    language_config: &LanguageConfig,
    common_config: &CommonConfig,
    signing_config: Option<&SignConfig>,
) -> Result<PathBuf> {
    // Build component
    let component_wasm_path = if let Some(raw_command) = component_config.build_command.as_ref() {
        build_custom_component(common_config, component_config, raw_command).await?
    } else {
        // Build component based on language toolchain
        match language_config {
            LanguageConfig::Rust(rust_config) => {
                let rust_wasm_path =
                    build_rust_component(common_config, rust_config, component_config).await?;
                match component_config.wasm_target {
                    WasmTarget::CoreModule | WasmTarget::WasiP1 => {
                        adapt_component_to_wasip2(&rust_wasm_path, component_config)?
                    }
                    WasmTarget::WasiP2 => rust_wasm_path,
                }
            }
            LanguageConfig::TinyGo(tinygo_config) => {
                let go_wasm_path =
                    build_tinygo_component(common_config, tinygo_config, component_config).await?;

                match component_config.wasm_target {
                    // NOTE(lxf): historically, wasip1 was being adapted to p2 which is different from rust target.
                    // We continue to do so here.
                    WasmTarget::CoreModule | WasmTarget::WasiP1 => {
                        embed_wasm_component_metadata(
                            &common_config.wit_dir,
                            component_config
                            .wit_world
                            .as_ref()
                            .context("missing `wit_world` in wasmcloud.toml ([component] section) for creating preview1 or preview2 components")?,
                            &go_wasm_path,
                            &go_wasm_path,
                        )?;
                        adapt_component_to_wasip2(&go_wasm_path, component_config)?
                    }
                    WasmTarget::WasiP2 => {
                        // NOTE(lxf): tinygo takes over wit world embedding for wasip2 target
                        go_wasm_path
                    }
                }
            }
            LanguageConfig::Go(_) | LanguageConfig::Other(_)
                if component_config.build_command.is_some() =>
            {
                // SAFETY: We checked that the build command is not None above
                build_custom_component(
                    common_config,
                    component_config,
                    component_config.build_command.as_ref().unwrap(),
                )
                .await?
            }
            LanguageConfig::Go(_) => {
                bail!("build command is required for unsupported language go");
            }
            LanguageConfig::Other(other) => {
                bail!("build command is required for unsupported language {other}");
            }
        }
    };

    let component_wasm_path = if component_wasm_path.is_relative() {
        tokio::fs::canonicalize(component_wasm_path.join(&common_config.project_dir))
            .await
            .context("failed to canonicalize component path")?
    } else {
        component_wasm_path
    };

    // Sign the wasm file (if configured)
    if let Some(cfg) = signing_config {
        sign_component_wasm(common_config, component_config, cfg, component_wasm_path)
    } else {
        Ok(component_wasm_path)
    }
}

pub(crate) fn adapt_component_to_wasip2(
    component_wasm_path: impl AsRef<Path>,
    component_config: &ComponentConfig,
) -> Result<PathBuf> {
    let adapted_wasm_path = component_wasm_path.as_ref();
    let adapter_wasm_bytes = get_wasip2_adapter_bytes(component_config)?;
    let wasm_bytes =
        adapt_wasip1_component(adapted_wasm_path, adapter_wasm_bytes).with_context(|| {
            format!(
                "failed to adapt component at [{}] to WASIP2",
                adapted_wasm_path.display(),
            )
        })?;
    fs::write(adapted_wasm_path, wasm_bytes).with_context(|| {
        format!(
            "failed to write WASIP2 adapted bytes to path [{}]",
            adapted_wasm_path.display(),
        )
    })?;
    Ok(adapted_wasm_path.to_path_buf())
}

/// Sign the component at `component_wasm_path` using the provided configuration
pub fn sign_component_wasm(
    common_config: &CommonConfig,
    component_config: &ComponentConfig,
    signing_config: &SignConfig,
    component_wasm_path: impl AsRef<Path>,
) -> Result<PathBuf> {
    // If we're building for WASIP1 or WASIP2, we're targeting components-first
    // functionality, and the signed module should be marked as experimental
    let mut tags = component_config.tags.clone().unwrap_or_default();
    if let WasmTarget::WasiP1 | WasmTarget::WasiP2 = &component_config.wasm_target {
        tags.insert(WASMCLOUD_WASM_TAG_EXPERIMENTAL.into());
    };

    let source = component_wasm_path
        .as_ref()
        .to_str()
        .ok_or_else(|| anyhow!("Could not convert file path to string"))?
        .to_string();

    let destination = if let Some(destination) = component_config.destination.clone() {
        if destination.is_relative() {
            common_config.project_dir.join(destination)
        } else {
            destination
        }
    } else {
        // Output the signed file in the same directory with a _s suffix by default
        PathBuf::from(source.replace(".wasm", "_s.wasm"))
    };

    let sign_options = SignCommand {
        source,
        destination: Some(destination.to_string_lossy().to_string()),
        metadata: ComponentMetadata {
            name: Some(common_config.name.clone()),
            ver: Some(common_config.version.to_string()),
            rev: Some(common_config.revision),
            call_alias: None,
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

    Ok(destination)
}

/// Builds a rust component and returns the path to the file.
async fn build_rust_component(
    common_config: &CommonConfig,
    rust_config: &RustConfig,
    component_config: &ComponentConfig,
) -> Result<PathBuf> {
    let mut command = match rust_config.cargo_path.as_ref() {
        Some(path) => tokio::process::Command::new(path),
        None => tokio::process::Command::new("cargo"),
    };

    // Build for a specified target if provided, or the default rust target
    let mut build_args = vec!["build"];

    if !rust_config.debug {
        build_args.push("--release");
    }

    let build_target: &str = rust_config.build_target(&component_config.wasm_target);
    build_args.extend_from_slice(&["--target", build_target]);
    let result = command
        .current_dir(&common_config.project_dir)
        .args(build_args)
        .status()
        .await
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                anyhow!("{:?} command is not found", command.as_std().get_program())
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
    let metadata = cargo_metadata::MetadataCommand::new()
        .current_dir(&common_config.project_dir)
        .exec()?;
    let mut wasm_path_buf = rust_config
        .target_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(metadata.target_directory.as_std_path()));
    wasm_path_buf.push(build_target);
    if rust_config.debug {
        wasm_path_buf.push("debug");
    } else {
        wasm_path_buf.push("release");
    }
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

    // move the file into the build folder for parity with tinygo and convenience for users.
    let copied_wasm_file = common_config
        .build_dir
        .join(format!("{wasm_bin_name}.wasm"));
    if let Some(p) = copied_wasm_file.parent() {
        fs::create_dir_all(p)?;
    }
    fs::copy(&wasm_file, &copied_wasm_file)?;
    fs::remove_file(&wasm_file)?;

    // Return the full path to the compiled Wasm file
    Ok(copied_wasm_file)
}

/// Builds a tinygo component and returns the path to the file.
async fn build_tinygo_component(
    common_config: &CommonConfig,
    tinygo_config: &TinyGoConfig,
    component_config: &ComponentConfig,
) -> Result<PathBuf> {
    let wasm_file_path = common_config
        .build_dir
        .join(format!("{}.wasm", common_config.name));

    let mut command = match &tinygo_config.tinygo_path {
        Some(path) => tokio::process::Command::new(path),
        None => tokio::process::Command::new("tinygo"),
    };

    // Ensure the target directory which will contain the eventual filename exists
    // this usually means creating the build folder in the golang project root
    let build_dir = wasm_file_path.parent().unwrap_or(&common_config.build_dir);
    if !build_dir.exists() {
        fs::create_dir_all(build_dir)?;
    }

    if component_config.wit_world.is_some() && !tinygo_config.disable_go_generate {
        generate_tinygo_bindgen(common_config.project_dir.as_path())
            .await
            .context("generating golang bindgen code failed")?;
    }

    let output_file_path = format!("{}", wasm_file_path.display());
    let wit_dir = format!("{}", common_config.wit_dir.display());
    let build_args = match &component_config.wasm_target {
        WasmTarget::WasiP1 | WasmTarget::CoreModule => vec![
            "build",
            "-o",
            &output_file_path,
            "-target",
            tinygo_config.build_target(&component_config.wasm_target),
            "-scheduler",
            "none",
            "-no-debug",
            ".",
        ],
        WasmTarget::WasiP2 => {
            let mut args = vec![
                "build",
                "-o",
                &output_file_path,
                "-target",
                tinygo_config.build_target(&component_config.wasm_target),
                "-wit-package",
                &wit_dir,
                "-wit-world",
                component_config.wit_world.as_ref().context(
                    "missing `wit_world` in wasmcloud.toml ([component] section) to run go bindgen generate",
                )?,
            ];
            if let Some(scheduler) = &tinygo_config.scheduler {
                args.push("-scheduler");
                args.push(scheduler.as_str());
            }

            if let Some(gc) = &tinygo_config.garbage_collector {
                args.push("-gc");
                args.push(gc.as_str());
            }

            args.push(".");
            args
        }
    };

    let result = command
        .current_dir(&common_config.project_dir)
        .args(build_args)
        .status()
        .await
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                anyhow!("{:?} command is not found", command.as_std().get_program())
            } else {
                anyhow!(e)
            }
        })?;

    if !result.success() {
        bail!("Compiling component failed: {}", result.to_string())
    }

    if !wasm_file_path.exists() {
        bail!(
            "Could not find compiled wasm file to sign: {}",
            wasm_file_path.display()
        );
    }

    Ok(wasm_file_path)
}

/// Builds a wasmCloud component using a custom override command, then returns the path to the file.
async fn build_custom_component(
    common_config: &CommonConfig,
    component_config: &ComponentConfig,
    raw_command: &str,
) -> Result<PathBuf> {
    let (command, args) = parse_custom_command(raw_command)?;
    let mut command = tokio::process::Command::new(command);
    // All remaining elements of the split command are interpreted as arguments
    command
        .current_dir(&common_config.project_dir)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = command.output().await.map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            anyhow!("`{:?}` was not found", command.as_std().get_program())
        } else {
            anyhow!(format!(
                "failed to run `{:?}`: {e}",
                command.as_std().get_program()
            ))
        }
    })?;

    if !output.status.success() {
        let stdout_output = String::from_utf8_lossy(&output.stdout);
        let stderr_output = String::from_utf8_lossy(&output.stderr);
        eprintln!("STDOUT:\n{stdout_output}\nSTDERR:\n{stderr_output}");
        bail!(
            "failed to build component with custom command: {}",
            output.status.to_string()
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
                common_config.project_dir.join(p)
            }
        })
        .unwrap_or_else(|| {
            common_config
                .build_dir
                .join(format!("{}.wasm", common_config.wasm_bin_name()))
        });
    if std::fs::metadata(component_path.as_path()).is_err() {
        warn!(
            "Component built with custom command but not found in expected path [{}]",
            component_path.display()
        );
    }
    Ok(component_path)
}

/// Generate the bindgen code that `TinyGo` components need
async fn generate_tinygo_bindgen(project_dir: impl AsRef<Path>) -> Result<()> {
    let project_dir = project_dir.as_ref();
    if !tokio::fs::try_exists(project_dir).await.unwrap_or_default() {
        bail!("directory @ [{}] does not exist", project_dir.display(),);
    }

    let mut command = tokio::process::Command::new("go");
    let result = command
        .args(vec!["generate", "./..."])
        .current_dir(project_dir)
        // NOTE: this can be removed once upstream merges verbose flag
        // https://github.com/bytecodealliance/wasm-tools-go/pull/214
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                anyhow!("{:?} command is not found", command.as_std().get_program())
            } else {
                anyhow!(e)
            }
        })?;

    if !result.status.success() {
        let stdout_output = String::from_utf8_lossy(&result.stdout);
        let stderr_output = String::from_utf8_lossy(&result.stderr);
        eprintln!("STDOUT:\n{stdout_output}\nSTDERR:\n{stderr_output}");
        bail!("go generate failed: {}", result.status.to_string())
    }

    Ok(())
}

/// Adapt a core module/wasip2 component to a wasip2 wasm component
/// returning the bytes that are the adapted wasm module
fn adapt_wasip1_component(
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
        .adapter(
            WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME,
            adapter_wasm_bytes.as_ref(),
        )
        .context("failed to set adapter during encoding")?;

    // Return the encoded module bytes
    encoder
        .encode()
        .context("failed to serialize encoded component")
}

/// Retrieve bytes for WASIP2 adapter given a project configuration,
/// if required by project configuration
pub(crate) fn get_wasip2_adapter_bytes(config: &ComponentConfig) -> Result<Vec<u8>> {
    if let ComponentConfig {
        wasm_target: WasmTarget::WasiP2,
        wasip1_adapter_path: Some(path),
        ..
    } = config
    {
        return std::fs::read(path)
            .with_context(|| format!("failed to read wasm bytes from [{}]", path.display()));
    }
    Ok(WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER.into())
}

/// Embed required component metadata to a given WebAssembly binary
fn embed_wasm_component_metadata(
    wit_dir: impl AsRef<Path>,
    wit_world: impl AsRef<str>,
    input_wasm: impl AsRef<Path>,
    output_wasm: impl AsRef<Path>,
) -> Result<()> {
    let wit_dir = wit_dir.as_ref();
    if !wit_dir.is_dir() {
        bail!(
            "expected 'wit' directory under project path at [{}] is missing",
            wit_dir.display()
        );
    };

    let (resolver, world_id) = convert_wit_dir_to_world(wit_dir, wit_world.as_ref())
        .context("failed to resolve WIT world")?;

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

    use crate::lib::build::{CommonConfig, WASMCLOUD_WASM_TAG_EXPERIMENTAL};
    use crate::lib::parser::{ComponentConfig, RegistryConfig, WasmTarget};
    use anyhow::{Context, Result};
    use semver::Version;
    use wascap::{jwt::Token, wasm::extract_claims};
    use wasmparser::{Parser, Payload};

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

    const COMPONENT_GO_MOD: &str = r"
module example

go 1.24.1

tool go.bytecodealliance.org/cmd/wit-bindgen-go

require go.bytecodealliance.org/cm v0.1.0

require (
	github.com/coreos/go-semver v0.3.1 // indirect
	github.com/docker/libtrust v0.0.0-20160708172513-aabc10ec26b7 // indirect
	github.com/klauspost/compress v1.17.11 // indirect
	github.com/opencontainers/go-digest v1.0.0 // indirect
	github.com/regclient/regclient v0.7.2 // indirect
	github.com/sirupsen/logrus v1.9.3 // indirect
	github.com/ulikunitz/xz v0.5.12 // indirect
	github.com/urfave/cli/v3 v3.0.0-beta1 // indirect
	go.bytecodealliance.org v0.5.0 // indirect
	golang.org/x/mod v0.22.0 // indirect
	golang.org/x/sys v0.28.0 // indirect
)
    ";

    const COMPONENT_GO_SUM: &str = r"
github.com/coreos/go-semver v0.3.1 h1:yi21YpKnrx1gt5R+la8n5WgS0kCrsPp33dmEyHReZr4=
github.com/coreos/go-semver v0.3.1/go.mod h1:irMmmIw/7yzSRPWryHsK7EYSg09caPQL03VsM8rvUec=
github.com/davecgh/go-spew v1.1.0/go.mod h1:J7Y8YcW2NihsgmVo/mv3lAwl/skON4iLHjSsI+c5H38=
github.com/davecgh/go-spew v1.1.1 h1:vj9j/u1bqnvCEfJOwUhtlOARqs3+rkHYY13jYWTU97c=
github.com/davecgh/go-spew v1.1.1/go.mod h1:J7Y8YcW2NihsgmVo/mv3lAwl/skON4iLHjSsI+c5H38=
github.com/docker/libtrust v0.0.0-20160708172513-aabc10ec26b7 h1:UhxFibDNY/bfvqU5CAUmr9zpesgbU6SWc8/B4mflAE4=
github.com/docker/libtrust v0.0.0-20160708172513-aabc10ec26b7/go.mod h1:cyGadeNEkKy96OOhEzfZl+yxihPEzKnqJwvfuSUqbZE=
github.com/klauspost/compress v1.17.11 h1:In6xLpyWOi1+C7tXUUWv2ot1QvBjxevKAaI6IXrJmUc=
github.com/klauspost/compress v1.17.11/go.mod h1:pMDklpSncoRMuLFrf1W9Ss9KT+0rH90U12bZKk7uwG0=
github.com/olareg/olareg v0.1.1 h1:Ui7q93zjcoF+U9U71sgqgZWByDoZOpqHitUXEu2xV+g=
github.com/olareg/olareg v0.1.1/go.mod h1:w8NP4SWrHHtxsFaUiv1lnCnYPm4sN1seCd2h7FK/dc0=
github.com/opencontainers/go-digest v1.0.0 h1:apOUWs51W5PlhuyGyz9FCeeBIOUDA/6nW8Oi/yOhh5U=
github.com/opencontainers/go-digest v1.0.0/go.mod h1:0JzlMkj0TRzQZfJkVvzbP0HBR3IKzErnv2BNG4W4MAM=
github.com/pmezard/go-difflib v1.0.0 h1:4DBwDE0NGyQoBHbLQYPwSUPoCMWR5BEzIk/f1lZbAQM=
github.com/pmezard/go-difflib v1.0.0/go.mod h1:iKH77koFhYxTK1pcRnkKkqfTogsbg7gZNVY4sRDYZ/4=
github.com/regclient/regclient v0.7.2 h1:vcldDAwBMLtighYVMeb6qNt5+0hKg3AN2IkCc0JIJNM=
github.com/regclient/regclient v0.7.2/go.mod h1:QlA7W9/pvmbblOXM4d49JgfuOTwVXcUMKt3bFuOSVIQ=
github.com/sergi/go-diff v1.3.1 h1:xkr+Oxo4BOQKmkn/B9eMK0g5Kg/983T9DqqPHwYqD+8=
github.com/sergi/go-diff v1.3.1/go.mod h1:aMJSSKb2lpPvRNec0+w3fl7LP9IOFzdc9Pa4NFbPK1I=
github.com/sirupsen/logrus v1.9.3 h1:dueUQJ1C2q9oE3F7wvmSGAaVtTmUizReu6fjN8uqzbQ=
github.com/sirupsen/logrus v1.9.3/go.mod h1:naHLuLoDiP4jHNo9R0sCBMtWGeIprob74mVsIT4qYEQ=
github.com/stretchr/objx v0.1.0/go.mod h1:HFkY916IF+rwdDfMAkV7OtwuqBVzrE8GR6GFx+wExME=
github.com/stretchr/testify v1.7.0/go.mod h1:6Fq8oRcR53rry900zMqJjRRixrwX3KX962/h/Wwjteg=
github.com/stretchr/testify v1.9.0 h1:HtqpIVDClZ4nwg75+f6Lvsy/wHu+3BoSGCbBAcpTsTg=
github.com/stretchr/testify v1.9.0/go.mod h1:r2ic/lqez/lEtzL7wO/rwa5dbSLXVDPFyf8C91i36aY=
github.com/ulikunitz/xz v0.5.12 h1:37Nm15o69RwBkXM0J6A5OlE67RZTfzUxTj8fB3dfcsc=
github.com/ulikunitz/xz v0.5.12/go.mod h1:nbz6k7qbPmH4IRqmfOplQw/tblSgqTqBwxkY0oWt/14=
github.com/urfave/cli/v3 v3.0.0-beta1 h1:6DTaaUarcM0wX7qj5Hcvs+5Dm3dyUTBbEwIWAjcw9Zg=
github.com/urfave/cli/v3 v3.0.0-beta1/go.mod h1:FnIeEMYu+ko8zP1F9Ypr3xkZMIDqW3DR92yUtY39q1Y=
go.bytecodealliance.org v0.5.0 h1:ywhCpF0JdqGkqct5JqSY1Me8lz001UIJXUaCSS32cew=
go.bytecodealliance.org v0.5.0/go.mod h1:8kYTSxmQr8DU3dKOKCOHH1Ap1gWX/61qlFSbIuIno2Q=
go.bytecodealliance.org/cm v0.1.0 h1:78Rk4d5rgir5Hm+LMFpDWhjmFBWrKDFPSKUwDBj+nwo=
go.bytecodealliance.org/cm v0.1.0/go.mod h1:NZ2UT0DyGhBfpIPOxPMCuG6g1YTR4YF3xweD7mHX5VQ=
golang.org/x/mod v0.22.0 h1:D4nJWe9zXqHOmWqj4VMOJhvzj7bEZg4wEYa759z1pH4=
golang.org/x/mod v0.22.0/go.mod h1:6SkKJ3Xj0I0BrPOZoBy3bdMptDDU9oJrpohJ3eWZ1fY=
golang.org/x/sync v0.10.0 h1:3NQrjDixjgGwUOCaF8w2+VYHv0Ve/vGYSbdkTa98gmQ=
golang.org/x/sync v0.10.0/go.mod h1:Czt+wKu1gCyEFDUtn0jG5QVvpJ6rzVqr5aXyt9drQfk=
golang.org/x/sys v0.0.0-20220715151400-c0bba94af5f8/go.mod h1:oPkhp1MJrh7nUepCBck5+mAzfO9JrbApNNgaTdGDITg=
golang.org/x/sys v0.28.0 h1:Fksou7UEQUWlKvIdsqzJmUmCX3cZuD2+P3XyyzwMhlA=
golang.org/x/sys v0.28.0/go.mod h1:/VUhepiaJMQUp4+oa/7Zr1D23ma6VTLIYjOOTFZPUcA=
golang.org/x/tools v0.28.0 h1:WuB6qZ4RPCQo5aP3WdKZS7i595EdWqWR8vqJTlwTVK8=
golang.org/x/tools v0.28.0/go.mod h1:dcIOrVd3mfQKTgrDVQHqCPMWy6lnhfhtX3hLXYVLfRw=
gopkg.in/check.v1 v0.0.0-20161208181325-20d25e280405/go.mod h1:Co6ibVJAznAaIkqp8huTwlJQCZ016jof/cbN4VW5Yz0=
gopkg.in/yaml.v3 v3.0.0-20200313102051-9f266ea9e77c/go.mod h1:K4uyk7z7BCEPqu6E+C64Yfv1cQ7kz7rIZviUmN+EgEM=
gopkg.in/yaml.v3 v3.0.1 h1:fxVm/GzAzEWqLHuvctI91KS9hhNmmWOoWu0XTYJS7CA=
gopkg.in/yaml.v3 v3.0.1/go.mod h1:K4uyk7z7BCEPqu6E+C64Yfv1cQ7kz7rIZviUmN+EgEM=
";

    const COMPONENT_GO_GENERATE: &str = r"
//go:generate go tool wit-bindgen-go generate --out gen ./wit

package main

func main() {}
    ";

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
        embed_wasm_component_metadata(
            project_dir.path().join("wit"),
            "test-world",
            &wasm_path,
            &wasm_path,
        )
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
    /// *and* experimental tag in claims when waspi1 or waspi2 targets are signed
    #[test]
    fn sign_component_includes_experimental() -> Result<()> {
        // Build project path, including WIT dir
        let project_dir = tempfile::tempdir()?;
        let wasm_path = setup_build_component(&project_dir)?;

        // Check targets that should have experimental tag set
        for wasm_target in [
            WasmTarget::CoreModule,
            WasmTarget::WasiP1,
            WasmTarget::WasiP2,
        ] {
            let updated_wasm_path = sign_component_wasm(
                &CommonConfig {
                    name: "test".into(),
                    version: Version::parse("0.1.0")?,
                    revision: 0,
                    wit_dir: project_dir.path().join("wit"),
                    build_dir: project_dir.path().join("build"),
                    project_dir: project_dir.path().into(),
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
                WasmTarget::WasiP1 | WasmTarget::WasiP2 => assert!(
                    tags.contains(&String::from(WASMCLOUD_WASM_TAG_EXPERIMENTAL)),
                    "experimental tag should be present on wasip1/wasip2 components"
                ),
            }
        }

        Ok(())
    }

    /// Ensure that golang component generation works with a bindgen'd component
    #[tokio::test]
    async fn golang_generate_bindgen_component_basic() -> Result<()> {
        let project_dir = tempfile::tempdir()?;

        // Set up directories
        let wit_dir = project_dir.path().join("wit");
        let output_dir = project_dir.path().join("gen");
        std::fs::create_dir(&wit_dir).context("failed to create WIT dir")?;
        std::fs::create_dir(&output_dir).context("failed to create output dir")?;

        // Write WIT for Golang code
        std::fs::write(project_dir.path().join("go.mod"), COMPONENT_GO_MOD)
            .context("failed to write go mod")?;
        std::fs::write(project_dir.path().join("go.sum"), COMPONENT_GO_SUM)
            .context("failed to write go sum")?;
        std::fs::write(project_dir.path().join("main.go"), COMPONENT_GO_GENERATE)
            .context("failed to write go file")?;
        std::fs::write(wit_dir.join("test.wit"), COMPONENT_BASIC_WIT)
            .context("failed to write test WIT file")?;

        // Multiple worlds without specifying them in the wit-bindgen call. This should fail.
        generate_tinygo_bindgen(&project_dir)
            .await
            .context("failed to run tinygo bindgen")?;

        let dir_contents = fs::read_dir(output_dir)
            .context("failed to read dir")?
            .collect::<Result<Vec<DirEntry>, std::io::Error>>()?;

        assert!(!dir_contents.is_empty(), "no files generated");

        Ok(())
    }

    /// Ensure that golang component generation works with a bindgen'd component
    /// which has multiple worlds
    #[tokio::test]
    async fn golang_generate_bindgen_component_multi_world() -> Result<()> {
        let project_dir = tempfile::tempdir()?;

        // Set up directories
        let wit_dir = project_dir.path().join("wit");
        let output_dir = project_dir.path().join("gen");
        std::fs::create_dir(&wit_dir).context("failed to create WIT dir")?;
        std::fs::create_dir(&output_dir).context("failed to create output dir")?;

        // Write WIT for Golang code
        std::fs::write(project_dir.path().join("go.mod"), COMPONENT_GO_MOD)
            .context("failed to write go mod")?;
        std::fs::write(project_dir.path().join("go.sum"), COMPONENT_GO_SUM)
            .context("failed to write go sum")?;
        std::fs::write(project_dir.path().join("main.go"), COMPONENT_GO_GENERATE)
            .context("failed to write go file")?;
        std::fs::write(wit_dir.join("upstream.wit"), COMPONENT_UPSTREAM_WIT)
            .context("failed to write test WIT file")?;
        std::fs::write(wit_dir.join("downstream.wit"), COMPONENT_DOWNSTREAM_WIT)
            .context("failed to write test WIT file")?;

        // NOTE: multi-world is now supported when generating
        assert!(generate_tinygo_bindgen(&project_dir).await.is_ok());

        let dir_contents = fs::read_dir(output_dir)
            .context("failed to read dir")?
            .collect::<Result<Vec<DirEntry>, std::io::Error>>()?;

        assert!(!dir_contents.is_empty(), "no files generated");

        Ok(())
    }

    #[test]
    fn can_parse_custom_command() {
        let cargo_component_build = "cargo component build --release --target wasm32-wasip1";
        let tinygo_build =
            "tinygo build -o build/test.wasm -target wasm32-wasip1 -scheduler none -no-debug .";
        // Raw strings because backslashes are used and shouldn't trigger escape sequences
        let some_other_language = r"zig build-exe .\tiny-hello.zig -O ReleaseSmall -fstrip -fsingle-threaded -target aarch64-linux";

        let (command, args) = super::parse_custom_command(cargo_component_build)
            .expect("should be able to parse cargo command");
        assert_eq!(command, "cargo");
        assert_eq!(
            args,
            vec![
                "component",
                "build",
                "--release",
                "--target",
                "wasm32-wasip1"
            ]
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
                "wasm32-wasip1",
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

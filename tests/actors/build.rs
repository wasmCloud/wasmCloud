use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use anyhow::{bail, ensure, Context};
use futures::try_join;
use nkeys::KeyPair;
use serde::Deserialize;
use tokio::fs;
use tokio::process::Command;
use wascap::caps;
use wasmcloud_component_adapters::WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER;

// Unfortunately, `cargo` exported structs and enums do not implement `Deserialize`, so
// implement the relevant parts here

//https://github.com/rust-lang/cargo/blob/b0742b2145f02d3557f596d1ee4b36c0426f39ab/src/cargo/core/compiler/crate_type.rs#L8-L17
#[derive(Deserialize, Eq, PartialEq)]
enum CrateType {
    #[serde(rename = "bin")]
    Bin,
    #[serde(rename = "lib")]
    Lib,
    #[serde(rename = "rlib")]
    Rlib,
    #[serde(rename = "dylib")]
    Dylib,
    #[serde(rename = "cdylib")]
    Cdylib,
    #[serde(rename = "staticlib")]
    Staticlib,
    #[serde(other)]
    Other,
}

// from https://github.com/rust-lang/cargo/blob/b0742b2145f02d3557f596d1ee4b36c0426f39ab/src/cargo/core/manifest.rs#L267-L286
#[derive(Deserialize)]
struct Target {
    name: String,
    kind: Vec<CrateType>,
}

#[derive(Deserialize)]
#[serde(tag = "reason")]
enum BuildMessage {
    // from https://github.com/rust-lang/cargo/blob/b0742b2145f02d3557f596d1ee4b36c0426f39ab/src/cargo/util/machine_message.rs#L34-L44
    #[serde(rename = "compiler-artifact")]
    CompilerArtifact {
        target: Target,
        filenames: Vec<PathBuf>,
    },
    #[serde(other)]
    Other,
}

async fn build_artifacts(
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    pred: impl Fn(&str, &[CrateType]) -> bool,
) -> anyhow::Result<impl Iterator<Item = (String, Vec<PathBuf>)>> {
    let Output {
        status,
        stdout,
        stderr: _, // inherited
    } = Command::new(env::var("CARGO").unwrap())
        .env("CARGO_ENCODED_RUSTFLAGS", "--cfg\x1ftokio_unstable") // Enable tokio on WASI
        .args(["build", "--message-format=json-render-diagnostics"])
        .args(args)
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn `cargo` process")?
        .wait_with_output()
        .await
        .context("failed to call `cargo`")?;
    ensure!(status.success(), "`cargo` invocation failed");
    serde_json::Deserializer::from_reader(stdout.as_slice())
        .into_iter()
        .filter_map(|message| match message {
            Ok(BuildMessage::CompilerArtifact {
                target: Target { name, kind },
                filenames,
            }) if pred(&name, &kind) => Some((name, filenames)),
            _ => None,
        })
        .try_fold(BTreeMap::new(), |mut artifacts, (pkg, files)| {
            use std::collections::btree_map::Entry::{Occupied, Vacant};

            match artifacts.entry(pkg) {
                Occupied(e) => bail!("duplicate entry for `{}`", e.key()),
                Vacant(e) => {
                    e.insert(files);
                    Ok(artifacts)
                }
            }
        })
        .map(IntoIterator::into_iter)
}

async fn copy(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> anyhow::Result<u64> {
    let src = src.as_ref();
    let dst = dst.as_ref();
    fs::copy(&src, &dst)
        .await
        .with_context(|| format!("failed to copy `{}` to `{}`", src.display(), dst.display()))
}

trait DerefArtifact {
    fn deref_artifact(&self) -> Option<(&str, &[PathBuf])>;
}

impl DerefArtifact for Option<(String, Vec<PathBuf>)> {
    fn deref_artifact(&self) -> Option<(&str, &[PathBuf])> {
        self.as_ref()
            .map(|(pkg, paths)| (pkg.as_str(), paths.as_slice()))
    }
}

async fn install_rust_wasm32_wasi_actors(out_dir: impl AsRef<Path>) -> anyhow::Result<()> {
    let out_dir = out_dir.as_ref();

    // NOTE: this list should be kept sorted
    let project_names = [
        "builtins-component-reactor",
        "interfaces-handler-reactor",
        "interfaces-reactor",
        "pinger-config-component",
        "ponger-config-component",
    ];

    let cargo_build_args = [
        vec![
            "--manifest-path=./rust/Cargo.toml".to_string(),
            "--target=wasm32-wasi".to_string(),
        ],
        project_names
            .iter()
            .map(|n| format!("-p={n}"))
            .collect::<Vec<String>>(),
    ]
    .concat();

    try_join!(
        // Build component actors
        async {
            let mut artifacts = build_artifacts(cargo_build_args, |name, kind| {
                project_names.contains(&name)
                    && (kind.contains(&CrateType::Cdylib) || kind.contains(&CrateType::Bin))
            })
            .await
            .with_context(|| format!("failed to build {:?} crates", project_names))?;
            match (
                artifacts.next().deref_artifact(),
                artifacts.next().deref_artifact(),
                artifacts.next().deref_artifact(),
                artifacts.next().deref_artifact(),
                artifacts.next().deref_artifact(),
                artifacts.next(),
            ) {
                (
                    Some(("builtins-component-reactor", [builtins_component_reactor])),
                    Some(("interfaces-handler-reactor", [interfaces_handler_reactor])),
                    Some(("interfaces-reactor", [interfaces_reactor])),
                    Some(("pinger-config-component", [pinger_config_component])),
                    Some(("ponger-config-component", [ponger_config_component])),
                    None,
                ) => {
                    try_join!(
                        copy(
                            builtins_component_reactor,
                            out_dir.join("rust-builtins-component-reactor.wasm"),
                        ),
                        copy(
                            interfaces_reactor,
                            out_dir.join("rust-interfaces-reactor.wasm"),
                        ),
                        copy(
                            interfaces_handler_reactor,
                            out_dir.join("rust-interfaces-handler-reactor.wasm"),
                        ),
                        copy(
                            pinger_config_component,
                            out_dir.join("rust-pinger-config-component.wasm"),
                        ),
                        copy(
                            ponger_config_component,
                            out_dir.join("rust-ponger-config-component.wasm"),
                        ),
                    )
                }
                v => bail!("invalid {:?} build artifacts: {v:#?}", project_names),
            }
        },
    )
    .context("failed to build `wasm32-wasi` actors")?;

    Ok(())
}

#[cfg(not(feature = "docs"))]
fn encode_component(module: impl AsRef<[u8]>, adapter: &[u8]) -> anyhow::Result<Vec<u8>> {
    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(module.as_ref())
        .context("failed to set core component module")?
        .adapter("wasi_snapshot_preview1", adapter)
        .context("failed to add WASI adapter")?
        .encode()
        .context("failed to encode a component")
}

#[cfg(feature = "docs")]
fn encode_component(_: impl AsRef<[u8]>, _: &[u8]) -> anyhow::Result<Vec<u8>> {
    Ok(Vec::default())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=../../crates/actor");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=rust");

    let out_dir = env::var("OUT_DIR")
        .map(PathBuf::from)
        .context("failed to lookup `OUT_DIR`")?;
    install_rust_wasm32_wasi_actors(&out_dir).await?;

    // Build WASI reactor components
    for name in [
        "builtins-component-reactor",
        "interfaces-handler-reactor",
        "interfaces-reactor",
        "pinger-config-component",
        "ponger-config-component",
    ] {
        let path = out_dir.join(format!("rust-{name}.wasm"));
        let module = fs::read(&path)
            .await
            .with_context(|| format!("failed to read `{}`", path.display()))?;
        let component = encode_component(module, WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER)
            .with_context(|| format!("failed to encode `{}`", path.display()))?;

        let path = out_dir.join(format!("rust-{name}-preview2.wasm"));
        fs::write(&path, component)
            .await
            .with_context(|| format!("failed to write `{}`", path.display()))?;
    }

    // Create a new keypair to use when signing wasm modules built for test
    let issuer = KeyPair::new_account();
    println!(
        "cargo:rustc-env=ISSUER={}",
        issuer.seed().expect("failed to extract issuer seed")
    );

    // Sign the built wasm modules with relevant claims
    let builtin_caps: Vec<String> = vec![
        caps::BLOB.into(),
        caps::HTTP_CLIENT.into(),
        caps::HTTP_SERVER.into(),
        caps::KEY_VALUE.into(),
        caps::LOGGING.into(),
        caps::MESSAGING.into(),
        caps::NUMBERGEN.into(),
    ];
    for (name, caps) in [
        ("builtins-component-reactor", Some(builtin_caps.clone())),
        (
            "builtins-component-reactor-preview2",
            Some(builtin_caps.clone()),
        ),
        ("interfaces-handler-reactor-preview2", None),
        ("interfaces-reactor-preview2", None),
        ("pinger-config-component-preview2", None),
        ("ponger-config-component-preview2", None),
    ] {
        let wasm = fs::read(out_dir.join(format!("rust-{name}.wasm")))
            .await
            .with_context(|| format!("failed to read `{name}` Wasm"))?;
        let wasm = if cfg!(feature = "docs") {
            _ = caps;
            wasm
        } else {
            let module = KeyPair::new_module();
            let claims = wascap::prelude::ClaimsBuilder::new()
                .issuer(&issuer.public_key())
                .subject(&module.public_key())
                .with_metadata(wascap::jwt::Actor {
                    name: Some(name.into()),
                    caps,
                    call_alias: Some(name.into()),
                    ..Default::default()
                })
                .build();
            wascap::wasm::embed_claims(&wasm, &claims, &issuer)
                .context("failed to embed actor claims")?
        };
        fs::write(out_dir.join(format!("rust-{name}.signed.wasm")), wasm)
            .await
            .context("failed to write Wasm")?;
    }

    Ok(())
}

//! Stage trusted fuzz harnesses against a release checkout.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Subcommand, ValueEnum};
use serde::Deserialize;

const PARSER_DEPENDENCIES: &[&str] = &["http", "oci-client", "prost", "serde_json"];
const FUZZ_ONLY_PACKAGES: &[&str] = &["wash-runtime-fuzz", "libfuzzer-sys", "arbitrary"];
const ROUTE_INCLUDE: &str = r#"
#[allow(unexpected_cfgs)]
pub mod release_fuzz {
    #[cfg(fuzzing)]
    use super::*;
    #[cfg(fuzzing)]
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fuzz/support/http_route.rs"));
}
"#;

#[derive(Subcommand)]
pub(crate) enum Task {
    /// Stage harnesses into a separate release checkout.
    Prepare {
        #[arg(long)]
        source: PathBuf,
        #[arg(long, value_enum)]
        language: Language,
    },
    /// Reject fuzz dependencies that differ from the release lockfile.
    VerifyLock {
        #[arg(long)]
        source: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Language {
    Go,
    Rust,
}

pub(crate) fn run(harness: &Path, task: Task) -> Result<()> {
    match task {
        Task::Prepare { source, language } => prepare(harness, &source, language),
        Task::VerifyLock { source } => verify_lock(&source),
    }
}

fn prepare(harness: &Path, source: &Path, language: Language) -> Result<()> {
    let harness = harness.canonicalize().context("resolve harness checkout")?;
    let source = source.canonicalize().context("resolve release checkout")?;
    if source == harness {
        bail!("use a separate release checkout for fuzzing");
    }
    match language {
        Language::Go => prepare_go(&harness, &source),
        Language::Rust => prepare_rust(&harness, &source),
    }
}

fn prepare_go(harness: &Path, source: &Path) -> Result<()> {
    let mut files = Vec::new();
    go_harnesses(&harness.join("runtime-operator"), &mut files)?;
    if files.is_empty() {
        bail!("no Go fuzz harnesses found");
    }
    files.sort();
    for file in files {
        let destination = source.join(file.strip_prefix(harness)?);
        let parent = destination.parent().context("Go harness has no parent")?;
        if !parent.is_dir() {
            bail!("release is missing Go package {}", parent.display());
        }
        fs::copy(&file, &destination)
            .with_context(|| format!("stage Go harness {}", file.display()))?;
    }
    Ok(())
}

fn go_harnesses(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)
        .with_context(|| format!("read Go harness directory {}", directory.display()))?
    {
        let path = entry?.path();
        if path.is_dir() {
            go_harnesses(&path, files)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with("_fuzz_test.go"))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn prepare_rust(harness: &Path, source: &Path) -> Result<()> {
    let release: toml::Value = read_toml(&source.join("Cargo.toml"))?;
    let dependencies = release
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml::Value::as_table)
        .context("release Cargo.toml has no workspace dependencies")?;
    let mut parser_dependencies = toml::Table::new();
    for &name in PARSER_DEPENDENCIES {
        let dependency = dependencies
            .get(name)
            .with_context(|| format!("release workspace has no {name} dependency"))?;
        if dependency.get("path").is_some() || dependency.get("git").is_some() {
            bail!("cannot stage registry parser dependency {name}: {dependency}");
        }
        parser_dependencies.insert(name.to_string(), dependency.clone());
    }
    let mut manifest: toml::Value = read_toml(&harness.join("fuzz/Cargo.toml"))?;
    manifest
        .get_mut("workspace")
        .and_then(toml::Value::as_table_mut)
        .context("fuzz Cargo.toml has no workspace")?
        .insert("dependencies".to_string(), parser_dependencies.into());
    let destination = source.join("fuzz");
    fs::create_dir_all(&destination)?;
    for directory in ["src", "fuzz_targets", "seeds", "support"] {
        copy_tree(
            &harness.join("fuzz").join(directory),
            &destination.join(directory),
        )?;
    }
    fs::write(destination.join("Cargo.toml"), toml::to_string(&manifest)?)?;
    fs::copy(source.join("Cargo.lock"), destination.join("Cargo.lock"))?;
    let route_file = source.join("crates/wash-runtime/src/host/http.rs");
    let mut routes = fs::read_to_string(&route_file)?;
    if !routes.contains(ROUTE_INCLUDE) {
        routes.push_str(ROUTE_INCLUDE);
        fs::write(route_file, routes)?;
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in
        fs::read_dir(source).with_context(|| format!("read fuzz assets {}", source.display()))?
    {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Lockfile {
    package: Vec<Package>,
}

#[derive(Deserialize, PartialEq, Eq, Hash)]
struct Package {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("parse {}", path.display()))
}

fn verify_lock(source: &Path) -> Result<()> {
    let release: Lockfile = read_toml(&source.join("Cargo.lock"))?;
    let fuzz: Lockfile = read_toml(&source.join("fuzz/Cargo.lock"))?;
    let names: HashSet<&str> = release
        .package
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let locked: HashSet<&Package> = release.package.iter().collect();
    for package in &fuzz.package {
        if FUZZ_ONLY_PACKAGES.contains(&package.name.as_str())
            && !names.contains(package.name.as_str())
        {
            continue;
        }
        if !locked.contains(package) {
            bail!(
                "fuzz dependency {} {} differs from release Cargo.lock",
                package.name,
                package.version
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    const LOCK: &str = r#"version = 4
[[package]]
name = "serde_json"
version = "1.0.100"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-checksum"
"#;
    const RELEASE_MANIFEST: &str = r#"[workspace.dependencies]
http = { version = "1", default-features = false }
oci-client = { version = "0.16", default-features = false, features = ["rustls-tls"] }
prost = { version = "0.13", default-features = false }
serde_json = "1"
"#;

    fn source() -> Result<tempfile::TempDir> {
        let source = tempfile::Builder::new()
            .prefix("fuzz recovery ")
            .tempdir()?;
        fs::write(source.path().join("Cargo.toml"), RELEASE_MANIFEST)?;
        fs::write(source.path().join("Cargo.lock"), LOCK)?;
        let routes = source.path().join("crates/wash-runtime/src/host/http.rs");
        fs::create_dir_all(routes.parent().context("route parent")?)?;
        fs::write(routes, "// Release parser implementation.\n")?;
        Ok(source)
    }

    fn harness() -> Result<PathBuf> {
        crate::workspace_dir()
    }

    #[test]
    fn recovers_checkout_without_fuzz_files_or_hook() -> Result<()> {
        let source = source()?;
        prepare(&harness()?, source.path(), Language::Rust)?;
        let generated = source.path().join("fuzz");
        let manifest: toml::Value = read_toml(&generated.join("Cargo.toml"))?;
        assert_eq!(
            manifest["workspace"]["dependencies"]["oci-client"]["version"].as_str(),
            Some("0.16")
        );
        assert_eq!(
            manifest["workspace"]["dependencies"]["prost"]["version"].as_str(),
            Some("0.13")
        );
        assert_eq!(
            manifest["workspace"]["dependencies"]["http"]["default-features"].as_bool(),
            Some(false)
        );
        assert_eq!(
            manifest["workspace"]["dependencies"]["oci-client"]["features"]
                .as_array()
                .context("OCI features")?
                .len(),
            1
        );
        assert_eq!(fs::read_to_string(generated.join("Cargo.lock"))?, LOCK);
        assert!(generated.join("seeds/http_route/local").is_file());
        assert!(generated.join("support/http_route.rs").is_file());
        prepare(&harness()?, source.path(), Language::Rust)?;
        let routes =
            fs::read_to_string(source.path().join("crates/wash-runtime/src/host/http.rs"))?;
        assert!(routes.starts_with("// Release parser implementation.\n"));
        assert_eq!(routes.matches(ROUTE_INCLUDE).count(), 1);
        assert_eq!(fs::read_to_string(source.path().join("Cargo.lock"))?, LOCK);
        Ok(())
    }

    #[test]
    fn rejects_dependency_drift_and_reseeds_from_release() -> Result<()> {
        let source = source()?;
        prepare(&harness()?, source.path(), Language::Rust)?;
        let lock = source.path().join("fuzz/Cargo.lock");
        fs::write(&lock, LOCK.replace("1.0.100", "1.0.101"))?;
        let error = verify_lock(source.path()).expect_err("dependency drift must fail");
        assert!(error.to_string().contains("serde_json 1.0.101 differs"));
        prepare(&harness()?, source.path(), Language::Rust)?;
        verify_lock(source.path())?;
        fs::write(
            source.path().join("Cargo.lock"),
            LOCK.replace("1.0.100", "1.0.102"),
        )?;
        prepare(&harness()?, source.path(), Language::Rust)?;
        assert!(fs::read_to_string(lock)?.contains("1.0.102"));
        verify_lock(source.path())
    }

    #[test]
    fn rejects_source_checksum_changes_and_new_runtime_dependencies() -> Result<()> {
        let source = source()?;
        prepare(&harness()?, source.path(), Language::Rust)?;
        let lock = source.path().join("fuzz/Cargo.lock");
        for (old, new) in [
            ("release-checksum", "changed"),
            ("crates.io-index", "other-index"),
        ] {
            fs::write(&lock, LOCK.replace(old, new))?;
            assert!(verify_lock(source.path()).is_err());
        }
        fs::write(
            lock,
            format!(
                "{LOCK}\n[[package]]\nname = \"unexpected-runtime-dep\"\nversion = \"1.0.0\"\n"
            ),
        )?;
        let error = verify_lock(source.path()).expect_err("new runtime dependency must fail");
        assert!(error.to_string().contains("unexpected-runtime-dep"));
        Ok(())
    }

    #[test]
    fn allows_fuzz_tooling_without_changing_existing_tooling_dependencies() -> Result<()> {
        let source = source()?;
        prepare(&harness()?, source.path(), Language::Rust)?;
        let lock = source.path().join("fuzz/Cargo.lock");
        fs::write(
            &lock,
            format!("{LOCK}\n[[package]]\nname = \"libfuzzer-sys\"\nversion = \"0.4.10\"\n"),
        )?;
        verify_lock(source.path())?;
        let arbitrary = "\n[[package]]\nname = \"arbitrary\"\nversion = \"1.4.2\"\n";
        fs::write(
            source.path().join("Cargo.lock"),
            format!("{LOCK}{arbitrary}"),
        )?;
        fs::write(
            lock,
            format!("{LOCK}{}", arbitrary.replace("1.4.2", "1.5.0")),
        )?;
        let error =
            verify_lock(source.path()).expect_err("existing tooling dependency must remain pinned");
        assert!(error.to_string().contains("arbitrary 1.5.0 differs"));
        Ok(())
    }

    #[test]
    fn restores_missing_go_harnesses_without_changing_module() -> Result<()> {
        let source = source()?;
        let operator = source.path().join("runtime-operator");
        fs::create_dir(&operator)?;
        fs::write(operator.join("go.mod"), "module release\n")?;
        let harness = harness()?;
        let mut files = Vec::new();
        go_harnesses(&harness.join("runtime-operator"), &mut files)?;
        for file in files {
            let destination = source.path().join(file.strip_prefix(&harness)?);
            fs::create_dir_all(destination.parent().context("Go harness parent")?)?;
        }
        prepare(&harness, source.path(), Language::Go)?;
        assert_eq!(
            fs::read_to_string(operator.join("go.mod"))?,
            "module release\n"
        );
        assert!(
            operator
                .join("api/runtime/v1alpha1/workload_fuzz_test.go")
                .is_file()
        );
        Ok(())
    }

    #[test]
    fn rejects_staging_into_the_harness_checkout() -> Result<()> {
        let harness = harness()?;
        let error =
            prepare(&harness, &harness, Language::Rust).expect_err("source must be separate");
        assert!(
            error
                .to_string()
                .contains("use a separate release checkout")
        );
        Ok(())
    }
}

//! Resolve fixture WIT without registries or previously fetched dependencies.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

#[derive(Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    overrides: BTreeMap<String, Override>,
}

#[derive(Deserialize)]
struct Override {
    path: Option<PathBuf>,
}

pub(crate) fn run(fixtures: &Path) -> Result<()> {
    let mut configs = Vec::new();
    for entry in fs::read_dir(fixtures)
        .with_context(|| format!("read fixture root {}", fixtures.display()))?
    {
        let config = entry?.path().join("wkg.toml");
        if config.is_file() {
            configs.push(config);
        }
    }
    configs.sort();
    ensure!(
        !configs.is_empty(),
        "no fixture manifests found in {}",
        fixtures.display()
    );
    let scratch = tempfile::Builder::new()
        .prefix("fixture-wit-offline-")
        .tempdir()?;
    let mut failures = 0;
    for config in &configs {
        if let Err(error) = check_fixture(config, scratch.path()) {
            eprintln!("{}: {error:#}", config.parent().unwrap().display());
            failures += 1;
        }
    }
    println!(
        "Offline WIT resolution: {}/{} fixtures passed",
        configs.len() - failures,
        configs.len()
    );
    ensure!(failures == 0, "{failures} fixture WIT packages failed");
    Ok(())
}

fn check_fixture(config: &Path, scratch: &Path) -> Result<()> {
    let wit = stage_fixture(config, scratch)?;
    let result = Command::new("wasm-tools")
        .args(["component", "wit"])
        .arg(&wit)
        .stdout(Stdio::null())
        .output()
        .context("run wasm-tools component wit")?;
    if !result.status.success() {
        bail!(
            "wasm-tools exited with {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(())
}

fn stage_fixture(config: &Path, scratch: &Path) -> Result<PathBuf> {
    let fixture = config.parent().context("fixture manifest has no parent")?;
    let wit = scratch.join(fixture.file_name().context("fixture has no name")?);
    // An old fetch must not hide an incomplete local override manifest.
    copy_tree(&fixture.join("wit"), &wit, true)?;
    let deps = wit.join("deps");
    fs::create_dir_all(&deps)?;
    let manifest: Manifest = toml::from_str(&fs::read_to_string(config)?)
        .with_context(|| format!("parse {}", config.display()))?;
    for (package, override_) in manifest.overrides {
        let path = override_
            .path
            .with_context(|| format!("{package} needs a local path override"))?;
        let source = fixture.join(path);
        let target = deps.join(package.replace(':', "-"));
        if source.is_dir() {
            copy_tree(&source, &target, false)?;
        } else {
            fs::create_dir_all(&target)?;
            fs::copy(
                &source,
                target.join(source.file_name().context("override has no file name")?),
            )
            .with_context(|| format!("copy local override {}", source.display()))?;
        }
    }
    Ok(wit)
}

fn copy_tree(source: &Path, target: &Path, skip_deps: bool) -> Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if skip_deps && entry.file_name() == "deps" {
                continue;
            }
            copy_tree(&path, &target.join(entry.file_name()), skip_deps)?;
        } else {
            fs::copy(&path, target.join(entry.file_name()))
                .with_context(|| format!("copy {}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path) -> Result<PathBuf> {
        let fixture = root.join("fixture with spaces");
        fs::create_dir_all(fixture.join("wit/deps/stale"))?;
        fs::write(fixture.join("wit/world.wit"), "package test:fixture;")?;
        fs::write(fixture.join("wit/deps/stale/package.wit"), "stale")?;
        Ok(fixture)
    }

    #[test]
    fn stages_local_overrides_without_fetched_dependencies() -> Result<()> {
        let root = tempfile::tempdir()?;
        let fixture = fixture(root.path())?;
        let local = root.path().join("local source");
        fs::create_dir(&local)?;
        fs::write(local.join("package.wit"), "package test:local;")?;
        fs::write(root.path().join("single.wit"), "package test:single;")?;
        let config = fixture.join("wkg.toml");
        fs::write(
            &config,
            r#"[overrides]
"test:local" = { path = "../local source" }
[overrides."test:single"]
path = "../single.wit"
"#,
        )?;
        let scratch = tempfile::tempdir()?;
        let wit = stage_fixture(&config, scratch.path())?;
        assert!(wit.join("world.wit").is_file());
        assert!(!wit.join("deps/stale").exists());
        assert!(wit.join("deps/test-local/package.wit").is_file());
        assert!(wit.join("deps/test-single/single.wit").is_file());
        assert!(fixture.join("wit/deps/stale/package.wit").is_file());
        Ok(())
    }

    #[test]
    fn refuses_registry_only_overrides() -> Result<()> {
        let root = tempfile::tempdir()?;
        let fixture = fixture(root.path())?;
        let config = fixture.join("wkg.toml");
        fs::write(
            &config,
            "[overrides]\n\"test:remote\" = { version = \"1\" }",
        )?;
        let scratch = tempfile::tempdir()?;
        let error = stage_fixture(&config, scratch.path()).unwrap_err();
        assert!(error.to_string().contains("needs a local path override"));
        Ok(())
    }

    #[test]
    fn refuses_empty_fixture_roots() -> Result<()> {
        let root = tempfile::tempdir()?;
        let error = run(root.path()).unwrap_err();
        assert!(error.to_string().contains("no fixture manifests found"));
        Ok(())
    }
}

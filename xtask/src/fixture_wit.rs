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

/// The part of a fixture's `.wash/config.yaml` that decides what `wash build`
/// fetches.
#[derive(Default, Deserialize)]
struct WashConfig {
    #[serde(default)]
    wit: WitConfig,
}

#[derive(Default, Deserialize)]
struct WitConfig {
    #[serde(default)]
    skip_fetch: bool,
    wit_dir: Option<PathBuf>,
    #[serde(default)]
    sources: BTreeMap<String, String>,
}

pub(crate) fn run(fixtures: &Path) -> Result<()> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(fixtures)
        .with_context(|| format!("read fixture root {}", fixtures.display()))?
    {
        let dir = entry?.path();
        // `wash build` fetches for any project with a WIT directory, manifest
        // or not, and the config can move that directory.
        if [".wash/config.yaml", "wkg.toml", "wit"]
            .iter()
            .any(|marker| dir.join(marker).exists())
        {
            dirs.push(dir);
        }
    }
    dirs.sort();
    ensure!(
        !dirs.is_empty(),
        "no fixtures found in {}",
        fixtures.display()
    );
    let scratch = tempfile::Builder::new()
        .prefix("fixture-wit-offline-")
        .tempdir()?;
    let mut tool_checked = false;
    let (mut checked, mut failures) = (0, 0);
    for dir in &dirs {
        let result = match fetched_wit_dir(dir) {
            // `wash build` fetches nothing for this fixture.
            Ok(None) => continue,
            Ok(Some(wit_dir)) => {
                if !tool_checked {
                    ensure_wasm_tools()?;
                    tool_checked = true;
                }
                check_fixture(dir, &wit_dir, scratch.path())
            }
            Err(error) => Err(error),
        };
        checked += 1;
        if let Err(error) = result {
            eprintln!("{}: {error:#}", dir.display());
            failures += 1;
        }
    }
    println!(
        "Offline WIT resolution: {}/{checked} fixtures passed, {} fetch nothing",
        checked - failures,
        dirs.len() - checked
    );
    ensure!(failures == 0, "{failures} fixture WIT packages failed");
    Ok(())
}

/// The WIT directory `wash build` fetches dependencies for, or `None` when it
/// fetches nothing: `wit.skip_fetch` is set, or the directory does not exist.
fn fetched_wit_dir(fixture: &Path) -> Result<Option<PathBuf>> {
    let config = fixture.join(".wash/config.yaml");
    let wit = if config.is_file() {
        let contents =
            fs::read_to_string(&config).with_context(|| format!("read {}", config.display()))?;
        serde_saphyr::from_str::<WashConfig>(&contents)
            .with_context(|| format!("parse {}", config.display()))?
            .wit
    } else {
        WitConfig::default()
    };
    if wit.skip_fetch {
        return Ok(None);
    }
    // wash merges these into the override map with rules of its own
    // (namespace-wide keys; registry, git and http sources). Refusing them
    // beats passing a fixture on inputs this check would get wrong.
    ensure!(
        wit.sources.is_empty(),
        "wit.sources in {} is not covered by this check; use wkg.toml path overrides",
        config.display()
    );
    let wit_dir = fixture.join(wit.wit_dir.unwrap_or_else(|| PathBuf::from("wit")));
    Ok(wit_dir.is_dir().then_some(wit_dir))
}

/// One error for a missing tool, rather than one per fixture that reads as a
/// broken manifest.
fn ensure_wasm_tools() -> Result<()> {
    let found = Command::new("wasm-tools")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    ensure!(
        found,
        "check-fixture-wit needs `wasm-tools` on PATH (cargo install --locked wasm-tools)"
    );
    Ok(())
}

fn check_fixture(fixture: &Path, wit_dir: &Path, scratch: &Path) -> Result<()> {
    let wit = stage_fixture(fixture, wit_dir, scratch)?;
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

fn stage_fixture(fixture: &Path, wit_dir: &Path, scratch: &Path) -> Result<PathBuf> {
    let wit = scratch.join(fixture.file_name().context("fixture has no name")?);
    // An old fetch must not hide an incomplete local override manifest.
    copy_tree(wit_dir, &wit, true)?;
    let deps = wit.join("deps");
    fs::create_dir_all(&deps)?;
    // Without a manifest there are no overrides, so any package the WIT names
    // outside itself would come from a registry.
    let config = fixture.join("wkg.toml");
    let manifest: Manifest = if config.is_file() {
        toml::from_str(&fs::read_to_string(&config)?)
            .with_context(|| format!("parse {}", config.display()))?
    } else {
        Manifest::default()
    };
    // Every override is staged, used or not: wash hands them all to the
    // resolver, which looks up what each one depends on, so an unused override
    // with an uncovered dependency reaches the registry too.
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

    fn wash_config(fixture: &Path, yaml: &str) -> Result<()> {
        fs::create_dir_all(fixture.join(".wash"))?;
        fs::write(fixture.join(".wash/config.yaml"), yaml)?;
        Ok(())
    }

    #[test]
    fn stages_local_overrides_without_fetched_dependencies() -> Result<()> {
        let root = tempfile::tempdir()?;
        let fixture = fixture(root.path())?;
        let local = root.path().join("local source");
        fs::create_dir(&local)?;
        fs::write(local.join("package.wit"), "package test:local;")?;
        fs::write(root.path().join("single.wit"), "package test:single;")?;
        fs::write(
            fixture.join("wkg.toml"),
            r#"[overrides]
"test:local" = { path = "../local source" }
[overrides."test:single"]
path = "../single.wit"
"#,
        )?;
        let scratch = tempfile::tempdir()?;
        let wit = stage_fixture(&fixture, &fixture.join("wit"), scratch.path())?;
        assert!(wit.join("world.wit").is_file());
        assert!(!wit.join("deps/stale").exists());
        assert!(wit.join("deps/test-local/package.wit").is_file());
        assert!(wit.join("deps/test-single/single.wit").is_file());
        assert!(fixture.join("wit/deps/stale/package.wit").is_file());
        Ok(())
    }

    #[test]
    fn stages_fixtures_without_a_manifest() -> Result<()> {
        let root = tempfile::tempdir()?;
        let fixture = fixture(root.path())?;
        let scratch = tempfile::tempdir()?;
        let wit = stage_fixture(&fixture, &fixture.join("wit"), scratch.path())?;
        assert!(wit.join("world.wit").is_file());
        assert_eq!(fs::read_dir(wit.join("deps"))?.count(), 0);
        Ok(())
    }

    #[test]
    fn refuses_registry_only_overrides() -> Result<()> {
        let root = tempfile::tempdir()?;
        let fixture = fixture(root.path())?;
        fs::write(
            fixture.join("wkg.toml"),
            "[overrides]\n\"test:remote\" = { version = \"1\" }",
        )?;
        let scratch = tempfile::tempdir()?;
        let error = stage_fixture(&fixture, &fixture.join("wit"), scratch.path()).unwrap_err();
        assert!(error.to_string().contains("needs a local path override"));
        Ok(())
    }

    #[test]
    fn follows_the_wash_config() -> Result<()> {
        let root = tempfile::tempdir()?;
        let fixture = fixture(root.path())?;
        assert_eq!(fetched_wit_dir(&fixture)?, Some(fixture.join("wit")));

        wash_config(&fixture, "build:\n  command: cargo build\n")?;
        assert_eq!(fetched_wit_dir(&fixture)?, Some(fixture.join("wit")));

        wash_config(&fixture, "wit:\n  skip_fetch: true\n")?;
        assert_eq!(fetched_wit_dir(&fixture)?, None);

        wash_config(&fixture, "wit:\n  wit_dir: interfaces\n")?;
        assert_eq!(fetched_wit_dir(&fixture)?, None);
        fs::create_dir(fixture.join("interfaces"))?;
        assert_eq!(fetched_wit_dir(&fixture)?, Some(fixture.join("interfaces")));

        wash_config(&fixture, "wit:\n  sources:\n    \"test:local\": ../local\n")?;
        let error = fetched_wit_dir(&fixture).unwrap_err();
        assert!(error.to_string().contains("wit.sources"));
        Ok(())
    }

    #[test]
    fn refuses_empty_fixture_roots() -> Result<()> {
        let root = tempfile::tempdir()?;
        let error = run(root.path()).unwrap_err();
        assert!(error.to_string().contains("no fixtures found"));
        Ok(())
    }
}

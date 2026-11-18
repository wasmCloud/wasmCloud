//! Resolve fixture WIT without registries or previously fetched dependencies.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use wasm_pkg_common::{label::Label, package::PackageRef};
use wit_parser::{PackageName, Resolve, UnresolvedPackageGroup};

#[path = "../../crates/wash/src/project_paths.rs"]
mod project_paths;
use project_paths::{locate_project_config, namespace_candidates};

#[derive(Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    overrides: BTreeMap<String, Override>,
}

#[derive(Deserialize)]
struct Override {
    path: Option<PathBuf>,
}

/// The project settings that determine which WIT directory wash fetches.
#[derive(Default, Deserialize)]
struct WashConfig {
    wit: Option<WitConfig>,
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
        if locate_project_config(&dir).exists()
            || dir.join("wkg.toml").exists()
            || dir.join("wit").exists()
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
    let (mut checked, mut failures) = (0, 0);
    for dir in &dirs {
        let result = match fetched_wit_dir(dir) {
            // `wash build` fetches nothing for this fixture.
            Ok(None) => continue,
            Ok(Some(wit_dir)) => check_fixture(dir, &wit_dir),
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
    let config = locate_project_config(fixture);
    let wit = if config.exists() {
        let contents =
            fs::read_to_string(&config).with_context(|| format!("read {}", config.display()))?;
        let parsed: WashConfig = match config.extension().and_then(|ext| ext.to_str()) {
            Some("yaml" | "yml") => serde_saphyr::from_str(&contents).map_err(anyhow::Error::from),
            Some("json") => serde_json::from_str(&contents).map_err(anyhow::Error::from),
            Some("toml") => toml::from_str(&contents).map_err(anyhow::Error::from),
            _ => bail!("unknown config format: {}", config.display()),
        }
        .with_context(|| format!("parse {}", config.display()))?;
        parsed.wit.unwrap_or_default()
    } else {
        WitConfig::default()
    };
    let wit_dir = fixture.join(wit.wit_dir.unwrap_or_else(|| PathBuf::from("wit")));
    if wit.skip_fetch || !wit_dir.try_exists()? {
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
    Ok(Some(wit_dir))
}

fn parse_package(path: &Path) -> Result<UnresolvedPackageGroup> {
    if path.is_dir() {
        // parse_dir reads just this package's WIT files, ignoring fetched deps.
        return UnresolvedPackageGroup::parse_dir(path);
    }
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    UnresolvedPackageGroup::parse(path, &contents)
        .map_err(|(map, error)| anyhow::anyhow!("{}", error.render(&map)))
}

fn package_ref(name: &PackageName) -> Result<PackageRef> {
    Ok(PackageRef::new(name.namespace.parse()?, name.name.parse()?))
}

// wkg collects registry requirements from the main package and every nested
// package, even for an explicit override the fixture does not import.
fn requirements(group: &UnresolvedPackageGroup) -> impl Iterator<Item = PackageRef> + '_ {
    std::iter::once(&group.main)
        .chain(&group.nested)
        .flat_map(|pkg| pkg.foreign_deps.keys())
        .filter_map(|name| package_ref(name).ok())
}

fn namespace_packages(
    source: &Path,
    namespace: &Label,
) -> BTreeMap<PackageRef, UnresolvedPackageGroup> {
    let mut packages = BTreeMap::new();
    for candidate in namespace_candidates(source) {
        if let Ok(group) = parse_package(&candidate)
            && let Ok(name) = package_ref(&group.main.name)
            && name.namespace() == namespace
        {
            packages.entry(name).or_insert(group);
        }
    }
    packages
}

fn check_fixture(fixture: &Path, wit_dir: &Path) -> Result<()> {
    let config = fixture.join("wkg.toml");
    let manifest: Manifest = if config.try_exists()? {
        crate::read_toml(&config)?
    } else {
        Manifest::default()
    };
    let root = UnresolvedPackageGroup::parse_dir(wit_dir)?;
    let mut pending: BTreeSet<_> = requirements(&root).collect();
    let mut packages = BTreeMap::new();
    let mut namespaces = BTreeMap::new();
    for (key, entry) in manifest.overrides {
        if !key.contains(':') {
            // wash expands namespace overrides only for reachable packages.
            let namespace: Label = key
                .parse()
                .with_context(|| format!("invalid namespace {key}"))?;
            namespaces.insert(namespace, entry);
            continue;
        }
        let name: PackageRef = key
            .parse()
            .with_context(|| format!("invalid package {key}"))?;
        let path = entry
            .path
            .with_context(|| format!("{key} needs a local path override"))?;
        let path = fixture.join(path);
        let group =
            parse_package(&path).with_context(|| format!("load {key} from {}", path.display()))?;
        package_ref(&group.main.name)
            .with_context(|| format!("invalid package name in {}", path.display()))?;
        pending.extend(requirements(&group));
        // wkg registers the local source under its manifest key. The WIT
        // declaration alone cannot satisfy a lookup under a different key.
        packages.insert(name, group);
    }
    let mut indexes = BTreeMap::new();
    while let Some(name) = pending.pop_first() {
        if packages.contains_key(&name) {
            continue;
        }
        let namespace = name.namespace();
        let path = namespaces
            .get(namespace)
            .and_then(|entry| entry.path.as_ref())
            .with_context(|| format!("{name} needs a local path override"))?;
        let index = indexes
            .entry(namespace.clone())
            .or_insert_with(|| namespace_packages(&fixture.join(path), namespace));
        let group = index
            .remove(&name)
            .with_context(|| format!("namespace source {} has no {name}", path.display()))?;
        pending.extend(requirements(&group));
        packages.insert(name, group);
    }

    // Keep unstable interfaces enabled, as wasm-pkg-core does during fetch.
    let mut resolve = Resolve {
        all_features: true,
        ..Resolve::default()
    };
    resolve
        .push_groups(root, packages.into_values().collect())
        .map_err(|err| anyhow::anyhow!("{}", err.render(&resolve.source_map)))?;
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
    fn resolves_file_and_directory_overrides_by_manifest_key() -> Result<()> {
        let root = tempfile::tempdir()?;
        let fixture = fixture(root.path())?;
        let wit = fixture.join("wit");
        fs::write(
            wit.join("world.wit"),
            "package test:fixture; world fixture { import test:local/api; }",
        )?;
        let local = fixture.join("local source");
        fs::create_dir(&local)?;
        fs::write(
            local.join("api.wit"),
            "package test:local; interface api { use test:single/types.{id}; }",
        )?;
        fs::write(
            fixture.join("single.wit"),
            "package test:single; interface types { type id = u32; }",
        )?;
        let manifest = r#"[overrides]
"test:local" = { path = "local source" }
"test:single" = { path = "single.wit" }"#;
        fs::write(fixture.join("wkg.toml"), manifest)?;
        check_fixture(&fixture, &wit)?;
        // All package contents are still present, but wash would fetch the
        // transitive requirement because its manifest key is misspelled.
        fs::write(
            fixture.join("wkg.toml"),
            manifest.replace("test:single", "test:typo"),
        )?;
        let err = check_fixture(&fixture, &wit).unwrap_err();
        assert!(
            err.to_string()
                .contains("test:single needs a local path override"),
            "{err:#}"
        );
        Ok(())
    }

    #[test]
    fn ignores_fetched_deps_when_there_is_no_manifest() -> Result<()> {
        let root = tempfile::tempdir()?;
        let fixture = fixture(root.path())?;
        let wit = fixture.join("wit");
        check_fixture(&fixture, &wit)?;
        fs::write(
            wit.join("deps/stale/package.wit"),
            "package test:stale; interface api {}",
        )?;
        fs::write(
            wit.join("world.wit"),
            "package test:fixture; world fixture { import test:stale/api; }",
        )?;
        let err = check_fixture(&fixture, &wit).unwrap_err();
        assert!(
            err.to_string()
                .contains("test:stale needs a local path override"),
            "{err:#}"
        );
        Ok(())
    }

    #[test]
    fn resolves_reachable_namespace_packages_and_prefers_exact_overrides() -> Result<()> {
        let root = tempfile::tempdir()?;
        let fixture = fixture(root.path())?;
        let wit = fixture.join("wit");
        fs::write(
            wit.join("world.wit"),
            "package test:fixture; world fixture { import demo:api/service; }",
        )?;
        let source = fixture.join("source");
        fs::create_dir_all(source.join("deps/types"))?;
        fs::write(
            source.join("api.wit"),
            "package demo:api; interface service { use demo:types/types.{id}; }",
        )?;
        fs::write(
            source.join("deps/types/types.wit"),
            "package demo:types; interface types { use demo:value/value.{id}; }",
        )?;
        fs::write(
            source.join("value.wit"),
            "package demo:value; interface wrong {}",
        )?;
        fs::write(
            source.join("unused.wit"),
            "package demo:unused; world service { import remote:unused/api; }",
        )?;
        fs::write(
            fixture.join("value.wit"),
            "package demo:value; interface value { type id = u32; }",
        )?;
        let manifest = r#"[overrides]
demo = { path = "source" }
"demo:value" = { path = "value.wit" }
unused = { version = "1" }"#;
        fs::write(fixture.join("wkg.toml"), manifest)?;
        check_fixture(&fixture, &wit)?;
        fs::remove_file(source.join("deps/types/types.wit"))?;
        let err = check_fixture(&fixture, &wit).unwrap_err();
        assert!(err.to_string().contains("has no demo:types"), "{err:#}");
        Ok(())
    }

    #[test]
    fn rejects_malformed_override_keys() -> Result<()> {
        let root = tempfile::tempdir()?;
        let fixture = fixture(root.path())?;
        for key in ["bad/name", "test:bad/name"] {
            fs::write(
                fixture.join("wkg.toml"),
                format!(
                    r#"[overrides]
"{key}" = {{ path = "wit" }}"#
                ),
            )?;
            let err = check_fixture(&fixture, &fixture.join("wit")).unwrap_err();
            assert!(err.to_string().contains(key), "{err:#}");
        }
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
        let error = check_fixture(&fixture, &fixture.join("wit")).unwrap_err();
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
    fn discovers_alternate_project_configs_in_wash_order() -> Result<()> {
        let root = tempfile::tempdir()?;
        let fixture = root.path().join("fixture");
        let config_dir = fixture.join(".wash");
        let interfaces = fixture.join("interfaces");
        fs::create_dir_all(&config_dir)?;
        fs::create_dir(&interfaces)?;
        fs::write(interfaces.join("root.wit"), "package test:fixture;")?;
        for (name, contents) in [
            ("config.toml", "[wit]\nwit_dir = 'interfaces'"),
            ("config.json", r#"{"wit":{"wit_dir":"interfaces"}}"#),
            ("config.yml", "wit:\n  wit_dir: interfaces"),
            ("config.yaml", "wit:\n  wit_dir: interfaces"),
        ] {
            fs::write(config_dir.join(name), contents)?;
            assert_eq!(locate_project_config(&fixture), config_dir.join(name));
            assert_eq!(fetched_wit_dir(&fixture)?.as_ref(), Some(&interfaces));
            run(root.path())?;
            fs::write(
                interfaces.join("root.wit"),
                "package test:fixture; world fixture { import missing:dep/api; }",
            )?;
            assert!(run(root.path()).is_err());
            fs::write(interfaces.join("root.wit"), "package test:fixture;")?;
        }
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

use super::*;

/// `contracttest` is neither a fallback namespace (`wasi`, `ba`) nor one the default config maps
/// to a registry, so no lookup in this suite can reach the network.
async fn fetcher(dir: &Path) -> Result<WkgFetcher> {
    let cache_dir = dir.join("cache");
    Ok(WkgFetcher::new(
        wasm_pkg_core::manifest::Manifest::default(),
        wasm_pkg_client::Config::default(),
        FileCache::new(cache_dir.clone()).await?,
        cache_dir,
    ))
}

fn package(dir: &Path) -> Result<PathBuf> {
    let path = dir.join("contract");
    std::fs::create_dir_all(&path)?;
    std::fs::write(
        path.join("contract.wit"),
        "package contracttest:api@1.2.3; interface values { get: func() -> u32; } world service { export values; }",
    )?;
    Ok(path)
}

fn root(dir: &Path) -> Result<PathBuf> {
    let path = dir.join("wit");
    std::fs::create_dir_all(&path)?;
    std::fs::write(
        path.join("root.wit"),
        "package app:root; world service { import contracttest:api/values@1.2.3; }",
    )?;
    Ok(path)
}

/// Removes the clone that `clone_git_and_find_wit` keeps, whether or not the asserts pass.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn local_overrides_load_named_package_contents_without_a_registry() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = package(dir.path())?;
    for key in ["contracttest:api", "contracttest"] {
        let mut fetcher = fetcher(dir.path()).await?;
        fetcher.wkg_config.overrides = Some(HashMap::from([(
            key.into(),
            wasm_pkg_core::manifest::Override {
                path: Some(path.clone()),
                version: None,
            },
        )]));
        for version in [None, Some("1.2.3")] {
            match fetcher
                .load_package(&"contracttest:api".parse()?, version, None)
                .await
            {
                PackageLookup::Found(contents) => {
                    assert_eq!(contents.version, Some(semver::Version::new(1, 2, 3)));
                    assert_eq!(contents.interfaces, ["values"]);
                    assert_eq!(contents.worlds, ["service"]);
                }
                PackageLookup::Failed(error) => return Err(error),
                PackageLookup::Missing(message) => anyhow::bail!(message),
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn pinned_overrides_serve_the_pinned_version() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = package(dir.path())?;
    let mut fetcher = fetcher(dir.path()).await?;
    fetcher.wkg_config.overrides = Some(HashMap::from([(
        "contracttest:api".into(),
        wasm_pkg_core::manifest::Override {
            path: Some(path),
            version: Some("=1.2.3".parse()?),
        },
    )]));
    for version in [Some("1.2.3"), None] {
        match fetcher
            .load_package(&"contracttest:api".parse()?, version, None)
            .await
        {
            PackageLookup::Found(contents) => assert_eq!(contents.interfaces, ["values"]),
            PackageLookup::Failed(error) => return Err(error),
            PackageLookup::Missing(message) => anyhow::bail!(message),
        }
    }
    Ok(())
}

#[tokio::test]
async fn lookup_distinguishes_invalid_versions_missing_packages_and_bad_overrides() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut fetcher = fetcher(dir.path()).await?;
    let name: PackageRef = "contracttest:api".parse()?;
    match fetcher.load_package(&name, Some("garbage"), None).await {
        PackageLookup::Failed(error) => assert!(error.to_string().contains("invalid version")),
        _ => anyhow::bail!("invalid version was not a lookup failure"),
    }
    match fetcher.load_package(&name, None, None).await {
        PackageLookup::Missing(message) => {
            assert!(message.contains("No registry is configured"), "{message}")
        }
        _ => anyhow::bail!("unconfigured registry was not reported as missing"),
    }
    fetcher.wkg_config.overrides = Some(HashMap::from([(
        name.to_string(),
        wasm_pkg_core::manifest::Override {
            path: Some(dir.path().join("missing")),
            version: None,
        },
    )]));
    match fetcher.load_package(&name, None, None).await {
        PackageLookup::Failed(error) => {
            let error = format!("{error:#}");
            assert!(error.contains("failed to apply the override"), "{error}");
            assert!(error.contains("does not exist"), "{error}");
        }
        _ => anyhow::bail!("missing override path was not a lookup failure"),
    }
    let path = package(dir.path())?;
    fetcher.wkg_config.overrides = Some(HashMap::from([(
        name.to_string(),
        wasm_pkg_core::manifest::Override {
            path: Some(path),
            version: Some("=1.2.3".parse()?),
        },
    )]));
    match fetcher.load_package(&name, Some("2.0.0"), None).await {
        PackageLookup::Missing(message) => {
            assert!(message.contains("pinned to [=1.2.3]"), "{message}");
            assert!(message.contains("requested version [2.0.0]"), "{message}");
        }
        _ => anyhow::bail!("incompatible pin was not reported as missing"),
    }
    Ok(())
}

#[tokio::test]
async fn fetching_a_local_override_populates_deps_and_builds_the_root() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = package(dir.path())?;
    let root = root(dir.path())?;
    let mut fetcher = fetcher(dir.path()).await?;
    fetcher.wkg_config.overrides = Some(HashMap::from([(
        "contracttest:api".into(),
        wasm_pkg_core::manifest::Override {
            path: Some(path),
            version: None,
        },
    )]));
    let mut lock = load_lock_file(dir.path()).await?;
    fetcher.fetch_wit_dependencies(&root, &mut lock).await?;

    let mut dependency = None;
    for entry in std::fs::read_dir(root.join("deps"))? {
        let entry = entry?.path();
        if entry
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("contracttest-api"))
        {
            dependency = Some(entry);
        }
    }
    let dependency = dependency.context("deps has no contracttest-api directory")?;
    let mut wit = None;
    for entry in std::fs::read_dir(&dependency)? {
        let entry = entry?.path();
        if entry
            .extension()
            .is_some_and(|extension| extension == "wit")
        {
            wit = Some(entry);
        }
    }
    let wit = wit.context("dependency directory has no .wit file")?;
    let contents = std::fs::read_to_string(&wit)?;
    assert!(
        contents.contains("package contracttest:api@1.2.3"),
        "{contents}"
    );

    let (name, version, bytes) = fetcher.build_wit_package(&root, &mut lock).await?;
    assert_eq!(name.to_string(), "app:root");
    assert_eq!(version, None);
    assert!(bytes.starts_with(b"\0asm"));
    assert!(fetcher.diagnose_wit(&root).await?.is_empty());

    std::fs::write(root.join("root.wit"), "invalid WIT")?;
    match fetcher.build_wit_package(&root, &mut lock).await {
        Ok(_) => anyhow::bail!("invalid WIT built"),
        Err(error) => assert!(
            format!("{error:#}").contains("Couldn't parse package"),
            "{error:#}"
        ),
    }
    match fetcher.fetch_wit_dependencies(&root, &mut lock).await {
        Ok(()) => anyhow::bail!("invalid WIT fetched"),
        Err(error) => assert!(
            format!("{error:#}").contains("Couldn't parse package"),
            "{error:#}"
        ),
    }
    match fetcher.diagnose_wit(&root).await {
        Ok(messages) => anyhow::bail!("invalid WIT was diagnosed as {messages:?}"),
        Err(error) => assert!(
            format!("{error:#}").contains("Couldn't parse package"),
            "{error:#}"
        ),
    }
    Ok(())
}

#[tokio::test]
async fn diagnoses_conflicting_versions_and_source_overrides() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("wit");
    std::fs::create_dir(&path)?;
    let mut fetcher = fetcher(dir.path()).await?;
    for (imports, expected) in [
        (
            "import contracttest:api/values@1.2.3; import contracttest:api/values@1.2.4;",
            "same major.minor line",
        ),
        (
            "import contracttest:api/values@1.2.3; import contracttest:api/values@2.0.0;",
            "more than one version",
        ),
        (
            "import contracttest:api/values; import contracttest:api/values@1.2.3;",
            "both with and without a version",
        ),
        (
            "import contracttest:api/values@1.2.3;",
            "No registry is configured",
        ),
    ] {
        std::fs::write(
            path.join("root.wit"),
            format!("package app:root; world service {{ {imports} }}"),
        )?;
        let messages = fetcher.diagnose_wit(&path).await?;
        assert_eq!(messages.len(), 1);
        assert!(messages.first().unwrap().contains(expected), "{messages:?}");
    }
    fetcher.wkg_config.overrides = Some(HashMap::from([(
        "contracttest".into(),
        wasm_pkg_core::manifest::Override {
            path: Some(package(dir.path())?),
            version: None,
        },
    )]));
    assert!(fetcher.diagnose_wit(&path).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn applying_project_sources_reports_missing_paths_and_preserves_overrides() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = package(dir.path())?;
    let mut fetcher = fetcher(dir.path()).await?;
    let mut config = WitConfig::default();
    config.sources = HashMap::from([("contracttest".into(), "./contract".into())]);
    fetcher.apply_wit_config(&config, dir.path()).await?;
    assert_eq!(
        fetcher
            .wkg_config
            .overrides
            .as_ref()
            .unwrap()
            .get("contracttest")
            .unwrap()
            .path
            .as_ref(),
        Some(&path)
    );
    config.sources = HashMap::from([("contracttest:missing".into(), "./missing".into())]);
    let error = fetcher
        .apply_wit_config(&config, dir.path())
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("does not exist"), "{error:#}");
    Ok(())
}

#[tokio::test]
async fn load_config_merges_a_config_file_and_adds_the_default_namespaces() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let config_path = dir.path().join("wkg.toml");
    std::fs::write(
        &config_path,
        "[namespace_registries]\ncontracttest = \"local.test\"\n",
    )?;
    let config = CommonPackageArgs {
        config: Some(config_path),
        cache: Some(dir.path().join("cache")),
    }
    .load_config()
    .await?;
    let label = "contracttest".parse().context("invalid label")?;
    match config.namespace_registry(&label) {
        Some(RegistryMapping::Registry(registry)) => {
            assert_eq!(registry.to_string(), "local.test")
        }
        Some(RegistryMapping::Custom(_)) => {
            anyhow::bail!("contracttest was mapped to a custom registry")
        }
        None => anyhow::bail!("contracttest mapping was not loaded from the config file"),
    }
    for namespace in ["wasmcloud", "wrpc"] {
        let label = namespace.parse().context("invalid label")?;
        assert!(config.namespace_registry(&label).is_some(), "{namespace}");
    }
    Ok(())
}

#[tokio::test]
async fn git_sources_clone_local_repositories_and_find_the_wit_root() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let repository = dir.path().join("source.git");
    let wit = repository.join("wit");
    std::fs::create_dir_all(&wit)?;
    std::fs::write(repository.join(".gitattributes"), "* -text\n")?;
    std::fs::write(
        wit.join("contract.wit"),
        "package contracttest:api;\n\nworld service {}\n",
    )?;
    for args in [
        vec!["init", "--quiet"],
        vec!["add", ".gitattributes", "wit/contract.wit"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    ] {
        let output = tokio::process::Command::new("git")
            .args(args)
            .current_dir(&repository)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", dir.path().join("empty-config"))
            .output()
            .await?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let url = format!(
        "git+{}",
        Url::from_directory_path(&repository)
            .map_err(|()| anyhow::anyhow!("invalid repository path"))?
    );
    let cloned = clone_git_and_find_wit(&url).await?;
    let checkout = RemoveOnDrop(
        cloned
            .parent()
            .context("cloned WIT directory has no parent")?
            .to_path_buf(),
    );
    assert_eq!(
        std::fs::read(cloned.join("contract.wit"))?,
        std::fs::read(wit.join("contract.wit"))?
    );
    assert!(checkout.0.join(".git").is_dir());
    Ok(())
}

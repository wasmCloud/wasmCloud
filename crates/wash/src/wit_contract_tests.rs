use super::*;

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
            assert!(format!("{error:#}").contains("failed to apply the override"))
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
async fn fetch_build_and_diagnose_local_wit_packages() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = package(dir.path())?;
    let fetcher = fetcher(dir.path()).await?;
    let mut lock = load_lock_file(dir.path()).await?;
    fetcher.fetch_wit_dependencies(&path, &mut lock).await?;
    let (name, version, bytes) = fetcher.build_wit_package(&path, &mut lock).await?;
    assert_eq!(name.to_string(), "contracttest:api");
    assert_eq!(version, Some(semver::Version::new(1, 2, 3)));
    assert!(bytes.starts_with(b"\0asm"));
    assert!(fetcher.diagnose_wit(&path).await?.is_empty());
    std::fs::write(path.join("contract.wit"), "invalid WIT")?;
    assert!(fetcher.build_wit_package(&path, &mut lock).await.is_err());
    assert!(
        fetcher
            .fetch_wit_dependencies(&path, &mut lock)
            .await
            .is_err()
    );
    assert!(fetcher.diagnose_wit(&path).await.is_err());
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
async fn git_sources_clone_local_repositories_and_find_the_wit_root() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let repository = dir.path().join("source.git");
    let wit = repository.join("wit");
    std::fs::create_dir_all(&wit)?;
    std::fs::write(
        wit.join("contract.wit"),
        "package contracttest:api; world service {}",
    )?;
    for args in [
        vec!["init", "--quiet"],
        vec!["add", "wit/contract.wit"],
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
    assert_eq!(
        std::fs::read(cloned.join("contract.wit"))?,
        std::fs::read(wit.join("contract.wit"))?
    );
    assert!(cloned.parent().unwrap().join(".git").is_dir());
    std::fs::remove_dir_all(cloned.parent().unwrap())?;
    Ok(())
}

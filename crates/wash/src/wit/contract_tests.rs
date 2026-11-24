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

/// A fetch waits while another wash process holds the cache lock: its scrub
/// would otherwise delete a blob that process is still writing.
#[tokio::test]
async fn fetching_waits_for_the_cache_lock() -> Result<()> {
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
    let cache = dir.path().join("cache");
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(cache.join(".wash-cache.lock"))?;
    held.lock()?;
    let writing = cache.join(format!("sha256:{}", "a".repeat(64)));
    std::fs::write(&writing, b"partial")?;

    let mut lock = load_lock_file(dir.path()).await?;
    let mut fetch = std::pin::pin!(fetcher.fetch_wit_dependencies(&root, &mut lock));
    let early = tokio::time::timeout(std::time::Duration::from_millis(500), fetch.as_mut()).await;
    assert!(
        early.is_err(),
        "fetched while another process held the cache lock"
    );
    assert!(
        writing.exists(),
        "the scrub deleted a blob another process was writing"
    );

    drop(held);
    fetch.await?;
    Ok(())
}

/// A fetcher whose `contracttest:api` comes from a local override in `dir`.
async fn overridden(dir: &Path) -> Result<(WkgFetcher, PathBuf)> {
    let path = package(dir)?;
    let root = root(dir)?;
    let mut fetcher = fetcher(dir).await?;
    fetcher.wkg_config.overrides = Some(HashMap::from([(
        "contracttest:api".into(),
        wasm_pkg_core::manifest::Override {
            path: Some(path),
            version: None,
        },
    )]));
    Ok((fetcher, root))
}

fn hold_cache_lock(dir: &Path) -> Result<std::fs::File> {
    let held = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("cache").join(CACHE_LOCK_FILE))?;
    held.lock()?;
    Ok(held)
}

/// A lock file this user cannot write is still taken, read-only, and the shared
/// cache is checked under it.
#[cfg(unix)]
#[tokio::test]
async fn a_lock_file_this_user_cannot_write_does_not_fail_the_fetch() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir()?;
    let (fetcher, root) = overridden(dir.path()).await?;
    let lock_file = dir.path().join("cache").join(CACHE_LOCK_FILE);
    std::fs::write(&lock_file, b"")?;
    std::fs::set_permissions(&lock_file, std::fs::Permissions::from_mode(0o444))?;
    if std::fs::OpenOptions::new()
        .write(true)
        .open(&lock_file)
        .is_ok()
    {
        return Ok(()); // root ignores the mode
    }
    let partial = dir
        .path()
        .join("cache")
        .join(format!("sha256:{}", "a".repeat(64)));
    std::fs::write(&partial, b"partial")?;
    let mut lock = load_lock_file(dir.path()).await?;
    fetcher.fetch_wit_dependencies(&root, &mut lock).await?;
    anyhow::ensure!(
        !partial.exists(),
        "the shared cache was not checked under the read-only lock"
    );
    Ok(())
}

/// A lock that stays held past the wait, as by a stopped process, gives the call a
/// private cache instead of a hang.
#[tokio::test]
async fn a_cache_lock_held_past_the_wait_gives_a_private_cache() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (mut fetcher, root) = overridden(dir.path()).await?;
    fetcher.lock_wait = std::time::Duration::from_millis(200);
    let _held = hold_cache_lock(dir.path())?;
    let writing = dir
        .path()
        .join("cache")
        .join(format!("sha256:{}", "a".repeat(64)));
    std::fs::write(&writing, b"partial")?;
    let mut lock = load_lock_file(dir.path()).await?;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        fetcher.fetch_wit_dependencies(&root, &mut lock),
    )
    .await
    .context("the fetch waited on a held cache lock past its wait")??;
    anyhow::ensure!(
        writing.exists(),
        "the fetch scrubbed the shared cache without its lock"
    );
    Ok(())
}

/// A wait that runs out leaves no thread blocked on the lock, so the runtime can
/// shut down while another process still holds it.
#[test]
fn a_timed_out_wait_for_the_cache_lock_leaves_no_thread_behind() -> Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir_all(dir.path().join("cache"))?;
    let held = hold_cache_lock(dir.path())?;
    let path = dir.path().join("cache").join(CACHE_LOCK_FILE);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let waited = runtime.block_on(lock_exclusively(&path, Duration::from_millis(200)));
    anyhow::ensure!(
        waited
            .as_ref()
            .is_err_and(|error| error.kind() == std::io::ErrorKind::TimedOut),
        "the wait did not time out: {waited:?}"
    );
    let (stopped, shut_down) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(runtime);
        let _ = stopped.send(());
    });
    // The bound only stops a hang; the runtime's shutdown is what ends the wait.
    let shut_down = shut_down.recv_timeout(Duration::from_secs(10));
    drop(held);
    shut_down.context("the runtime waited for a thread still blocked on the cache lock")?;
    Ok(())
}

/// Where no lock file can be made, as in a read-only cache, a fetch that writes
/// nothing to the cache still works.
#[cfg(unix)]
#[tokio::test]
async fn a_read_only_cache_that_needs_no_writes_still_fetches() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir()?;
    let (fetcher, root) = overridden(dir.path()).await?;
    let cache = dir.path().join("cache");
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o555))?;
    let _restore = std::mem::DropGuard::new(cache.clone(), |cache| {
        let _ = std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o755));
    });
    if std::fs::write(cache.join("probe"), b"").is_ok() {
        return Ok(()); // root ignores the mode
    }
    let mut lock = load_lock_file(dir.path()).await?;
    fetcher.fetch_wit_dependencies(&root, &mut lock).await
}

/// Diagnosing a failed fetch only asks registries, so it does not wait for the
/// cache lock.
#[tokio::test]
async fn diagnosing_does_not_wait_for_the_cache_lock() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (fetcher, root) = overridden(dir.path()).await?;
    let _held = hold_cache_lock(dir.path())?;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fetcher.diagnose_wit(&root),
    )
    .await
    .context("diagnose_wit waited for the cache lock")??;
    Ok(())
}

/// Passes the message of every event on the thread it is set for to a callback.
struct OnEvent<F>(F);

impl<F: Fn(&str) + Send + Sync + 'static> tracing::Subscriber for OnEvent<F> {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Message(String);
        impl tracing::field::Visit for Message {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                }
            }
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        (self.0)(&message.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Calls `callback` with each event message on this thread until the result drops.
fn on_event(
    callback: impl Fn(&str) + Send + Sync + 'static,
) -> (tracing::Dispatch, tracing::subscriber::DefaultGuard) {
    // With one dispatcher, tracing caches a callsite as disabled for good if it is
    // first hit on a thread without one; a second makes it ask every dispatcher.
    let second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    (second, tracing::subscriber::set_default(OnEvent(callback)))
}

/// A fetch that waits for another wash process says so instead of hanging
/// silently.
#[tokio::test]
async fn waiting_for_the_cache_lock_is_logged() -> Result<()> {
    let waiting = std::sync::Arc::new(tokio::sync::Notify::new());
    let _events = on_event({
        let waiting = waiting.clone();
        move |message| {
            if message.contains("waiting for another wash process") {
                waiting.notify_one();
            }
        }
    });
    let dir = tempfile::tempdir()?;
    let (fetcher, root) = overridden(dir.path()).await?;
    let held = hold_cache_lock(dir.path())?;
    let mut lock = load_lock_file(dir.path()).await?;
    let mut fetch = std::pin::pin!(fetcher.fetch_wit_dependencies(&root, &mut lock));
    // The timeout only stops a hang; the log line is what ends the wait.
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        tokio::select! {
            fetched = fetch.as_mut() => {
                anyhow::bail!("fetched while another process held the cache lock: {fetched:?}")
            }
            () = waiting.notified() => Ok(()),
        }
    })
    .await
    .context("waited for the cache lock without saying so")??;
    drop(held);
    fetch.await
}

/// Waits until `call` reaches `registry`, then checks the cache lock is held
/// exclusively.
async fn assert_lock_held_on_the_network<F: std::future::Future>(
    registry: &tokio::net::TcpListener,
    lock_path: &Path,
    call: F,
    name: &str,
) -> Result<()> {
    let mut call = std::pin::pin!(call);
    let _connection = tokio::select! {
        _ = call.as_mut() => anyhow::bail!("{name} finished without waiting on the registry"),
        accepted = tokio::time::timeout(std::time::Duration::from_secs(30), registry.accept()) =>
            accepted.with_context(|| format!("{name} never reached the registry"))??.0,
    };
    let probe = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    match probe.try_lock_shared() {
        Err(std::fs::TryLockError::WouldBlock) => Ok(()),
        Ok(()) => anyhow::bail!("{name} reached the registry without holding the cache lock"),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// The lock is held, exclusively, for as long as a call may fill the cache.
#[tokio::test]
async fn the_cache_lock_is_held_exclusively_while_a_registry_is_read() -> Result<()> {
    let registry = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address: Registry = registry
        .local_addr()?
        .to_string()
        .parse()
        .context("invalid registry")?;
    let dir = tempfile::tempdir()?;
    let root = root(dir.path())?;
    let mut fetcher = fetcher(dir.path()).await?;
    fetcher.wkg_client_config.set_namespace_registry(
        "contracttest".parse().context("invalid label")?,
        RegistryMapping::Registry(address),
    );
    let lock_path = dir.path().join("cache").join(CACHE_LOCK_FILE);
    let mut lock = load_lock_file(dir.path()).await?;
    assert_lock_held_on_the_network(
        &registry,
        &lock_path,
        fetcher.fetch_wit_dependencies(&root, &mut lock),
        "fetch",
    )
    .await?;
    assert_lock_held_on_the_network(
        &registry,
        &lock_path,
        fetcher.build_wit_package(&root, &mut lock),
        "build",
    )
    .await?;
    let package = "contracttest:api".parse()?;
    assert_lock_held_on_the_network(
        &registry,
        &lock_path,
        fetcher.load_package(&package, None, None),
        "load",
    )
    .await?;
    Ok(())
}

/// Serves `contracttest` over plain HTTP, answering each request path with
/// `respond`; the credentials keep any credential helper out of it.
async fn fake_registry(
    fetcher: &mut WkgFetcher,
    respond: impl Fn(&str) -> (u16, String) + Send + 'static,
) -> Result<()> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let registry: Registry = listener
        .local_addr()?
        .to_string()
        .parse()
        .context("invalid registry")?;
    fetcher.wkg_client_config.set_namespace_registry(
        "contracttest".parse().context("invalid label")?,
        RegistryMapping::Registry(registry.clone()),
    );
    fetcher
        .wkg_client_config
        .get_or_insert_registry_config_mut(&registry)
        .set_backend_config(
            "oci",
            OciRegistryConfig {
                client_config: wasm_pkg_client::oci::client::ClientConfig {
                    protocol: wasm_pkg_client::oci::client::ClientProtocol::Http,
                    ..Default::default()
                },
                credentials: Some(BasicCredentials {
                    username: "test".into(),
                    password: "test".to_string().into(),
                }),
            },
        )?;
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                match stream.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => request.extend_from_slice(buffer.get(..read).unwrap_or_default()),
                }
            }
            let request = String::from_utf8_lossy(&request);
            let (status, body) = respond(request.split_whitespace().nth(1).unwrap_or_default());
            let response = format!(
                "HTTP/1.1 {status} x\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    Ok(())
}

/// Whether some handle other than a new one holds the cache lock at `path`.
fn cache_lock_is_held(path: &Path) -> bool {
    std::fs::File::open(path).is_ok_and(|probe| {
        matches!(
            probe.try_lock_shared(),
            Err(std::fs::TryLockError::WouldBlock)
        )
    })
}

/// Whether the cache lock at `path` is still held after 5 s. A child process that
/// another test is starting can hold an inherited handle to it for a moment.
fn cache_lock_stays_held(path: &Path) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while cache_lock_is_held(path) {
        if std::time::Instant::now() >= deadline {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

/// Each fetch attempt holds the cache lock while it reads the registry, and lets
/// it go while it waits to try again, so other wash processes are not held up.
#[tokio::test]
async fn the_cache_lock_is_released_while_waiting_to_retry() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = root(dir.path())?;
    let mut fetcher = fetcher(dir.path()).await?;
    let lock_path = dir.path().join("cache").join(CACHE_LOCK_FILE);
    let on_the_registry = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    fake_registry(&mut fetcher, {
        let (held, lock_path) = (on_the_registry.clone(), lock_path.clone());
        move |path| match path {
            "/v2/" => (200, String::new()),
            path if path.ends_with("/tags/list") => {
                held.lock().unwrap().push(cache_lock_is_held(&lock_path));
                (503, String::new())
            }
            _ => (404, String::new()),
        }
    })
    .await?;
    let waiting_to_retry = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let _events = on_event({
        let held = waiting_to_retry.clone();
        move |message| {
            if message.contains("retrying") {
                held.lock().unwrap().push(cache_lock_stays_held(&lock_path));
            }
        }
    });
    let mut lock = load_lock_file(dir.path()).await?;
    let fetched = fetcher.fetch_wit_dependencies(&root, &mut lock).await;
    let built = fetcher
        .build_wit_package(&root, &mut lock)
        .await
        .map(|_| ());
    for result in [fetched, built] {
        anyhow::ensure!(
            result
                .as_ref()
                .is_err_and(is_transient_registry_fetch_error),
            "the registry did not fail as a transient error: {result:?}"
        );
    }
    let attempts = 2 * WIT_FETCH_MAX_ATTEMPTS as usize;
    assert_eq!(
        *on_the_registry.lock().unwrap(),
        vec![true; attempts],
        "an attempt read the registry without the cache lock"
    );
    assert_eq!(
        *waiting_to_retry.lock().unwrap(),
        vec![false; attempts - 2],
        "the cache lock was held while waiting to retry"
    );
    Ok(())
}

/// A shared cache this process can write but not lock is left alone: a fetch or
/// build checks a private cache instead of deleting another process's download.
#[cfg(unix)]
#[tokio::test]
async fn a_writable_cache_that_cannot_be_locked_is_not_scrubbed() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir()?;
    let (fetcher, root) = overridden(dir.path()).await?;
    let cache = dir.path().join("cache");
    let lock_file = cache.join(CACHE_LOCK_FILE);
    std::fs::write(&lock_file, b"")?;
    std::fs::set_permissions(&lock_file, std::fs::Permissions::from_mode(0o000))?;
    if std::fs::File::open(&lock_file).is_ok() {
        return Ok(()); // root ignores the mode
    }
    let downloading = cache.join(format!("sha256:{}", "a".repeat(64)));
    std::fs::write(&downloading, b"partial")?;
    let mut lock = load_lock_file(dir.path()).await?;
    fetcher.fetch_wit_dependencies(&root, &mut lock).await?;
    assert!(
        downloading.exists(),
        "a fetch deleted another process's download from a cache it could not lock"
    );
    fetcher.build_wit_package(&root, &mut lock).await?;
    assert!(
        downloading.exists(),
        "a build deleted another process's download from a cache it could not lock"
    );
    Ok(())
}

/// A shared cache this process cannot write is still read: a package it holds
/// is served from it, not fetched again into a private cache.
#[cfg(unix)]
#[tokio::test]
async fn a_read_only_cache_still_serves_the_packages_it_holds() -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir()?;
    let root = root(dir.path())?;
    let elsewhere = tempfile::tempdir()?;
    let (_, _, bytes) = fetcher(elsewhere.path())
        .await?
        .build_wit_package(
            package(dir.path())?,
            &mut load_lock_file(elsewhere.path()).await?,
        )
        .await?;
    let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
    let cache = dir.path().join("cache");
    let mut fetcher = fetcher(dir.path()).await?;
    std::fs::write(cache.join(&digest), &bytes)?;
    std::fs::write(
        cache.join("contracttest:api-1.2.3.json"),
        format!(r#"{{"version":"1.2.3","content_digest":"{digest}"}}"#),
    )?;
    // The registry lists the version but cannot serve it, so only the cache can.
    fake_registry(&mut fetcher, |path| match path {
        "/v2/" => (200, String::new()),
        path if path.ends_with("/tags/list") => (
            200,
            r#"{"name":"contracttest/api","tags":["1.2.3"]}"#.to_string(),
        ),
        _ => (404, String::new()),
    })
    .await?;
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o555))?;
    let _restore = std::mem::DropGuard::new(cache.clone(), |cache| {
        let _ = std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o755));
    });
    if std::fs::write(cache.join("probe"), b"").is_ok() {
        return Ok(()); // root ignores the mode
    }
    let mut lock = load_lock_file(dir.path()).await?;
    fetcher.fetch_wit_dependencies(&root, &mut lock).await?;
    anyhow::ensure!(
        std::fs::read_dir(root.join("deps"))?.next().is_some(),
        "the fetch wrote no dependencies"
    );
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
    let config = WitConfig {
        sources: HashMap::from([("contracttest".into(), "./contract".into())]),
        ..Default::default()
    };
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
    let config = WitConfig {
        sources: HashMap::from([("contracttest:missing".into(), "./missing".into())]),
        ..Default::default()
    };
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

#[tokio::test]
async fn namespace_sources_apply_to_the_packages_a_root_names() -> Result<()> {
    let dir = tempfile::tempdir()?;
    package(dir.path())?;
    let root = root(dir.path())?;
    let mut fetcher = fetcher(dir.path()).await?;
    let config = WitConfig {
        sources: HashMap::from([("contracttest".to_string(), "./contract".to_string())]),
        ..Default::default()
    };
    fetcher.apply_wit_config(&config, dir.path()).await?;
    let mut lock = load_lock_file(dir.path()).await?;
    fetcher.fetch_wit_dependencies(&root, &mut lock).await?;
    assert!(root.join("deps/contracttest-api-1.2.3").is_dir());
    let (name, _, _) = fetcher.build_wit_package(&root, &mut lock).await?;
    assert_eq!(name.to_string(), "app:root");
    assert_eq!(
        fetcher
            .wkg_config
            .overrides
            .as_ref()
            .map(|overrides| overrides.contains_key("contracttest")),
        Some(true)
    );
    Ok(())
}

/// A local namespace source holding `contracttest:api`, whose WIT imports
/// `contracttest:types` from the source's `deps`, and `contracttest:extra` in a subdirectory.
fn namespace_source(dir: &Path) -> Result<PathBuf> {
    let source = dir.join("source");
    for (subdir, wit) in [
        (
            "",
            "package contracttest:api@1.2.3; interface values { use contracttest:types/shapes@1.0.0.{shape}; get: func() -> shape; }",
        ),
        (
            "deps/contracttest-types-1.0.0",
            "package contracttest:types@1.0.0; interface shapes { type shape = u32; }",
        ),
        (
            "extra",
            "package contracttest:extra@2.0.0; interface more { get: func() -> u32; }",
        ),
    ] {
        let path = source.join(subdir);
        std::fs::create_dir_all(&path)?;
        std::fs::write(path.join("package.wit"), wit)?;
    }
    Ok(source)
}

fn namespace_config(key: &str, source: &str) -> WitConfig {
    WitConfig {
        sources: HashMap::from([(key.to_string(), source.to_string())]),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_namespace_source_serves_each_package_from_where_it_lives() -> Result<()> {
    let dir = tempfile::tempdir()?;
    namespace_source(dir.path())?;
    let root = dir.path().join("wit");
    std::fs::create_dir_all(&root)?;
    std::fs::write(
        root.join("root.wit"),
        "package app:root; world service { import contracttest:api/values@1.2.3; import contracttest:extra/more@2.0.0; }",
    )?;
    let mut fetcher = fetcher(dir.path()).await?;
    fetcher
        .apply_wit_config(&namespace_config("contracttest", "./source"), dir.path())
        .await?;
    let mut lock = load_lock_file(dir.path()).await?;
    fetcher.fetch_wit_dependencies(&root, &mut lock).await?;
    for package in [
        "contracttest-api-1.2.3",
        "contracttest-extra-2.0.0",
        "contracttest-types-1.0.0",
    ] {
        assert!(root.join("deps").join(package).is_dir(), "{package}");
    }
    let (name, _, _) = fetcher.build_wit_package(&root, &mut lock).await?;
    assert_eq!(name.to_string(), "app:root");
    for (package, interface) in [
        ("contracttest:extra", "more"),
        ("contracttest:api", "values"),
    ] {
        match fetcher.load_package(&package.parse()?, None, None).await {
            PackageLookup::Found(contents) => assert_eq!(contents.interfaces, [interface]),
            PackageLookup::Missing(message) => anyhow::bail!(message),
            PackageLookup::Failed(error) => return Err(error),
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_namespace_source_does_not_stand_in_for_a_package_it_lacks() -> Result<()> {
    let dir = tempfile::tempdir()?;
    package(dir.path())?;
    let root = dir.path().join("wit");
    std::fs::create_dir_all(&root)?;
    std::fs::write(
        root.join("root.wit"),
        "package app:root; world service { import contracttest:api/values@1.2.3; import contracttest:other/things@1.0.0; }",
    )?;
    let mut fetcher = fetcher(dir.path()).await?;
    fetcher
        .apply_wit_config(&namespace_config("contracttest", "./contract"), dir.path())
        .await?;

    let mut lock = load_lock_file(dir.path()).await?;
    for error in [
        fetcher
            .fetch_wit_dependencies(&root, &mut lock)
            .await
            .err()
            .context("a fetch needing an absent package succeeded")?,
        fetcher
            .build_wit_package(&root, &mut lock)
            .await
            .err()
            .context("a build needing an absent package succeeded")?,
    ] {
        let error = error.to_string();
        assert!(error.contains("[contracttest:other] is not in"), "{error}");
        assert!(error.contains("holds [contracttest:api]"), "{error}");
    }

    match fetcher
        .load_package(&"contracttest:other".parse()?, None, None)
        .await
    {
        PackageLookup::Missing(message) => {
            assert!(
                message.contains("[contracttest:other] is not in"),
                "{message}"
            );
        }
        PackageLookup::Found(contents) => {
            anyhow::bail!(
                "served another package's contents: {:?}",
                contents.interfaces
            )
        }
        PackageLookup::Failed(error) => return Err(error),
    }
    Ok(())
}

#[tokio::test]
async fn a_namespace_source_nothing_uses_is_left_out_of_resolution() -> Result<()> {
    let dir = tempfile::tempdir()?;
    package(dir.path())?;
    let root = root(dir.path())?;
    let mut fetcher = fetcher(dir.path()).await?;
    fetcher.wkg_config.overrides = Some(HashMap::from([
        (
            "contracttest:api".into(),
            wasm_pkg_core::manifest::Override {
                path: Some(dir.path().join("contract")),
                version: None,
            },
        ),
        (
            "unused".into(),
            wasm_pkg_core::manifest::Override {
                path: Some(dir.path().join("absent")),
                version: None,
            },
        ),
    ]));
    let mut lock = load_lock_file(dir.path()).await?;
    fetcher.fetch_wit_dependencies(&root, &mut lock).await?;
    assert!(root.join("deps/contracttest-api-1.2.3").is_dir());
    Ok(())
}

#[tokio::test]
async fn a_package_override_takes_precedence_over_its_namespace() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = package(dir.path())?;
    let root = root(dir.path())?;
    let mut fetcher = fetcher(dir.path()).await?;
    let local = |path: PathBuf| wasm_pkg_core::manifest::Override {
        path: Some(path),
        version: None,
    };
    fetcher.wkg_config.overrides = Some(HashMap::from([
        ("contracttest".into(), local(dir.path().join("absent"))),
        ("contracttest:api".into(), local(path)),
    ]));
    let mut lock = load_lock_file(dir.path()).await?;
    fetcher.fetch_wit_dependencies(&root, &mut lock).await?;
    assert!(root.join("deps/contracttest-api-1.2.3").is_dir());
    Ok(())
}

#[tokio::test]
async fn a_path_override_does_not_serve_a_version_it_lacks() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = package(dir.path())?;
    let mut fetcher = fetcher(dir.path()).await?;
    fetcher.wkg_config.overrides = Some(HashMap::from([(
        "contracttest:api".into(),
        wasm_pkg_core::manifest::Override {
            path: Some(path),
            version: None,
        },
    )]));
    let name: PackageRef = "contracttest:api".parse()?;
    match fetcher.load_package(&name, Some("9.9.9"), None).await {
        PackageLookup::Missing(message) => {
            assert!(message.contains("provides version [1.2.3]"), "{message}");
            assert!(message.contains("requested version [9.9.9]"), "{message}");
        }
        PackageLookup::Found(_) => panic!("a version the override lacks was served"),
        PackageLookup::Failed(error) => panic!("{error:#}"),
    }
    match fetcher.load_package(&name, Some("1.2.3"), None).await {
        PackageLookup::Found(contents) => {
            assert_eq!(contents.version, Some(semver::Version::new(1, 2, 3)));
        }
        PackageLookup::Missing(message) => panic!("{message}"),
        PackageLookup::Failed(error) => panic!("{error:#}"),
    }
    Ok(())
}

#[tokio::test]
async fn looking_up_a_local_package_ignores_overrides_it_does_not_reach() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = package(dir.path())?;
    let mut fetcher = fetcher(dir.path()).await?;
    fetcher.wkg_config.overrides = Some(HashMap::from([
        (
            "contracttest:api".into(),
            wasm_pkg_core::manifest::Override {
                path: Some(path),
                version: None,
            },
        ),
        (
            "contracttest:unrelated".into(),
            wasm_pkg_core::manifest::Override {
                path: Some(dir.path().join("missing")),
                version: None,
            },
        ),
    ]));
    match fetcher
        .load_package(&"contracttest:api".parse()?, Some("1.2.3"), None)
        .await
    {
        PackageLookup::Found(contents) => assert_eq!(contents.interfaces, ["values"]),
        PackageLookup::Missing(message) => panic!("{message}"),
        PackageLookup::Failed(error) => panic!("{error:#}"),
    }
    Ok(())
}

#[test]
fn the_cache_dir_falls_back_to_the_default_wkg_cache() {
    let args = CommonPackageArgs {
        config: None,
        cache: None,
    };
    let expected = std::env::var_os("WKG_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(FileCache::global_cache_path);
    assert_eq!(args.cache_dir().ok(), expected);
}

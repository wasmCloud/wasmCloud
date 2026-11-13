use super::{Coalescer, docker_config_path};
use crate::cache::{CredentialKey, Identity, OciCache};
use anyhow::{Context, Result, anyhow, bail};
use docker_credential::{CredentialRetrievalError, DockerCredential, get_credential};
use oci_client::secrets::RegistryAuth;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, OnceLock, PoisonError},
    time::Duration,
};
use tracing::{debug, instrument, warn};

/// The longest a docker credential lookup may take, including waiting for a
/// helper slot. Past it the pull proceeds anonymously, as for any other
/// failed lookup. `docker_credential` gives no way to stop the helper itself.
const DOCKER_HELPER_TIMEOUT: Duration = Duration::from_secs(30);

/// Credential resolver that implements the precedence: explicit → docker creds → anonymous
pub(super) struct CredentialResolver {
    explicit_credentials: Option<(String, String)>,
}

impl CredentialResolver {
    pub(super) fn new(explicit_credentials: Option<(String, String)>) -> Self {
        Self {
            explicit_credentials,
        }
    }

    /// Resolve credentials for a given registry
    #[instrument(skip(self), fields(registry = %registry))]
    pub(super) async fn resolve_credentials(
        &self,
        registry: &str,
        cache: Arc<dyn OciCache>,
    ) -> RegistryAuth {
        // First, try explicit credentials
        if let Some((username, password)) = &self.explicit_credentials {
            debug!("using explicit credentials");
            return RegistryAuth::Basic(username.clone(), password.clone());
        }

        static HELPERS: OnceLock<Coalescer<(Identity, CredentialKey), RegistryAuth>> =
            OnceLock::new();
        let helpers = HELPERS.get_or_init(|| Coalescer::flights(DOCKER_HELPER_TIMEOUT));
        let key = CredentialKey {
            registry: registry.into(),
            config_path: docker_config_path(),
        };
        // Reuse only saves a helper run. A backend that cannot serve or keep an
        // answer costs that run: it never fails the pull, which falls back to
        // the helper and then to anonymous access, as it always has.
        helpers
            .run((Identity(Arc::clone(&cache)), key.clone()), || async {
                match cache.oci_get_credentials(&key).await {
                    Ok(Some(auth)) => return Ok(auth),
                    Ok(None) => {}
                    Err(error) => warn!(
                        error = format!("{error:#}"),
                        "credential cache lookup failed; asking the docker credential helper"
                    ),
                }
                // Different backends must each perform their own lookup and
                // publication, but may share the same helper process. Otherwise
                // the running-helper guard would reject the second backend.
                static LOOKUPS: OnceLock<Coalescer<CredentialKey, RegistryAuth>> = OnceLock::new();
                let lookups = LOOKUPS.get_or_init(|| Coalescer::flights(DOCKER_HELPER_TIMEOUT));
                let registry = key.registry.clone();
                let lookup_registry = registry.clone();
                let lookup = lookups.run(key.clone(), || run_helper(registry, move || {
                    Ok(Self::get_docker_credentials(&lookup_registry)?
                        .unwrap_or(RegistryAuth::Anonymous))
                }));
                let answer = tokio::time::timeout(DOCKER_HELPER_TIMEOUT, lookup).await
                    .context("docker credential lookup timed out").and_then(|result| result);
                match answer {
                    Ok(auth) => {
                        if let Err(error) = cache.oci_put_credentials(&key, auth.clone()).await {
                            warn!(
                                error = format!("{error:#}"),
                                "credential cache publication failed; continuing without reuse"
                            );
                        }
                        Ok(auth)
                    }
                    Err(error) => {
                        warn!(%error, "failed to retrieve docker credentials; using anonymous access");
                        Ok(RegistryAuth::Anonymous)
                    }
                }
            })
            .await
            .unwrap_or_else(|error| {
                warn!(%error, "failed to retrieve docker credentials; using anonymous access");
                RegistryAuth::Anonymous
            })
    }

    /// Attempt to retrieve credentials from docker credential helper
    fn get_docker_credentials(registry: &str) -> Result<Option<RegistryAuth>> {
        match get_credential(registry) {
            Ok(DockerCredential::UsernamePassword(user, pass)) => {
                Ok(Some(RegistryAuth::Basic(user, pass)))
            }
            Ok(DockerCredential::IdentityToken(_)) => {
                bail!("docker credential helper returned identity token, which is not supported")
            }
            // No credentials configured is the ordinary case for a host that
            // pulls anonymously, not a problem to report.
            Err(
                CredentialRetrievalError::ConfigNotFound
                | CredentialRetrievalError::NoCredentialConfigured,
            ) => Ok(None),
            // `docker_credential` reports an absent `~/.docker/config.json` as
            // `ConfigReadError`, not `ConfigNotFound`: `ConfigNotFound` means
            // there is no home directory at all, while an absent file, an
            // unreadable one, and one that does not parse all flatten into this
            // variant. Only the first is ordinary — a host with `HOME` set and
            // no docker config is the normal case in a container — so the file
            // itself is what separates "nothing to use" from "something is
            // wrong with what you configured".
            Err(CredentialRetrievalError::ConfigReadError) => {
                match docker_config_path() {
                    Some(path) if path.is_file() => bail!(
                        "docker config {} exists but could not be read or parsed",
                        path.display()
                    ),
                    _ => debug!("no docker config; pulling anonymously"),
                }
                Ok(None)
            }
            // Edge case for macOS, shows as an error when really it's just not found
            Err(CredentialRetrievalError::HelperFailure { stdout, .. })
                if stdout.contains("credentials not found in native keychain") =>
            {
                Ok(None)
            }
            Err(e) => Err(anyhow!("docker credential retrieval error: {e}")),
        }
    }
}

/// Coalesce helper lookups in tests, including anonymous results.
/// Failures are retried next time, so a repaired configuration is usable at once.
///
/// Concurrent lookups for one registry share one helper run, taken before any
/// permit, so a registry costs at most one blocking task at a time. Helpers
/// run on their own limiter and under [`DOCKER_HELPER_TIMEOUT`]: one that hangs
/// fails its lookup instead of stalling every pull, and cannot starve client
/// builds or other registries' lookups of permits.
#[cfg(test)]
pub(super) async fn cached_credentials(
    cache: &Coalescer<String, RegistryAuth>,
    registry: String,
    lookup: impl FnOnce() -> Result<RegistryAuth> + Send + 'static,
) -> Result<RegistryAuth> {
    cached_credentials_within(cache, registry, DOCKER_HELPER_TIMEOUT, lookup).await
}

#[cfg(test)]
async fn cached_credentials_within(
    cache: &Coalescer<String, RegistryAuth>,
    registry: String,
    timeout: Duration,
    lookup: impl FnOnce() -> Result<RegistryAuth> + Send + 'static,
) -> Result<RegistryAuth> {
    cache
        .run(registry.clone(), || async move {
            tokio::time::timeout(timeout, run_helper(registry, lookup))
                .await
                .with_context(|| {
                    format!("docker credential lookup did not finish within {timeout:?}")
                })?
        })
        .await
}

/// Registries whose helper is still running. A lookup that times out cannot
/// stop its helper, which runs on in the blocking pool; until it returns,
/// further lookups for that registry fail at once instead of starting another.
/// A hung helper costs one blocking thread per registry, not one per timeout.
static RUNNING_HELPERS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Run `lookup` for `registry` on Tokio's blocking pool.
///
/// The permit bounds the helpers callers are waiting on. The caller holds it,
/// not the blocking task, so a lookup that times out releases it even though
/// its helper runs on: hung helpers cannot exhaust the limiter.
async fn run_helper(
    registry: String,
    lookup: impl FnOnce() -> Result<RegistryAuth> + Send + 'static,
) -> Result<RegistryAuth> {
    static HELPERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let _permit = HELPERS
        .acquire()
        .await
        .context("docker credential helper limiter closed")?;
    let running = HelperRunning::claim(registry)?;
    tokio::task::spawn_blocking(move || {
        let _running = running;
        lookup()
    })
    .await
    .context("docker credential helper task failed")?
}

/// Marks a registry's helper as running until the blocking task drops it.
struct HelperRunning(String);

impl HelperRunning {
    fn claim(registry: String) -> Result<Self> {
        let mut running = RUNNING_HELPERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !running.insert(registry.clone()) {
            bail!("an earlier docker credential helper for {registry} has not finished");
        }
        Ok(Self(registry))
    }
}

impl Drop for HelperRunning {
    fn drop(&mut self) {
        RUNNING_HELPERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    /// Helpers that never return time their lookups out without keeping the
    /// limiter's permits, and are not started again while they run.
    #[tokio::test(start_paused = true)]
    async fn hung_helpers_do_not_exhaust_the_limiter() {
        let cache: &'static Coalescer<String, RegistryAuth> =
            Box::leak(Box::new(Coalescer::flights(Duration::from_secs(60))));
        let runs = Arc::new(AtomicUsize::new(0));
        let mut releases = Vec::new();
        // Start twice the limiter's capacity. Helpers stay blocked after timeout.
        for n in 0..8 {
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let (started, startup) = std::sync::mpsc::channel();
            releases.push(release);
            let runs = Arc::clone(&runs);
            let lookup = tokio::spawn(cached_credentials_within(
                cache,
                format!("hung-{n}"),
                DOCKER_HELPER_TIMEOUT,
                move || {
                    runs.fetch_add(1, Ordering::SeqCst);
                    let _ = started.send(());
                    let _ = wait.recv();
                    Ok(RegistryAuth::Anonymous)
                },
            ));
            // The blocking wait prevents automatic clock advancement during startup.
            tokio::task::spawn_blocking(move || startup.recv_timeout(Duration::from_secs(10)))
                .await
                .expect("startup wait task finishes")
                .expect("helper starts");
            tokio::time::advance(DOCKER_HELPER_TIMEOUT + Duration::from_secs(1)).await;
            let err = lookup.await.expect("lookup task finishes").unwrap_err();
            assert!(err.to_string().contains("did not finish"), "{err:#}");
        }
        assert_eq!(runs.load(Ordering::SeqCst), 8);
        tokio::time::resume();

        // A registry whose helper still runs does not start another.
        let again =
            cached_credentials_within(cache, "hung-0".into(), Duration::from_secs(60), || {
                unreachable!("a second helper started for hung-0")
            })
            .await
            .unwrap_err();
        assert!(again.to_string().contains("has not finished"), "{again:#}");

        // Other registries still get a permit.
        let auth =
            cached_credentials_within(cache, "healthy".into(), Duration::from_secs(60), || {
                Ok(RegistryAuth::Basic("user".into(), "secret".into()))
            })
            .await
            .unwrap();
        assert!(matches!(auth, RegistryAuth::Basic(..)));

        // Once its helper returns, the registry can be looked up again.
        drop(releases);
        tokio::time::timeout(Duration::from_secs(10), async {
            while RUNNING_HELPERS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .any(|registry| registry.starts_with("hung-"))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("released helpers finish");
        cached_credentials_within(cache, "hung-0".into(), Duration::from_secs(60), || {
            Ok(RegistryAuth::Anonymous)
        })
        .await
        .unwrap();
    }
}

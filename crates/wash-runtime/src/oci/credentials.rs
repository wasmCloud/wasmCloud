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

        static FLIGHTS: OnceLock<CredentialFlights> = OnceLock::new();
        let flights = FLIGHTS.get_or_init(|| CredentialFlights::new(DOCKER_HELPER_TIMEOUT));
        let key = CredentialKey {
            registry: registry.into(),
            config_path: docker_config_path(),
        };
        let registry = key.registry.clone();
        cached_credentials(flights, cache, key, DOCKER_HELPER_TIMEOUT, move || {
            Ok(Self::get_docker_credentials(&registry)?.unwrap_or(RegistryAuth::Anonymous))
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

/// Credential lookups in flight.
struct CredentialFlights {
    /// One backend's requests for a registry: the lookup of its cache, the
    /// helper run and the publication of its answer.
    requests: Coalescer<(Identity, CredentialKey), RegistryAuth>,
    /// Helper runs, shared across backends. Each backend looks up and
    /// publishes for itself, but they share one helper process: a second
    /// would be refused while the first runs (see [`RUNNING_HELPERS`]).
    helpers: Coalescer<CredentialKey, RegistryAuth>,
}

impl CredentialFlights {
    fn new(join: Duration) -> Self {
        Self {
            requests: Coalescer::flights(join),
            helpers: Coalescer::flights(join),
        }
    }
}

/// Answer `key` from `cache`, or from a helper lookup whose successful answer,
/// anonymous included, is published to it. Failures are not kept, so a
/// repaired configuration is usable at once.
///
/// Concurrent lookups for one registry share one helper run, taken before any
/// permit, so a registry costs at most one blocking task at a time. Helpers
/// run on their own limiter and under `timeout`: one that hangs fails its
/// lookup instead of stalling every pull, and cannot starve client builds or
/// other registries' lookups of permits.
///
/// Reuse only saves a helper run. A backend that cannot serve or keep an
/// answer costs that run and never fails the lookup.
async fn cached_credentials(
    flights: &CredentialFlights,
    cache: Arc<dyn OciCache>,
    key: CredentialKey,
    timeout: Duration,
    lookup: impl FnOnce() -> Result<RegistryAuth> + Send + 'static,
) -> Result<RegistryAuth> {
    flights
        .requests
        .run((Identity(Arc::clone(&cache)), key.clone()), || async {
            match cache.oci_get_credentials(&key).await {
                Ok(Some(auth)) => return Ok(auth),
                Ok(None) => {}
                Err(error) => warn!(
                    error = format!("{error:#}"),
                    "credential cache lookup failed; asking the docker credential helper"
                ),
            }
            let registry = key.registry.clone();
            let helper = flights
                .helpers
                .run(key.clone(), || run_helper(registry, lookup));
            let auth = tokio::time::timeout(timeout, helper)
                .await
                .with_context(|| {
                    format!("docker credential lookup did not finish within {timeout:?}")
                })??;
            if let Err(error) = cache.oci_put_credentials(&key, auth.clone()).await {
                warn!(
                    error = format!("{error:#}"),
                    "credential cache publication failed; continuing without reuse"
                );
            }
            Ok(auth)
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
    use crate::cache::DefaultOciCache;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn flights() -> &'static CredentialFlights {
        Box::leak(Box::new(CredentialFlights::new(Duration::from_secs(60))))
    }

    fn key(registry: &str) -> CredentialKey {
        CredentialKey {
            registry: registry.into(),
            config_path: None,
        }
    }

    fn default_cache() -> Arc<dyn OciCache> {
        Arc::new(DefaultOciCache::default())
    }

    fn basic() -> RegistryAuth {
        RegistryAuth::Basic("user".into(), "secret".into())
    }

    /// Helpers that never return time their lookups out without keeping the
    /// limiter's permits, and are not started again while they run.
    #[tokio::test(start_paused = true)]
    async fn hung_helpers_do_not_exhaust_the_limiter() {
        let flights = flights();
        let cache = default_cache();
        let runs = Arc::new(AtomicUsize::new(0));
        let mut releases = Vec::new();
        // Start twice the limiter's capacity. Helpers stay blocked after timeout.
        for n in 0..8 {
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let (started, startup) = std::sync::mpsc::channel();
            releases.push(release);
            let runs = Arc::clone(&runs);
            let lookup = tokio::spawn(cached_credentials(
                flights,
                Arc::clone(&cache),
                key(&format!("hung-{n}")),
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
        let again = cached_credentials(
            flights,
            Arc::clone(&cache),
            key("hung-0"),
            Duration::from_secs(60),
            || unreachable!("a second helper started for hung-0"),
        )
        .await
        .unwrap_err();
        assert!(again.to_string().contains("has not finished"), "{again:#}");

        // Other registries still get a permit.
        let auth = cached_credentials(
            flights,
            Arc::clone(&cache),
            key("healthy"),
            Duration::from_secs(60),
            || Ok(basic()),
        )
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
        cached_credentials(
            flights,
            cache,
            key("hung-0"),
            Duration::from_secs(60),
            || Ok(RegistryAuth::Anonymous),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn credential_misses_coalesce_without_blocking_the_runtime() {
        let flights = flights();
        let cache = default_cache();
        let runs = Arc::new(AtomicUsize::new(0));
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let lookup = |key, lookup| {
            cached_credentials(
                flights,
                Arc::clone(&cache),
                key,
                DOCKER_HELPER_TIMEOUT,
                lookup,
            )
        };
        let first = tokio::spawn(lookup(key("coalesced"), {
            let runs = Arc::clone(&runs);
            Box::new(move || {
                runs.fetch_add(1, Ordering::SeqCst);
                let _ = started.send(());
                wait.recv_timeout(Duration::from_secs(2))?;
                Ok(basic())
            }) as Box<dyn FnOnce() -> Result<RegistryAuth> + Send>
        }));
        ready.await.unwrap();
        let second = tokio::spawn(lookup(key("coalesced"), {
            let runs = Arc::clone(&runs);
            Box::new(move || {
                runs.fetch_add(1, Ordering::SeqCst);
                Ok(RegistryAuth::Anonymous)
            })
        }));
        // On a current-thread runtime the second caller cannot join while the
        // helper blocks, unless the helper runs on the bounded blocking pool.
        let request = (Identity(Arc::clone(&cache)), key("coalesced"));
        tokio::time::timeout(Duration::from_secs(1), async {
            while flights.requests.waiters(&request) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the second caller joins while the helper blocks");
        release.send(()).unwrap();
        assert!(matches!(
            first.await.unwrap().unwrap(),
            RegistryAuth::Basic(..)
        ));
        assert!(matches!(
            second.await.unwrap().unwrap(),
            RegistryAuth::Basic(..)
        ));
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // A failure is not kept: the next lookup asks again.
        assert!(
            lookup(key("broken"), Box::new(|| bail!("bad config")))
                .await
                .is_err()
        );
        assert!(
            lookup(key("broken"), Box::new(|| Ok(RegistryAuth::Anonymous)))
                .await
                .is_ok()
        );
    }

    /// A published answer, anonymous included, is served without the helper.
    #[tokio::test]
    async fn a_published_answer_is_reused_without_the_helper() {
        let flights = flights();
        let cache = default_cache();
        for (registry, answer) in [
            ("reused-basic", basic()),
            ("reused-anonymous", RegistryAuth::Anonymous),
        ] {
            let expected = crate::oci::AuthId::of(&answer);
            let first = cached_credentials(
                flights,
                Arc::clone(&cache),
                key(registry),
                DOCKER_HELPER_TIMEOUT,
                move || Ok(answer),
            )
            .await
            .unwrap();
            assert!(crate::oci::AuthId::of(&first) == expected);
            let second = cached_credentials(
                flights,
                Arc::clone(&cache),
                key(registry),
                DOCKER_HELPER_TIMEOUT,
                || unreachable!("a published answer needs no helper"),
            )
            .await
            .unwrap();
            assert!(crate::oci::AuthId::of(&second) == expected);
        }
    }

    /// Two backends each publish for themselves, but share the one helper run
    /// a registry is allowed: the second is not refused as a duplicate.
    #[tokio::test]
    async fn different_backends_share_one_helper_run() {
        let flights = flights();
        let (a, b) = (default_cache(), default_cache());
        let runs = Arc::new(AtomicUsize::new(0));
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let first = tokio::spawn(cached_credentials(
            flights,
            Arc::clone(&a),
            key("shared-helper"),
            DOCKER_HELPER_TIMEOUT,
            {
                let runs = Arc::clone(&runs);
                move || {
                    runs.fetch_add(1, Ordering::SeqCst);
                    let _ = started.send(());
                    wait.recv_timeout(Duration::from_secs(10))?;
                    Ok(basic())
                }
            },
        ));
        ready.await.unwrap();
        let second = tokio::spawn(cached_credentials(
            flights,
            Arc::clone(&b),
            key("shared-helper"),
            DOCKER_HELPER_TIMEOUT,
            || unreachable!("the helper already runs for this registry"),
        ));
        tokio::time::timeout(Duration::from_secs(10), async {
            while flights.helpers.waiters(&key("shared-helper")) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the second backend joins the running helper");
        release.send(()).unwrap();
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        for cache in [a, b] {
            assert!(
                cache
                    .oci_get_credentials(&key("shared-helper"))
                    .await
                    .unwrap()
                    .is_some(),
                "each backend keeps its own answer"
            );
        }
    }

    /// A backend that fails to serve or keep an answer costs a helper run and
    /// never the lookup.
    #[tokio::test]
    async fn a_failing_backend_falls_back_to_the_helper() {
        #[derive(Default)]
        struct Failing {
            asked: AtomicBool,
        }
        #[async_trait::async_trait]
        impl OciCache for Failing {
            async fn oci_get_credentials(&self, _: &CredentialKey) -> Result<Option<RegistryAuth>> {
                self.asked.store(true, Ordering::SeqCst);
                bail!("credential backend unavailable")
            }
            async fn oci_put_credentials(&self, _: &CredentialKey, _: RegistryAuth) -> Result<()> {
                bail!("credential backend unavailable")
            }
        }
        let backend = Arc::new(Failing::default());
        let auth = cached_credentials(
            flights(),
            backend.clone(),
            key("failing-backend"),
            DOCKER_HELPER_TIMEOUT,
            || Ok(basic()),
        )
        .await
        .unwrap();
        assert!(matches!(auth, RegistryAuth::Basic(..)));
        assert!(backend.asked.load(Ordering::SeqCst));
    }
}

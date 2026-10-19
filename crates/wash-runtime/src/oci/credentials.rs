use super::{Coalescer, blocking_on, docker_config_path};
use anyhow::{Context, Result, anyhow, bail};
use docker_credential::{CredentialRetrievalError, DockerCredential, get_credential};
use oci_client::secrets::RegistryAuth;
use std::{sync::OnceLock, time::Duration};
use tracing::{debug, instrument, warn};

/// How long a docker credential lookup answers for its registry. A login or
/// rotation through docker takes effect within this.
const DOCKER_AUTH_REUSE: Duration = Duration::from_secs(30);

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
    pub(super) async fn resolve_credentials(&self, registry: &str) -> RegistryAuth {
        // First, try explicit credentials
        if let Some((username, password)) = &self.explicit_credentials {
            debug!("using explicit credentials");
            return RegistryAuth::Basic(username.clone(), password.clone());
        }

        // Next, try docker credential helper. Its answer is reused for a
        // short while: asking means reading the docker config and possibly
        // running a helper process, which once per workload start is a
        // process spawn per workload on a host restoring thousands.
        static DOCKER_AUTH: OnceLock<Coalescer<String, RegistryAuth>> = OnceLock::new();
        let docker_auth = DOCKER_AUTH.get_or_init(|| {
            Coalescer::with_min_weight(64, DOCKER_HELPER_TIMEOUT, DOCKER_AUTH_REUSE, 1, |_| 1)
        });
        let registry = registry.to_owned();
        cached_credentials(docker_auth, registry.clone(), move || {
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

/// Cache and coalesce successful helper lookups, including anonymous results.
/// Failures are retried next time, so a repaired configuration is usable at once.
///
/// Concurrent lookups for one registry share one helper run, taken before any
/// permit, so a registry costs at most one blocking task at a time. Helpers
/// run on their own limiter and under [`DOCKER_HELPER_TIMEOUT`]: one that hangs
/// fails its lookup instead of stalling every pull, and cannot starve client
/// builds of permits.
pub(super) async fn cached_credentials(
    cache: &Coalescer<String, RegistryAuth>,
    registry: String,
    lookup: impl FnOnce() -> Result<RegistryAuth> + Send + 'static,
) -> Result<RegistryAuth> {
    static HELPERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    cache
        .run_with_reuse(registry, true, || async move {
            tokio::time::timeout(DOCKER_HELPER_TIMEOUT, blocking_on(&HELPERS, lookup))
                .await
                .with_context(|| {
                    format!(
                        "docker credential lookup did not finish within {DOCKER_HELPER_TIMEOUT:?}"
                    )
                })?
        })
        .await
}

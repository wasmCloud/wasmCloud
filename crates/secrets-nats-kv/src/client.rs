use std::collections::HashSet;

use anyhow::{bail, ensure, Context};
use async_nats::jetstream;
use wasmcloud_secrets_types::Secret;

pub const SECRETS_API_VERSION: &str = "v1alpha1";

use crate::{find_key_rev, PutSecretError, PutSecretRequest, PutSecretResponse};

/// Helper function wrapper around [`put_secret`] that allows putting multiple secrets in the secret store.
/// See the documentation for [`put_secret`] for more information.
///
/// Returns a Vec of results, one for each secret put operation.
pub async fn put_secrets(
    nats_client: &async_nats::Client,
    subject_base: &str,
    transit_xkey: &nkeys::XKey,
    secrets: Vec<PutSecretRequest>,
) -> Vec<anyhow::Result<()>> {
    futures::future::join_all(
        secrets
            .into_iter()
            .map(|s| async move { put_secret(nats_client, subject_base, transit_xkey, s).await }),
    )
    .await
}

/// Put a secret in the NATS KV backed secret store
///
/// # Arguments
/// - `nats_client` - the NATS client connected to a server that the secret store is listening on
/// - `subject_base` - the base subject to use for requests to the secret store
/// - `transit_xkey` - the transit key to use to encrypt the secret. Can be constructed from a seed or public key
/// - `secret` - the secret to put in the store
pub async fn put_secret(
    nats_client: &async_nats::Client,
    subject_base: &str,
    transit_xkey: &nkeys::XKey,
    secret: PutSecretRequest,
) -> anyhow::Result<()> {
    ensure!(
        !(secret.binary_secret.is_some() && secret.string_secret.is_some()),
        "secret cannot have both binary and string values"
    );

    let request_xkey = nkeys::XKey::new();
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(
        wasmcloud_secrets_types::WASMCLOUD_HOST_XKEY,
        request_xkey
            .public_key()
            .parse::<async_nats::HeaderValue>()
            .context("could not parse request xkey public key as header value")?,
    );

    let value = serde_json::to_string(&secret).context("failed to serialize secret to string")?;
    let v = request_xkey
        .seal(value.as_bytes(), transit_xkey)
        .expect("should be able to encrypt the secret");
    let response = nats_client
        .request_with_headers(
            format!("{subject_base}.{SECRETS_API_VERSION}.nats-kv.put_secret"),
            headers,
            v.into(),
        )
        .await?;

    let put_secret_response = serde_json::from_slice::<PutSecretResponse>(&response.payload)
        .context("failed to deserialize put secret response")?;
    put_secret_response.error.map_or(Ok(()), |e| match e {
        PutSecretError::DecryptionError => Err(anyhow::anyhow!(e)
            .context("Error decrypting secret. Ensure the transit xkey is the same as the one provided to the backend")),
        _ => Err(anyhow::anyhow!(e)),
    })
}

/// Get a secret from the NATS KV backed secret store. This function directly requests the secret from the KV store
/// and decrypts it using the provided encryption key. Notably, this does not check if the requesting entity is allowed
/// to access the secret, as owning the encryption key implies that the caller is trusted.
///
/// # Arguments
/// - `nats_client` - the NATS client connected to a server that the secret store is accessible on, jetstream enabled
/// - `secret_bucket_name` - the name of the secret bucket to use fetch secrets from
/// - `encryption_xkey` - the encryption key to use to decrypt the secret. Must be constructed from a seed key
/// - `name` - the name of the secret to get
/// - `version` - the version of the secret to get
pub async fn get_secret(
    nats_client: &async_nats::Client,
    secret_bucket_name: &str,
    encryption_xkey: &nkeys::XKey,
    name: &str,
    version: Option<&str>,
) -> anyhow::Result<Secret> {
    let js = jetstream::new(nats_client.clone());
    let secrets = js.get_key_value(secret_bucket_name).await?;

    let entry = match &version {
        Some(v) => {
            let revision = str::parse::<u64>(v)
                .context("invalid version format - must be a positive integer")?;

            let mut key_hist = secrets
                .history(&name)
                .await
                .with_context(|| format!("failed to get history for secret '{name}'"))?;
            find_key_rev(&mut key_hist, revision).await
        }
        None => secrets
            .entry(name)
            .await
            .with_context(|| format!("failed to get latest version of secret '{name}'"))?,
    };

    let Some(entry) = entry else {
        bail!("secret not found in KV store")
    };

    let mut secret = Secret {
        version: entry.revision.to_string(),
        ..Default::default()
    };

    let decrypted = encryption_xkey
        .open(&entry.value, encryption_xkey)
        .context("failed to decrypt secret: ensure the encryption key is correct")?;

    match String::from_utf8(decrypted) {
        Ok(s) => {
            secret.string_secret = Some(s);
        }
        Err(_) => {
            secret.binary_secret = Some(entry.value.to_vec());
        }
    };

    Ok(secret)
}

/// Add the allowed secrets a given public key is allowed to access
///
/// # Arguments
/// - `nats_client` - the NATS client connected to a server that the secret store is listening on
/// - `subject_base` - the base subject to use for requests to the secret store
/// - `public_key` - the identity public key of the entity that is allowed to access the secrets
/// - `secrets` - the names of the secrets that the public key is allowed to access
pub async fn add_mapping(
    nats_client: &async_nats::Client,
    subject_base: &str,
    public_key: &str,
    secrets: HashSet<String>,
) -> anyhow::Result<()> {
    ensure!(!subject_base.is_empty(), "subject base cannot be empty");
    ensure!(!public_key.is_empty(), "subject base cannot be empty");

    nats_client
        .request(
            format!("{subject_base}.{SECRETS_API_VERSION}.nats-kv.add_mapping.{public_key}"),
            serde_json::to_vec(&secrets)
                .context("failed to serialize set of secrets")?
                .into(),
        )
        .await?;

    Ok(())
}

/// Remove allowed secrets a given public key is allowed to access
///
/// # Arguments
/// - `nats_client` - the NATS client connected to a server that the secret store is listening on
/// - `subject_base` - the base subject to use for requests to the secret store
/// - `public_key` - the identity public key of the entity that is allowed to access the secrets
/// - `secrets` - the names of the secrets that the public key is allowed to access
pub async fn remove_mapping(
    nats_client: &async_nats::Client,
    subject_base: &str,
    public_key: &str,
    secrets: HashSet<String>,
) -> anyhow::Result<()> {
    ensure!(!subject_base.is_empty(), "subject base cannot be empty");
    ensure!(!public_key.is_empty(), "subject base cannot be empty");

    nats_client
        .request(
            format!("{subject_base}.{SECRETS_API_VERSION}.nats-kv.remove_mapping.{public_key}"),
            serde_json::to_vec(&secrets)
                .context("failed to serialize set of secrets")?
                .into(),
        )
        .await?;

    Ok(())
}

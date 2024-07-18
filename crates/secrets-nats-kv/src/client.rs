use anyhow::{ensure, Context};

/// Helper function wrapper around [`put_secret`] that allows putting multiple secrets in the secret store.
/// See the documentation for [`put_secret`] for more information.
///
/// Returns a Vec of results, one for each secret put operation.
pub async fn put_secrets(
    nats_client: &async_nats::Client,
    subject_base: &str,
    transit_xkey: &nkeys::XKey,
    secrets: Vec<wasmcloud_secrets_types::Secret>,
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
    secret: wasmcloud_secrets_types::Secret,
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
    nats_client
        .request_with_headers(
            format!("{subject_base}.v0.nats-kv.put_secret"),
            headers,
            v.into(),
        )
        .await?;

    Ok(())
}

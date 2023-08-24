// Adapted from
// https://github.com/wasmCloud/wasmcloud-otp/blob/5f13500646d9e077afa1fca67a3fe9c8df5f3381/host_core/native/hostcore_wasmcloud_native/src/client.rs

use std::env;
use std::env::consts::{ARCH, OS};
use std::path::PathBuf;
use std::str;
use std::sync::Arc;

use anyhow::{bail, Context};
use async_trait::async_trait;
use bindle::cache::DumbCache;
use bindle::client::tokens::{HttpBasic, LongLivedToken, NoToken, TokenManager};
use bindle::client::{self, Client, ClientBuilder};
use bindle::filters::BindleFilter;
use bindle::provider::file::FileProvider;
use bindle::provider::Provider;
use bindle::search::NoopEngine;
use bindle::signature::{KeyRing, KeyRingLoader, KeyRingSaver};
use bindle::{Invoice, SignatureRole, VerificationStrategy};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tracing::warn;
use wascap::jwt;

use crate::par;
use crate::registry::{Auth as RegistryAuth, Settings as RegistrySettings};

const BINDLE_URL_ENV: &str = "BINDLE_URL";
const BINDLE_KEYRING_PATH: &str = "BINDLE_KEYRING_PATH";

const DEFAULT_BINDLE_URL: &str = "http://localhost:8080/v1/";
const CACHE_DIR: &str = "wasmcloud_bindlecache";
const KEYRING_FILE: &str = "keyring.toml";

/// Authentication method
#[derive(Clone)]
pub enum Auth {
    /// No authentication
    None(NoToken),
    /// HTTP authentication
    Http(HttpBasic),
    /// Token authentication
    LongLived(LongLivedToken),
}

#[async_trait]
impl TokenManager for Auth {
    async fn apply_auth_header(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> client::Result<reqwest::RequestBuilder> {
        match &self {
            Auth::None(nt) => nt.apply_auth_header(builder).await,
            Auth::Http(h) => h.apply_auth_header(builder).await,
            Auth::LongLived(l) => l.apply_auth_header(builder).await,
        }
    }
}

fn get_bindle_auth(registry_settings: &RegistrySettings) -> Auth {
    match &registry_settings.auth {
        RegistryAuth::Basic(username, password) => Auth::Http(HttpBasic::new(username, password)),
        RegistryAuth::Token(token) => Auth::LongLived(LongLivedToken::new(token)),
        RegistryAuth::Anonymous => Auth::None(NoToken),
    }
}

/// Returns a bindle client configured to cache to disk
#[allow(clippy::missing_errors_doc)] // TODO: document errors
pub async fn get_client(
    bindle_id: &str,
    registry_settings: &RegistrySettings,
) -> anyhow::Result<DumbCache<FileProvider<NoopEngine>, Client<Auth>>> {
    let auth = get_bindle_auth(registry_settings);

    // Make sure the cache dir exists
    let temp_dir = std::env::temp_dir();
    let bindle_dir = temp_dir.join(CACHE_DIR);

    let keyring_path = if let Ok(bindle_keyring_path) = env::var(BINDLE_KEYRING_PATH) {
        PathBuf::from(bindle_keyring_path)
    } else {
        bindle_dir.join(KEYRING_FILE)
    };
    tokio::fs::create_dir_all(&bindle_dir).await?;
    let bindle_url = extract_server(bindle_id);
    let keyring: KeyRing = match keyring_path.load().await {
        Ok(k) => k,
        Err(e) => {
            warn!("got error when trying to load keyring: {e}\n\n Attempting to fetch host keys from server");
            let client = Client::new(&bindle_url, auth.clone(), Arc::new(KeyRing::default()))?;

            let k = client
                .get_host_keys()
                .await
                .context("failed to fetch host keys for validation and no keyring was provided")?;
            if let Err(e) = keyring_path.save(&k).await {
                warn!("unable to save fetched host keys to {}. Will continue with keyring in memory: {e}", keyring_path.display());
            }
            k
        }
    };
    let client = ClientBuilder::default()
        .verification_strategy(VerificationStrategy::MultipleAttestation(vec![
            SignatureRole::Host,
        ]))
        .build(&bindle_url, auth, std::sync::Arc::new(keyring))?;
    let local = FileProvider::new(bindle_dir, NoopEngine::default()).await;
    Ok(DumbCache::new(client, local))
}

// By the time the bindle ID gets here, if it's in "secure registry" form (invoice@server)
fn extract_server(bindle_id: &str) -> String {
    let parts: Vec<_> = bindle_id.split('@').collect();
    if parts.len() == 2 {
        parts[1].to_owned()
    } else {
        env::var(BINDLE_URL_ENV).unwrap_or_else(|_| DEFAULT_BINDLE_URL.to_owned())
    }
}

// If the bindle ID is in "secure registry" form, just take the invoice portion of invoice@server
pub(crate) fn normalize_bindle_id(bindle_id: &str) -> String {
    let parts: Vec<_> = bindle_id.split('@').collect();
    if parts.len() == 2 {
        parts[0].to_owned()
    } else {
        bindle_id.to_owned()
    }
}

/// Fetch actor from bindle
#[allow(clippy::missing_errors_doc)] // TODO: document errors
pub async fn fetch_actor(
    bindle_id: impl AsRef<str>,
    registry_settings: &RegistrySettings,
) -> anyhow::Result<Vec<u8>> {
    let bindle_id = bindle_id.as_ref();
    // Get the invoice, validate this bindle contains an actor, fetch the actor and return
    let client = get_client(bindle_id, registry_settings)
        .await
        .context("failed to get client")?;

    let bindle_id = normalize_bindle_id(bindle_id);
    let Invoice { bindle, parcel, .. } = client
        .get_invoice(bindle_id)
        .await
        .context("failed to get invoice")?;

    // TODO: We may want to allow more than one down the line, or include the JWT separately as
    // part of the bindle. For now we just expect the single parcel
    let Some([parcel]) = parcel.as_deref() else {
        bail!("actor bindle should contain exactly one parcel")
    };
    let mut stream = client
        .get_parcel(&bindle.id, &parcel.label.sha256)
        .await
        .context("failed to get parcel")?;
    let mut data = Vec::new();
    while let Some(res) = stream.next().await {
        let bytes = res?;
        data.extend(bytes);
    }
    Ok(data)
}

/// Fetch provider from bindle
#[allow(clippy::missing_errors_doc)] // TODO: document errors
pub async fn fetch_provider(
    bindle_id: impl AsRef<str>,
    link_name: impl AsRef<str>,
    registry_settings: &RegistrySettings,
) -> anyhow::Result<(PathBuf, jwt::Claims<jwt::CapabilityProvider>)> {
    let bindle_id = bindle_id.as_ref();
    let client = get_client(bindle_id, registry_settings)
        .await
        .context("failed to construct client")?;
    let bindle_id = normalize_bindle_id(bindle_id);
    // Get the invoice first
    let inv = client
        .get_invoice(bindle_id)
        .await
        .context("failed to get invoice")?;

    // Now filter to figure out which parcels to get (should only get the claims and the provider based on arch)
    let parcels = BindleFilter::new(&inv)
        .activate_feature("wasmcloud", "arch", ARCH)
        .activate_feature("wasmcloud", "os", OS)
        .filter();
    let (claims, provider) = match parcels.as_slice() {
        [claims, provider] | [provider, claims] if claims.label.name == "claims.jwt" => {
            (claims, provider)
        }
        _ => bail!("invalid bindle"),
    };

    let claims = {
        let mut stream = client
            .get_parcel(&inv.bindle.id, &claims.label.sha256)
            .await
            .context("failed to get parcel")?;
        let mut data = Vec::new();
        while let Some(res) = stream.next().await {
            let bytes = res?;
            data.extend(bytes);
        }
        let data = str::from_utf8(&data).context("invalid UTF-8 data in claims")?;
        jwt::Claims::decode(data)?
    };

    let exe = par::cache_path(&claims, link_name);
    // Now get the parcel (if it doesn't already exist on disk)
    if let Some(mut file) = par::create(&exe).await? {
        let mut written = 0;
        let mut stream = client
            .get_parcel(&inv.bindle.id, &provider.label.sha256)
            .await
            .context("failed to get parcel")?;
        while let Some(res) = stream.next().await {
            let bytes = res?;
            written += bytes.len();
            file.write_all(&bytes).await.context("failed to write")?;
        }
        file.flush().await.context("failed to flush")?;
        if written == 0 {
            bail!("provider parcel not found or was empty");
        }
    }
    Ok((exe, claims))
}

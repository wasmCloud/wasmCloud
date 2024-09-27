use async_nats::{
    jetstream::{
        self,
        context::KeyValueError,
        kv::{Config, Entry, History, Store},
        publish::PublishAck,
        response::Response,
        stream::{Config as StreamConfig, DiscardPolicy, StorageType},
    },
    Message, Subject,
};
use async_trait::async_trait;
use backoff::{future::retry, Error as BackoffError, ExponentialBackoffBuilder};
use bytes::Bytes;
use futures::StreamExt;
use nkeys::XKey;
use std::{collections::HashSet, time::Duration};
use tracing::{debug, error, info, warn};
use wascap::jwt::{CapabilityProvider, Host};
use wascap::prelude::{validate_token, Claims, Component};
use wasmcloud_secrets_types::*;

use crate::types::*;

const OPERATION_INDEX: usize = 3;

/// The `Api` struct implements the functionality of this secrets backend.
pub struct Api {
    /// The server's public XKey, used to decrypt secrets sent to the server.
    server_transit_xkey: XKey,
    /// The encryption key used to encrypt secrets in NATS KV.
    /// This _must_ always be the same value after the first time a secret is written otherwise you
    /// will *not* able to decrypt it!
    encryption_xkey: XKey,
    /// The NATS client used to communicate with wasmCloud hosts and the KV backend.
    pub client: async_nats::Client,
    /// The base subject for all secrets operations. Should default to `wasmcloud.secrets`.
    subject_base: String,
    /// The name of this provider. It must be unique for every {subject_base} + name combination.
    pub name: String,
    /// The KV bucket used to store secrets.
    pub bucket: String,
    /// The maximum number of revisions to keep for each secret.
    max_secret_history: usize,
    /// The prefix to use for the name of the queue subscription group that this backend belongs
    /// to.
    queue_base: String,
    /// The version of the secrets API that this backend implements.
    api_version: String,
}

impl Api {
    // The name of the queue group to use for this backend
    fn queue_name(&self) -> String {
        format!("{}.{}", self.queue_base, self.name)
    }

    /// The name of the stream used to coordinate write access to the KV bucket.
    fn lock_stream_name(&self) -> String {
        format!("SECRETS_{}_state_lock", self.name)
    }

    pub fn subject(&self) -> String {
        format!("{}.{}.{}", self.subject_base, self.api_version, self.name)
    }

    pub fn state_bucket_name(&self) -> String {
        format!("SECRETS_{}_state", self.name)
    }

    /// Retrieve the state bucket used to store mappings of entities to secrets.
    async fn state_bucket(&self) -> anyhow::Result<Store, KeyValueError> {
        let js = jetstream::new(self.client.clone());
        js.get_key_value(self.state_bucket_name()).await
    }

    /// Retrieve the lock stream used to coordinate write access to the state bucket.
    async fn ensure_state_lock_stream(&self) -> anyhow::Result<()> {
        let name = self.lock_stream_name();
        let js = jetstream::new(self.client.clone());
        js.get_or_create_stream(StreamConfig {
            name: name.clone(),
            description: Some("Lock stream for secrets state".to_string()),
            discard: DiscardPolicy::New,
            discard_new_per_subject: true,
            storage: StorageType::Memory,
            max_messages_per_subject: 1,
            max_age: Duration::from_secs(3),
            subjects: vec![format!("{}.*", self.lock_stream_name())],
            ..Default::default()
        })
        .await?;
        Ok(())
    }

    async fn handle_put_secret(&self, msg: &Message, reply: Subject) {
        let js = jetstream::new(self.client.clone());
        let payload = &msg.payload;
        if payload.is_empty() {
            let _ = self
                .client
                .publish(
                    reply,
                    PutSecretResponse::from(PutSecretError::InvalidPayload).into(),
                )
                .await;
            return;
        }

        if msg.headers.is_none() {
            let _ = self
                .client
                .publish(
                    reply,
                    PutSecretResponse::from(PutSecretError::InvalidHeaders).into(),
                )
                .await;
            return;
        }

        // SAFETY: We just checked that headers is not None above
        let headers = &msg.headers.clone().unwrap();
        let host_key = match headers.get(WASMCLOUD_HOST_XKEY) {
            None => {
                let _ = self
                    .client
                    .publish(
                        reply,
                        PutSecretResponse::from(PutSecretError::InvalidXKey).into(),
                    )
                    .await;
                return;
            }
            Some(key) => key,
        };

        let k = XKey::from_public_key(host_key.as_str()).unwrap();
        let payload = match self.server_transit_xkey.open(payload, &k) {
            Ok(p) => p,
            Err(_e) => {
                let _ = self
                    .client
                    .publish(
                        reply,
                        PutSecretResponse::from(PutSecretError::DecryptionError).into(),
                    )
                    .await;
                return;
            }
        };

        let secret: PutSecretRequest = match serde_json::from_slice(&payload) {
            Ok(s) => s,
            Err(e) => {
                let _ = self.client.publish(reply, e.to_string().into()).await;
                return;
            }
        };

        let store = match js.get_key_value(&self.bucket).await {
            Ok(s) => s,
            Err(e) => {
                let _ = self.client.publish(reply, e.to_string().into()).await;
                return;
            }
        };

        let encrypted_value = if let Some(s) = secret.string_secret {
            self.encryption_xkey
                .seal(s.as_bytes(), &self.encryption_xkey)
                .unwrap()
        } else if let Some(b) = secret.binary_secret {
            self.encryption_xkey
                .seal(&b, &self.encryption_xkey)
                .unwrap()
        } else {
            let _ = self
                .client
                .publish(
                    reply,
                    PutSecretResponse::from(PutSecretError::InvalidPayload).into(),
                )
                .await;
            return;
        };

        match store.put(secret.key, encrypted_value.into()).await {
            Ok(revision) => {
                let resp = PutSecretResponse::from(revision);
                let _ = self
                    .client
                    .publish(reply, serde_json::to_string(&resp).unwrap().into())
                    .await;
            }
            Err(e) => {
                let _ = self.client.publish(reply, e.to_string().into()).await;
            }
        };
    }

    async fn handle_get_secret(&self, msg: &Message, reply: Subject) {
        let payload = msg.payload.clone();
        if payload.is_empty() {
            let _ = self
                .client
                .publish(
                    reply,
                    SecretResponse::from(GetSecretError::InvalidPayload).into(),
                )
                .await;
            return;
        }

        if msg.headers.is_none() {
            let _ = self
                .client
                .publish(
                    reply,
                    SecretResponse::from(GetSecretError::InvalidHeaders).into(),
                )
                .await;
            return;
        }

        let headers = msg.headers.clone().unwrap();
        let host_key = match headers.get(WASMCLOUD_HOST_XKEY) {
            None => {
                let _ = self
                    .client
                    .publish(
                        reply,
                        SecretResponse::from(GetSecretError::InvalidXKey).into(),
                    )
                    .await;
                return;
            }
            Some(key) => key,
        };

        let k = XKey::from_public_key(host_key.as_str()).unwrap();
        let payload = match self.server_transit_xkey.open(&payload, &k) {
            Ok(p) => p,
            Err(_e) => {
                let _ = self
                    .client
                    .publish(
                        reply,
                        SecretResponse::from(GetSecretError::DecryptionError).into(),
                    )
                    .await;
                return;
            }
        };
        let secret_req: SecretRequest = match serde_json::from_slice(&payload) {
            Ok(r) => r,
            Err(_) => {
                let _ = self
                    .client
                    .publish(
                        reply,
                        SecretResponse::from(GetSecretError::InvalidRequest).into(),
                    )
                    .await;
                return;
            }
        };

        let response = self.get(secret_req).await;
        match response {
            Ok(resp) => {
                let encoded: Bytes = resp.into();
                let encryption_key = XKey::new();
                let encrypted = match encryption_key.seal(&encoded, &k) {
                    Ok(e) => e,
                    Err(_e) => {
                        let _ = self
                            .client
                            .publish(
                                reply,
                                SecretResponse::from(GetSecretError::EncryptionError).into(),
                            )
                            .await;
                        return;
                    }
                };

                let mut headers = async_nats::HeaderMap::new();
                headers.insert(RESPONSE_XKEY, encryption_key.public_key().as_str());

                let _ = self
                    .client
                    .publish_with_headers(reply, headers, encrypted.into())
                    .await;
            }
            Err(e) => {
                let _ = self
                    .client
                    .publish(reply, SecretResponse::from(e).into())
                    .await;
            }
        }
    }

    /// Run the secrets backend. This function will block until the NATS connection is closed.
    pub async fn run(&self) -> anyhow::Result<()> {
        let queue_name = self.queue_name();
        let subject = format!("{}.>", self.subject());
        info!(subject, "Starting listener");
        let mut sub = self
            .client
            .queue_subscribe(subject.clone(), queue_name)
            .await?;

        let js = jetstream::new(self.client.clone());
        let _store = match js.get_key_value(&self.bucket).await {
            Ok(s) => s,
            Err(e) => {
                if e.kind() == jetstream::context::KeyValueErrorKind::GetBucket {
                    js.create_key_value(Config {
                        bucket: self.bucket.clone(),
                        description: "Secrets store".to_string(),
                        compression: true,
                        history: self.max_secret_history as i64,
                        ..Default::default()
                    })
                    .await?
                } else {
                    return Err(e.into());
                }
            }
        };

        match self.state_bucket().await {
            Ok(s) => s,
            Err(e) => {
                if e.kind() == jetstream::context::KeyValueErrorKind::GetBucket {
                    js.create_key_value(Config {
                        bucket: self.state_bucket_name(),
                        description: "Secrets state store".to_string(),
                        compression: true,
                        ..Default::default()
                    })
                    .await?
                } else {
                    return Err(e.into());
                }
            }
        };

        self.ensure_state_lock_stream().await?;

        while let Some(msg) = sub.next().await {
            let reply = match &msg.reply {
                Some(reply) => reply.clone(),
                None => continue,
            };

            let parts: Vec<&str> = msg
                .subject
                .trim_start_matches(&self.subject_base)
                .split('.')
                .collect();
            if parts.len() < OPERATION_INDEX + 1 {
                let _ = self.client.publish(reply, "invalid subject".into()).await;
                continue;
            }
            let op = parts[OPERATION_INDEX];

            // Match the operation to perform and actually call the underlying handler.
            // Errors should be returned to the caller.
            match op {
                "server_xkey" => {
                    let _ = self
                        .client
                        .publish(reply, self.server_xkey().public_key().into())
                        .await;
                }
                "get" => {
                    self.handle_get_secret(&msg, reply).await;
                }
                // Custom handlers
                // These handlers are not part of the wasmCloud secrets spec, but are provided in
                // in order to extend the secrets backend in order to make it usable.
                "add_mapping" => {
                    let entity = match parts.get(OPERATION_INDEX + 1) {
                        Some(e) => e,
                        None => {
                            let _ = self
                                .client
                                .publish(reply, "no entity provided".into())
                                .await;
                            continue;
                        }
                    };

                    let payload = msg.payload;
                    let values: HashSet<String> = match serde_json::from_slice(&payload) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = self.client.publish(reply, e.to_string().into()).await;
                            continue;
                        }
                    };
                    match self.add_mapping(entity.to_string(), values).await {
                        Ok(_) => {
                            let _ = self.client.publish(reply, "ok".into()).await;
                        }
                        Err(e) => {
                            let _ = self.client.publish(reply, e.to_string().into()).await;
                        }
                    }
                }
                "remove_mapping" => {
                    let entity = match parts.get(OPERATION_INDEX + 1) {
                        Some(e) => e,
                        None => {
                            let _ = self
                                .client
                                .publish(
                                    reply,
                                    "no provider or component public key provided".into(),
                                )
                                .await;
                            continue;
                        }
                    };

                    let payload = msg.payload;
                    let values: HashSet<String> = match serde_json::from_slice(&payload) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = self.client.publish(reply, e.to_string().into()).await;
                            continue;
                        }
                    };
                    match self.remove_mapping(entity.to_string(), values).await {
                        Ok(_) => {
                            let _ = self.client.publish(reply, "ok".into()).await;
                        }
                        Err(e) => {
                            let _ = self.client.publish(reply, e.to_string().into()).await;
                        }
                    }
                }
                "put_secret" => {
                    self.handle_put_secret(&msg, reply).await;
                }
                o => {
                    let _ = self
                        .client
                        .publish(reply, format!("unknown operation {o}").into())
                        .await;
                }
            }
        }

        Ok(())
    }

    async fn get_lock(&self, subject: String) -> anyhow::Result<PublishAck> {
        // TODO: make this all configurable
        let exp = ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_millis(100))
            .with_max_elapsed_time(Some(Duration::from_secs(3)))
            .build();

        let op = || async {
            let resp = self
                .client
                .request(subject.clone(), "lock".into())
                .await
                .map_err(|e| {
                    debug!("Error locking state stream: {}", e);
                    BackoffError::transient("")
                })?;
            match serde_json::from_slice(&resp.payload) {
                Ok(Response::Ok(p)) => Ok(p),
                Ok(Response::Err { error: e }) => {
                    debug!("Error locking state stream: {:?}", e);
                    Err(BackoffError::transient("unable to get lock"))
                }
                Err(e) => {
                    error!("Error locking state stream: {}", e);
                    Err(BackoffError::permanent("error publishing message"))
                }
            }
        };

        let result = retry(exp, op).await;
        result.map_err(|_e| anyhow::anyhow!("timed out getting lock"))
    }

    // TODO: add a way to specify labels that should apply to this mapping. That way you can
    // provide host labels that should grant an entity access to a secret.
    async fn add_mapping(&self, entity: String, values: HashSet<String>) -> anyhow::Result<()> {
        let c = jetstream::new(self.client.clone());
        let subject = format!("{}.{}", self.lock_stream_name(), entity);

        let ack = self.get_lock(subject.clone()).await?;
        let seq = ack.sequence;
        let state = self.state_bucket().await?;
        let entry = state.get(&entity).await?;
        if let Some(e) = entry {
            let mut stored_values: HashSet<String> = serde_json::from_slice(&e)?;
            stored_values.extend(values.clone());
            let str = serde_json::to_string(&stored_values)?;
            state.put(entity.clone(), str.into()).await?;
        } else {
            let str = serde_json::to_string(&values)?;
            state.put(entity.clone(), str.into()).await?;
        }
        let s = c.get_stream(&self.lock_stream_name()).await?;
        s.delete_message(seq).await?;
        Ok(())
    }

    async fn remove_mapping(&self, entity: String, values: HashSet<String>) -> anyhow::Result<()> {
        let c = jetstream::new(self.client.clone());
        let subject = format!("{}.{}", self.lock_stream_name(), entity);

        let ack = self.get_lock(subject.clone()).await?;
        let seq = ack.sequence;
        let state = self.state_bucket().await?;
        let entry = state.get(&entity).await?;
        let mut map: HashSet<String> = match entry {
            Some(e) => serde_json::from_slice(&e)?,
            None => HashSet::new(),
        };

        if !map.contains(&entity) {
            let s = c.get_stream(&self.lock_stream_name()).await?;
            s.delete_message(seq).await?;
            return Ok(());
        }

        map.retain(|v| !values.contains(v));
        let new_vals = serde_json::to_string(&map)?;
        state.put(entity.clone(), new_vals.into()).await?;

        // TODO all this locking logic should probably be wrapped up into some sort of a wrapper
        // struct that does the right thing when it goes out of scope
        let s = c.get_stream(&self.lock_stream_name()).await?;
        s.delete_message(seq).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        server_xkey: XKey,
        encryption_xkey: XKey,
        client: async_nats::Client,
        subject_base: String,
        name: String,
        bucket: String,
        max_secret_history: usize,
        queue_base: String,
        api_version: String,
    ) -> Self {
        Self {
            server_transit_xkey: server_xkey,
            encryption_xkey,
            client,
            subject_base,
            name,
            bucket,
            max_secret_history,
            queue_base,
            api_version,
        }
    }
}

#[async_trait]
impl SecretsServer for Api {
    async fn get(&self, request: SecretRequest) -> Result<SecretResponse, GetSecretError> {
        // First validate the entity JWT
        if let Err(e) = request.context.valid_claims() {
            return Err(GetSecretError::InvalidEntityJWT(e.to_string()));
        }

        // Next, validate the host JWT
        let host_claims: Claims<Host> = Claims::decode(&request.context.host_jwt)
            .map_err(|e| GetSecretError::InvalidEntityJWT(e.to_string()))?;
        if let Err(e) = validate_token::<Host>(&request.context.host_jwt) {
            return Err(GetSecretError::InvalidHostJWT(e.to_string()));
        };

        // TODO: this shouldn't be possible in the future, but until we have a way of dynamically
        // issuing host JWTs for this purpose we can just warn about it.
        if host_claims.issuer.starts_with('N') {
            warn!("Host JWT issued by a non-account key");
        }

        // Now that we have established both JWTs are valid, we can go ahead and retrieve the
        // secret
        // TODO: Would be great to do this without two separate calls to decode, especially since we may send back the wrong error
        let component_claims: wascap::Result<Claims<Component>> =
            Claims::decode(&request.context.entity_jwt);
        let provider_claims: wascap::Result<Claims<CapabilityProvider>> =
            Claims::decode(&request.context.entity_jwt);
        let subject = match (component_claims, provider_claims) {
            (Ok(c), _) => c.subject,
            (_, Ok(p)) => p.subject,
            (Err(e), _) => return Err(GetSecretError::InvalidEntityJWT(e.to_string())),
        };

        let store = self
            .state_bucket()
            .await
            .map_err(|e| GetSecretError::UpstreamError(e.to_string()))?;
        let entry = store
            .get(&subject)
            .await
            .map_err(|e| GetSecretError::UpstreamError(e.to_string()))?;

        if entry.is_none() {
            return Err(GetSecretError::Unauthorized);
        }
        let values: HashSet<String> = serde_json::from_slice(&entry.unwrap())
            .map_err(|e| GetSecretError::UpstreamError(e.to_string()))?;

        if !values.contains(&request.key) {
            return Err(GetSecretError::Unauthorized);
        }

        let js = jetstream::new(self.client.clone());
        let secrets = js
            .get_key_value(&self.bucket)
            .await
            .map_err(|e| GetSecretError::UpstreamError(e.to_string()))?;

        let entry = match &request.version {
            Some(v) => {
                let revision = str::parse::<u64>(v).map_err(|_| GetSecretError::InvalidRequest)?;

                let mut key_hist = secrets
                    .history(&request.key)
                    .await
                    .map_err(|e| GetSecretError::UpstreamError(e.to_string()))?;
                find_key_rev(&mut key_hist, revision).await
            }
            None => secrets
                .entry(&request.key)
                .await
                .map_err(|e| GetSecretError::UpstreamError(e.to_string()))?,
        };

        if entry.is_none() {
            return Err(GetSecretError::SecretNotFound);
        }
        // SAFETY: entry is not None, we just verified that
        let entry = entry.unwrap();

        let mut secret = Secret {
            version: entry.revision.to_string(),
            ..Default::default()
        };

        let decrypted = self
            .encryption_xkey
            .open(&entry.value, &self.encryption_xkey)
            .map_err(|_| GetSecretError::DecryptionError)?;

        match String::from_utf8(decrypted) {
            Ok(s) => {
                secret.string_secret = Some(s);
            }
            Err(_) => {
                secret.binary_secret = Some(entry.value.to_vec());
            }
        };

        let response = SecretResponse {
            secret: Some(secret),
            ..Default::default()
        };
        Ok(response)
    }

    fn server_xkey(&self) -> XKey {
        let xkey = XKey::from_public_key(self.server_transit_xkey.public_key().as_str()).unwrap();
        xkey
    }
}

async fn find_key_rev(h: &mut History, revision: u64) -> Option<Entry> {
    while let Some(entry) = h.next().await {
        if let Ok(entry) = entry {
            if entry.revision == revision {
                return Some(entry);
            }
        }
    }
    None
}

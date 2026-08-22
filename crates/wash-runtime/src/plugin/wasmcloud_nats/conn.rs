//! Per-workload NATS connections.
//!
//! Connections are never shared across workloads, even when configuration
//! matches. NATS authorizes per principal, so one shared client would give
//! every workload identical rights and defeat the isolation the sandbox
//! provides. Within a workload, bindings with identical configuration do share
//! one client — `async_nats::Client` multiplexes subscriptions over a single
//! TCP connection.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::config::{ConnKey, NatsAuth, NatsConfig};
use super::policy::PolicyEngine;

/// Budget for draining in-flight work at shutdown. The host allows 3 seconds
/// per plugin and kills the task after, so this leaves room to log what was
/// abandoned.
const DRAIN_BUDGET: std::time::Duration = std::time::Duration::from_millis(2_500);

/// A live connection plus everything derived from the config that opened it.
pub struct ConnHandle {
    /// Bumped every time the client (re)connects.
    ///
    /// A JetStream push subscription cannot survive a server restart — an
    /// ephemeral consumer is gone with the server's state, and the client
    /// resubscribes to a deliver subject nothing publishes to any more. The
    /// idle-heartbeat timeout catches a *deleted* consumer, but not this: the
    /// subscription simply parks. Watching reconnects gives the subscription
    /// loops a signal they can rebuild on.
    pub reconnects: tokio::sync::watch::Sender<u64>,
    pub client: async_nats::Client,
    pub jetstream: async_nats::jetstream::Context,
    pub policy: Arc<PolicyEngine>,
    pub ack_mode: super::config::AckMode,
    pub limits: super::config::Limits,
    /// Server `max_payload` from INFO, checked before a publish leaves the host.
    pub max_payload: u64,
    /// Server version from INFO, for capability gating.
    pub server_version: String,
}

impl ConnHandle {
    /// True when the connected server is at least `major.minor`.
    pub fn server_at_least(&self, major: u64, minor: u64) -> bool {
        server_at_least(&self.server_version, major, minor)
    }
}

/// Compares a NATS server version string against a `major.minor` floor.
pub fn server_at_least(version: &str, major: u64, minor: u64) -> bool {
    let mut parts = version.split('.');
    let actual_major: u64 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let actual_minor: u64 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (actual_major, actual_minor) >= (major, minor)
}

/// Connections held on behalf of one workload.
#[derive(Default)]
struct WorkloadConns {
    by_key: HashMap<ConnKey, Arc<ConnHandle>>,
}

/// All connections the plugin holds, partitioned by workload.
#[derive(Default)]
pub struct ConnectionRegistry {
    workloads: RwLock<HashMap<String, WorkloadConns>>,
}

/// Applies auth to a builder. The nkey signing callback stays here, host-side.
fn apply_auth(
    opts: async_nats::ConnectOptions,
    auth: &NatsAuth,
) -> anyhow::Result<async_nats::ConnectOptions> {
    Ok(match auth {
        NatsAuth::Anonymous => opts,
        NatsAuth::CredsFile(path) => {
            let creds = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read NATS creds file {}", path.display()))?;
            async_nats::ConnectOptions::with_credentials(&creds)
                .context("failed to parse NATS creds file")?
        }
        NatsAuth::JwtNkey { jwt, seed } => {
            let key_pair = std::sync::Arc::new(
                nkeys::KeyPair::from_seed(seed).context("invalid NATS nkey seed")?,
            );
            opts.jwt(jwt.clone(), move |nonce| {
                let key_pair = key_pair.clone();
                async move { key_pair.sign(&nonce).map_err(async_nats::AuthError::new) }
            })
        }
        NatsAuth::NkeySeed(seed) => opts.nkey(seed.to_string()),
        NatsAuth::UserPassword { username, password } => {
            opts.user_and_password(username.clone(), password.to_string())
        }
        NatsAuth::Token(token) => opts.token(token.to_string()),
    })
}

/// Builds connect options from a workload's config.
///
/// `reconnects` is bumped from the event callback, which is the only place the
/// client reports having come back.
fn build_options(
    config: &NatsConfig,
    reconnects: tokio::sync::watch::Sender<u64>,
) -> anyhow::Result<async_nats::ConnectOptions> {
    let mut opts = apply_auth(async_nats::ConnectOptions::new(), &config.auth)?;

    if let Some(name) = &config.name {
        opts = opts.name(name);
    }
    if let Some(prefix) = &config.inbox_prefix {
        opts = opts.custom_inbox_prefix(prefix.clone());
    }
    if let Some(timeout) = config.request_timeout {
        opts = opts.request_timeout(Some(timeout));
    }
    opts = opts.subscription_capacity(config.limits.subscription_capacity);
    if let Some(ca) = &config.tls.ca {
        opts = opts.add_root_certificates(ca.clone());
    }
    if config.tls.first {
        opts = opts.tls_first();
    }
    if let (Some(cert), Some(key)) = (&config.tls.cert, &config.tls.key) {
        opts = opts.add_client_certificate(cert.clone(), key.clone());
    }

    // Without a callback these are raised and discarded. SlowConsumer is the
    // only signal that a subscription buffer overflowed and dropped messages.
    Ok(opts.event_callback(move |event| {
        let reconnects = reconnects.clone();
        async move {
            match event {
                async_nats::Event::SlowConsumer(sid) => {
                    warn!(subscription = sid, "NATS slow consumer: messages dropped")
                }
                async_nats::Event::Disconnected => warn!("disconnected from NATS"),
                async_nats::Event::Connected => {
                    debug!("connected to NATS");
                    reconnects.send_modify(|generation| *generation += 1);
                }
                async_nats::Event::ClientError(err) => warn!(%err, "NATS client error"),
                async_nats::Event::ServerError(err) => warn!(%err, "NATS server error"),
                other => debug!(event = %other, "NATS connection event"),
            }
        }
    }))
}

/// Derives the per-workload inbox prefix.
///
/// The default `_INBOX.>` is shared, so two workloads on one server can observe
/// and race to consume each other's request-reply responses.
pub fn workload_inbox_prefix(workload_id: &str) -> String {
    let sanitized: String = workload_id
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    format!("_INBOX_{sanitized}")
}

impl ConnectionRegistry {
    /// Opens or joins a connection for a workload, returning a live handle.
    pub async fn acquire(
        &self,
        workload_id: &str,
        config: &NatsConfig,
        lattice_prefixes: Vec<String>,
    ) -> anyhow::Result<Arc<ConnHandle>> {
        let key = config.connection_key();

        if let Some(existing) = self
            .workloads
            .read()
            .await
            .get(workload_id)
            .and_then(|w| w.by_key.get(&key))
        {
            return Ok(existing.clone());
        }

        let (reconnects, _) = tokio::sync::watch::channel(0);
        let opts = build_options(config, reconnects.clone())?;
        let client = opts
            .connect(config.servers.clone())
            .await
            .with_context(|| {
                format!(
                    "failed to connect to NATS at {} using {} auth",
                    config.servers.join(","),
                    config.auth.kind()
                )
            })?;

        let info = client.server_info();
        let max_payload = info.max_payload as u64;
        let server_version = info.version.clone();

        let jetstream = match &config.jetstream_domain {
            Some(domain) => async_nats::jetstream::with_domain(client.clone(), domain),
            None => async_nats::jetstream::new(client.clone()),
        };

        let handle = Arc::new(ConnHandle {
            reconnects,
            client,
            jetstream,
            policy: Arc::new(PolicyEngine::new(&config.policy, lattice_prefixes)),
            ack_mode: config.ack_mode,
            limits: config.limits.clone(),
            max_payload,
            server_version,
        });

        let mut workloads = self.workloads.write().await;
        let entry = workloads.entry(workload_id.to_string()).or_default();
        // A concurrent acquire may have won; prefer the connection already stored.
        if let Some(existing) = entry.by_key.get(&key) {
            return Ok(existing.clone());
        }
        entry.by_key.insert(key, handle.clone());
        Ok(handle)
    }

    /// Looks up a workload's connection.
    ///
    /// A workload holds exactly one, enforced at bind time: capability calls
    /// carry no binding identity, so a second connection would make the client
    /// a call resolves to — and therefore its grant — arbitrary.
    pub async fn get(&self, workload_id: &str) -> Option<Arc<ConnHandle>> {
        let workloads = self.workloads.read().await;
        let conns = workloads.get(workload_id)?;
        debug_assert!(
            conns.by_key.len() <= 1,
            "a workload must hold at most one NATS connection"
        );
        conns.by_key.values().next().cloned()
    }

    /// True when this workload already holds a connection opened with a
    /// different configuration.
    pub async fn has_conflicting(&self, workload_id: &str, key: &ConnKey) -> bool {
        self.workloads
            .read()
            .await
            .get(workload_id)
            .is_some_and(|conns| conns.by_key.keys().any(|existing| existing != key))
    }

    /// Drops every connection held for a workload, draining within budget.
    pub async fn release(&self, workload_id: &str) {
        let Some(conns) = self.workloads.write().await.remove(workload_id) else {
            return;
        };
        for (key, handle) in conns.by_key {
            let servers = key.servers.join(",");
            match tokio::time::timeout(DRAIN_BUDGET, handle.client.drain()).await {
                Ok(Ok(())) => debug!(workload_id, servers, "drained NATS connection"),
                Ok(Err(err)) => warn!(workload_id, servers, %err, "NATS drain failed"),
                Err(_) => warn!(
                    workload_id,
                    servers, "NATS drain exceeded budget; in-flight messages abandoned"
                ),
            }
        }
    }

    /// Drains everything at host shutdown.
    pub async fn shutdown(&self) {
        let workload_ids: Vec<String> = self.workloads.read().await.keys().cloned().collect();
        for workload_id in workload_ids {
            self.release(&workload_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbox_prefix_is_per_workload() {
        assert_eq!(workload_inbox_prefix("orders"), "_INBOX_orders");
        assert_ne!(
            workload_inbox_prefix("orders"),
            workload_inbox_prefix("payments")
        );
    }

    #[test]
    fn inbox_prefix_sanitizes_subject_separators() {
        // A dot would widen the subscription to a whole extra token level.
        assert_eq!(workload_inbox_prefix("a.b/c-d"), "_INBOX_a_b_c_d");
    }

    #[test]
    fn server_version_comparison() {
        assert!(server_at_least("2.11.0", 2, 10));
        assert!(server_at_least("2.10.0", 2, 10));
        assert!(server_at_least("2.12.1", 2, 10));
        assert!(!server_at_least("2.9.5", 2, 10));
        assert!(!server_at_least("", 2, 10));
        assert!(!server_at_least("garbage", 2, 10));
    }
}

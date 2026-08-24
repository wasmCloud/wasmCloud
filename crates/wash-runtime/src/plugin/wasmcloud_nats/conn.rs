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
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context as _;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::config::{ConnKey, NatsAuth, NatsConfig};
use super::policy::PolicyEngine;

/// Budget for draining one connection's in-flight work at shutdown.
///
/// The host drops the plugin's `stop()` future at `plugin_stop() + 1s`, so the
/// budget is taken from that cap rather than hard-coded: a
/// `WASH_PLUGIN_STOP_TIMEOUT_SECS` override moves both together. Half a second
/// is held back to log what was abandoned before the task is killed.
fn drain_budget() -> std::time::Duration {
    crate::timeouts::plugin_stop().saturating_sub(std::time::Duration::from_millis(500))
}

/// How long a delivered `reply-to` stays publishable — see [`ConnHandle::pending_replies`].
const REPLY_GRANT_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Cap on outstanding reply grants, so a firehose of unanswered requests cannot
/// grow the map without bound.
const MAX_PENDING_REPLIES: usize = 1024;

/// How long to wait on a credentials-file read before failing the bind.
///
/// The read is off the executor either way; the timeout is so a hung mount
/// fails the deploy with something legible rather than parking it forever.
const CREDS_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A live connection plus everything derived from the config that opened it.
pub struct ConnHandle {
    /// Bumped every time the client (re)connects.
    ///
    /// A JetStream push subscription cannot survive a server restart — an
    /// ephemeral consumer is gone with the server's state, and the client
    /// resubscribes to a deliver subject nothing publishes to any more.
    ///
    /// The client's own 2x-idle-heartbeat timer does eventually notice
    /// (`Messages::poll_next` fires whatever stopped the traffic), so this is a
    /// ~30s backstop rather than the only detector. Watching reconnects is what
    /// makes the rebuild *prompt*: seeing a heartbeat-triggered rebuild without
    /// a reconnect-triggered one means the `Connected` event was dropped by the
    /// bounded event channel.
    pub reconnects: tokio::sync::watch::Sender<u64>,
    /// Subjects the server refused a SUB for, as they arrive.
    ///
    /// A permissions violation is asynchronous: the subscription is accepted
    /// locally and simply never delivers. Without this the workload runs
    /// forever receiving nothing, and the only trace is a generic server-error
    /// warn that names no subject.
    pub subscription_denials: tokio::sync::broadcast::Sender<String>,
    pub client: async_nats::Client,
    pub jetstream: async_nats::jetstream::Context,
    pub policy: Arc<PolicyEngine>,
    pub ack_mode: super::config::AckMode,
    pub limits: super::config::Limits,
    /// Inboxes the host has handed to the guest as a `reply-to`, each good for
    /// one publish.
    ///
    /// A responder has to answer on a random `_INBOX` subject that no sane
    /// grant covers. Granting `_INBOX.>` instead would let the workload read
    /// every other client's replies, so the authorization is scoped to the
    /// exact inbox the host just delivered and consumed on first use.
    pending_replies: std::sync::Mutex<HashMap<String, tokio::time::Instant>>,
}

impl ConnHandle {
    /// True when the connected server is at least `major.minor`.
    ///
    /// Any patch release of that minor qualifies. A floor that landed in a
    /// patch release wants [`server_at_least`] directly, which is also what a
    /// caller that has to name the running version in an error message
    /// already holds.
    pub fn server_at_least(&self, major: u64, minor: u64) -> bool {
        server_at_least(&self.server_version(), (major, minor, 0))
    }

    /// The server's current `max_payload`.
    ///
    /// Read live rather than snapshotted at connect: async-nats refreshes it
    /// from every reconnect's INFO, and a failover onto a node with a smaller
    /// limit would otherwise pass oversize bodies through the pre-flight check
    /// and report the old limit in the error.
    pub fn max_payload(&self) -> u64 {
        self.client.max_payload() as u64
    }

    /// The connected server's version, live for the same reason.
    pub fn server_version(&self) -> String {
        self.client.server_info().version
    }

    /// Records that `inbox` was handed to the guest and may be replied to once.
    pub fn grant_reply(&self, inbox: &str) {
        let now = tokio::time::Instant::now();
        let Ok(mut pending) = self.pending_replies.lock() else {
            return;
        };
        pending.retain(|_, granted| now.duration_since(*granted) < REPLY_GRANT_TTL);
        if pending.len() >= MAX_PENDING_REPLIES {
            return;
        }
        pending.insert(inbox.to_string(), now);
    }

    /// Consumes a reply grant, if this subject has an unexpired one.
    pub fn take_reply_grant(&self, subject: &str) -> bool {
        let Ok(mut pending) = self.pending_replies.lock() else {
            return false;
        };
        pending.remove(subject).is_some_and(|granted| {
            tokio::time::Instant::now().duration_since(granted) < REPLY_GRANT_TTL
        })
    }
}

/// Compares a NATS server version string against a `major.minor.patch` floor.
///
/// The patch takes part because not every capability floor lands on a minor
/// boundary — subject-filtered stream info arrived in 2.7.2. A component that
/// will not parse reads as zero, so an unrecognisable version gates the call
/// off rather than being trusted with it.
pub fn server_at_least(version: &str, floor: (u64, u64, u64)) -> bool {
    let mut parts = version.split('.');
    let major: u64 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor: u64 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch: u64 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor, patch) >= floor
}

/// Connections held on behalf of one workload.
///
/// A workload's `wasmcloud:nats` bindings are addressed by *binding name*: the
/// `(implements ..)` label a component imports the interface under, or the
/// empty string for a plain, unlabeled binding. Two labels pointing at the same
/// server share one connection — `by_key` is what makes that so — while two
/// labels on different servers get one each.
#[derive(Default)]
struct WorkloadConns {
    by_key: HashMap<ConnKey, Arc<ConnHandle>>,
    /// Binding name -> the connection it resolves to, with the key it was
    /// opened under so a re-bind under different configuration is caught.
    by_name: HashMap<String, (ConnKey, Arc<ConnHandle>)>,
}

/// The binding name of a plain, unlabeled `wasmcloud:nats` interface.
pub const UNNAMED_BINDING: &str = "";

/// All connections the plugin holds, partitioned by workload.
#[derive(Default)]
pub struct ConnectionRegistry {
    workloads: RwLock<HashMap<String, WorkloadConns>>,
}

/// Applies auth to a builder. The nkey signing callback stays here, host-side.
async fn apply_auth(
    opts: async_nats::ConnectOptions,
    auth: &NatsAuth,
) -> anyhow::Result<async_nats::ConnectOptions> {
    Ok(match auth {
        NatsAuth::Anonymous => opts,
        NatsAuth::CredsFile(path) => {
            // Blocking here would pin a runtime worker for as long as the mount
            // takes to answer, which on a hung NFS/FUSE path is forever.
            let creds = tokio::time::timeout(CREDS_READ_TIMEOUT, tokio::fs::read_to_string(path))
                .await
                .with_context(|| format!("timed out reading NATS creds file {}", path.display()))?
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
async fn build_options(
    config: &NatsConfig,
    reconnects: tokio::sync::watch::Sender<u64>,
    denials: tokio::sync::broadcast::Sender<String>,
) -> anyhow::Result<async_nats::ConnectOptions> {
    let mut opts = apply_auth(async_nats::ConnectOptions::new(), &config.auth).await?;

    if let Some(name) = &config.name {
        opts = opts.name(name);
    }
    if let Some(prefix) = &config.inbox_prefix {
        opts = opts.custom_inbox_prefix(prefix.clone());
    }
    // Set unconditionally: `None` clears async-nats's implicit 10s default,
    // which would otherwise fire before any guest deadline longer than that and
    // report it as the guest's own timeout. The guest deadline is enforced by
    // the `tokio::time::timeout` wrapped around every `client.request`, so it
    // becomes the sole timer when the operator configures no cap of their own.
    opts = opts.request_timeout(config.request_timeout);
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
    //
    // The plugin never sets `retry_on_initial_connect`, and events are
    // dispatched FIFO by a single task, so the first `Connected` through this
    // callback is deterministically the opening connect rather than a reconnect.
    let initial_connect_seen = Arc::new(AtomicBool::new(false));
    Ok(opts.event_callback(move |event| {
        let reconnects = reconnects.clone();
        let denials = denials.clone();
        let initial_connect_seen = initial_connect_seen.clone();
        async move {
            handle_nats_event(&event, &reconnects, &denials, &initial_connect_seen);
        }
    }))
}

/// Routes one connection event, extracted from the callback so it is testable.
fn handle_nats_event(
    event: &async_nats::Event,
    reconnects: &tokio::sync::watch::Sender<u64>,
    denials: &tokio::sync::broadcast::Sender<String>,
    initial_connect_seen: &AtomicBool,
) {
    match event {
        async_nats::Event::SlowConsumer(sid) => {
            warn!(subscription = sid, "NATS slow consumer: messages dropped")
        }
        async_nats::Event::Disconnected => warn!("disconnected from NATS"),
        async_nats::Event::Connected => {
            debug!("connected to NATS");
            // The opening connect is not a reconnect. Bumping the generation
            // for it races the subscriber loops' `mark_unchanged`, and losing
            // that race tears down a consumer that was just created — which for
            // `deliver-policy: new` re-anchors "now" a couple of seconds later
            // and skips whatever was published in between.
            if initial_connect_seen.swap(true, Ordering::SeqCst) {
                reconnects.send_modify(|generation| *generation += 1);
            }
        }
        async_nats::Event::ClientError(err) => warn!(%err, "NATS client error"),
        async_nats::Event::ServerError(err) => {
            if let async_nats::ServerError::Other(text) = err
                && let Some(subject) = denied_subscription_subject(text)
            {
                warn!(
                    subject = %subject,
                    "NATS server denied SUB permission; this subscription will never deliver"
                );
                let _ = denials.send(subject);
                return;
            }
            warn!(%err, "NATS server error")
        }
        other => debug!(event = %other, "NATS connection event"),
    }
}

/// Pulls the subject out of a server permissions violation for a subscription.
///
/// nats-server sends `Permissions Violation for Subscription to "orders.new"`,
/// optionally followed by ` using queue "workers"`. Anything else — including
/// the publish-side violation, which is a different failure — returns `None`.
fn denied_subscription_subject(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    if !lower.contains("permissions violation for subscription") {
        return None;
    }
    let mut quoted = text.split('"');
    quoted.next()?;
    quoted.next().map(str::to_string)
}

/// Derives the per-workload inbox prefix.
///
/// The default `_INBOX.>` is shared, so two workloads on one server can observe
/// and race to consume each other's request-reply responses.
pub fn workload_inbox_prefix(workload_id: &str) -> String {
    format!("_INBOX_{}", sanitize_subject_token(workload_id))
}

/// Derives the inbox prefix for one binding of a workload.
///
/// Named bindings are separate connections, often to separate servers, and two
/// of them sharing a prefix would put both their replies on one subject where
/// either could consume the other's.
pub fn binding_inbox_prefix(workload_id: &str, binding: &str) -> String {
    if binding.is_empty() {
        return workload_inbox_prefix(workload_id);
    }
    format!(
        "_INBOX_{}_{}",
        sanitize_subject_token(workload_id),
        sanitize_subject_token(binding)
    )
}

/// Escapes anything that would widen a subscription — a `.` opens a whole extra
/// token level — into a form that stays one token.
///
/// The escape is injective: mapping every non-alphanumeric character to the
/// same `_` would give `orders.a` and `orders-a` one shared inbox prefix, and
/// two workloads sharing a prefix can read each other's replies. `_` itself is
/// escaped so nothing collides with the escape sequence.
fn sanitize_subject_token(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            // Fixed-width hex of the code point, so no two inputs can produce
            // the same escape.
            out.push('_');
            out.push_str(&format!("{:04x}", c as u32));
        }
    }
    out
}

/// Drains one connection, logging exactly one outcome line for it.
async fn drain_one(workload_id: &str, key: &ConnKey, handle: &Arc<ConnHandle>) {
    let servers = key.servers.join(",");
    match tokio::time::timeout(drain_budget(), handle.client.drain()).await {
        Ok(Ok(())) => debug!(workload_id, servers, "drained NATS connection"),
        Ok(Err(err)) => warn!(workload_id, servers, %err, "NATS drain failed"),
        Err(_) => warn!(
            workload_id,
            servers, "NATS drain exceeded budget; in-flight messages abandoned"
        ),
    }
}

impl ConnectionRegistry {
    /// Opens or joins a connection for a workload, returning a live handle.
    pub async fn acquire(
        &self,
        workload_id: &str,
        binding: &str,
        config: &NatsConfig,
        lattice_prefixes: Vec<String>,
    ) -> anyhow::Result<Arc<ConnHandle>> {
        let key = config.connection_key();

        if let Some(existing) = self
            .workloads
            .write()
            .await
            .get_mut(workload_id)
            .and_then(|w| {
                let handle = w.by_key.get(&key).cloned()?;
                w.by_name
                    .insert(binding.to_string(), (key.clone(), handle.clone()));
                Some(handle)
            })
        {
            return Ok(existing);
        }

        let (reconnects, _) = tokio::sync::watch::channel(0);
        let (subscription_denials, _) = tokio::sync::broadcast::channel(64);
        let opts = build_options(config, reconnects.clone(), subscription_denials.clone()).await?;
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

        let jetstream = match &config.jetstream_domain {
            Some(domain) => async_nats::jetstream::with_domain(client.clone(), domain),
            None => async_nats::jetstream::new(client.clone()),
        };

        let handle = Arc::new(ConnHandle {
            reconnects,
            subscription_denials,
            client,
            jetstream,
            policy: Arc::new(PolicyEngine::new(&config.policy, lattice_prefixes)),
            ack_mode: config.ack_mode,
            limits: config.limits.clone(),
            pending_replies: std::sync::Mutex::new(HashMap::new()),
        });

        let mut workloads = self.workloads.write().await;
        let entry = workloads.entry(workload_id.to_string()).or_default();
        // A concurrent acquire may have won; prefer the connection already stored.
        let handle = match entry.by_key.get(&key) {
            Some(existing) => existing.clone(),
            None => {
                entry.by_key.insert(key.clone(), handle.clone());
                handle
            }
        };
        entry
            .by_name
            .insert(binding.to_string(), (key, handle.clone()));
        Ok(handle)
    }

    /// Looks up the connection a plain, unlabeled binding calls through.
    ///
    /// `None` when the workload binds `wasmcloud:nats` only under
    /// `(implements ..)` labels: an unlabeled call names no binding, so there is
    /// no grant to check it against.
    pub async fn get(&self, workload_id: &str) -> Option<Arc<ConnHandle>> {
        self.get_named(workload_id, UNNAMED_BINDING).await
    }

    /// Looks up the connection a named (labeled) binding calls through.
    pub async fn get_named(&self, workload_id: &str, binding: &str) -> Option<Arc<ConnHandle>> {
        self.workloads
            .read()
            .await
            .get(workload_id)?
            .by_name
            .get(binding)
            .map(|(_, handle)| handle.clone())
    }

    /// Every binding name this workload holds, for wiring label-routed imports.
    pub async fn bindings_for(&self, workload_id: &str) -> HashMap<String, Arc<ConnHandle>> {
        self.workloads
            .read()
            .await
            .get(workload_id)
            .map(|conns| {
                conns
                    .by_name
                    .iter()
                    .map(|(name, (_, handle))| (name.clone(), handle.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// True when this binding name already resolves to a connection opened with
    /// a different configuration.
    ///
    /// Scoped to the name because that is what a call carries: two *labels* may
    /// differ freely, but one label with two configurations would leave the
    /// grant a call is checked against undefined.
    pub async fn has_conflicting(&self, workload_id: &str, binding: &str, key: &ConnKey) -> bool {
        self.workloads
            .read()
            .await
            .get(workload_id)
            .and_then(|conns| conns.by_name.get(binding))
            .is_some_and(|(existing, _)| existing != key)
    }

    /// Drops every connection held for a workload, draining within budget.
    pub async fn release(&self, workload_id: &str) {
        let Some(conns) = self.workloads.write().await.remove(workload_id) else {
            return;
        };
        // A workload holds few connections — one per distinct binding config —
        // and this path is not racing the host's stop cap, so serial is fine.
        for (key, handle) in conns.by_key {
            drain_one(workload_id, &key, &handle).await;
        }
    }

    /// Drains everything at host shutdown.
    ///
    /// Concurrently, so wall-clock stays near one [`drain_budget`] however many
    /// workloads are bound: draining serially at up to a full budget each
    /// overruns the host's stop cap from the third workload on, and every
    /// connection after the cut-off is abandoned without even a log line.
    pub async fn shutdown(&self) {
        let workloads = std::mem::take(&mut *self.workloads.write().await);
        let drains = workloads.iter().flat_map(|(workload_id, conns)| {
            conns
                .by_key
                .iter()
                .map(move |(key, handle)| drain_one(workload_id, key, handle))
        });
        futures::future::join_all(drains).await;
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
        let prefix = workload_inbox_prefix("a.b/c-d");
        assert!(!prefix.contains('.'), "{prefix} spans two tokens");
        assert!(prefix.starts_with("_INBOX_"));
    }

    /// Two workload ids that differ only in punctuation must not land on one
    /// inbox prefix — sharing it means sharing replies.
    #[test]
    fn inbox_prefix_escaping_is_injective() {
        assert_ne!(workload_inbox_prefix("a.b"), workload_inbox_prefix("a-b"));
        assert_ne!(workload_inbox_prefix("a.b"), workload_inbox_prefix("a_b"));
        assert_ne!(
            workload_inbox_prefix("a_0062"),
            workload_inbox_prefix("a.b")
        );
    }

    #[test]
    fn binding_inbox_prefixes_are_per_binding() {
        assert_eq!(
            binding_inbox_prefix("orders", ""),
            workload_inbox_prefix("orders")
        );
        assert_ne!(
            binding_inbox_prefix("orders", "hub"),
            binding_inbox_prefix("orders", "leaf")
        );
    }

    /// The opening connect must not read as a reconnect: the subscriber loops
    /// would tear down a consumer they had just created.
    #[test]
    fn the_first_connect_is_not_a_reconnect() {
        let (reconnects, _rx) = tokio::sync::watch::channel(0u64);
        let (denials, _drx) = tokio::sync::broadcast::channel(4);
        let seen = AtomicBool::new(false);

        handle_nats_event(&async_nats::Event::Connected, &reconnects, &denials, &seen);
        assert_eq!(*reconnects.borrow(), 0);

        handle_nats_event(
            &async_nats::Event::Disconnected,
            &reconnects,
            &denials,
            &seen,
        );
        assert_eq!(*reconnects.borrow(), 0);

        handle_nats_event(&async_nats::Event::Connected, &reconnects, &denials, &seen);
        assert_eq!(*reconnects.borrow(), 1);
    }

    #[test]
    fn a_subscription_denial_names_its_subject() {
        assert_eq!(
            denied_subscription_subject(
                r#"Permissions Violation for Subscription to "internal.events""#
            )
            .as_deref(),
            Some("internal.events")
        );
        assert_eq!(
            denied_subscription_subject(
                r#"Permissions Violation for Subscription to "_nats_push.>" using queue "workers""#
            )
            .as_deref(),
            Some("_nats_push.>")
        );
        // The publish-side violation is a different failure and is not routed here.
        assert_eq!(
            denied_subscription_subject(r#"Permissions Violation for Publish to "orders.new""#),
            None
        );
        assert_eq!(
            denied_subscription_subject("Unknown Protocol Operation"),
            None
        );
    }

    #[test]
    fn server_version_comparison() {
        assert!(server_at_least("2.11.0", (2, 10, 0)));
        assert!(server_at_least("2.10.0", (2, 10, 0)));
        assert!(server_at_least("2.12.1", (2, 10, 0)));
        assert!(!server_at_least("2.9.5", (2, 10, 0)));
        assert!(!server_at_least("", (2, 10, 0)));
        assert!(!server_at_least("garbage", (2, 10, 0)));

        // A floor inside a minor release separates the patches around it.
        assert!(server_at_least("2.7.2", (2, 7, 2)));
        assert!(server_at_least("2.7.3", (2, 7, 2)));
        assert!(!server_at_least("2.7.1", (2, 7, 2)));
        assert!(!server_at_least("2.7", (2, 7, 2)));
        // A missing or unreadable patch is zero, which clears a floor that
        // does not name one.
        assert!(server_at_least("2.10", (2, 10, 0)));
        assert!(server_at_least("2.10.x", (2, 10, 0)));
        assert!(!server_at_least("2.10.x", (2, 10, 1)));
    }
}

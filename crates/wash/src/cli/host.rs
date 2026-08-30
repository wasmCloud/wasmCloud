use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Context as _;
use clap::Args;
use tracing::info;
use wash_runtime::{
    engine::{Engine, WasmProposal},
    observability::Meters,
    plugin::{self},
};

use crate::cli::{CliCommand, CliContext, CommandOutput};
use crate::config::{HttpClientTrustRoots, load_config};

#[derive(Debug, Clone, Args)]
pub struct HostCommand {
    /// The host group label to assign to the host
    #[arg(long = "host-group", default_value = "default")]
    pub host_group: String,

    /// NATS URL for Control Plane communications
    #[arg(long = "scheduler-nats-url", default_value = "nats://localhost:4222")]
    pub scheduler_nats_url: String,

    /// Path to TLS CA certificate file for NATS Scheduler connection
    #[arg(long = "scheduler-nats-tls-ca")]
    pub scheduler_nats_tls_ca: Option<PathBuf>,

    /// Enable TLS handshake first mode for NATS Scheduler connection
    #[arg(long = "scheduler-nats-tls-first", default_value_t = false)]
    pub scheduler_nats_tls_first: bool,

    /// Path to NATS TLS certificate file for NATS Scheduler connection
    #[arg(long = "scheduler-nats-tls-cert")]
    pub scheduler_nats_tls_cert: Option<PathBuf>,

    /// Path to NATS TLS private key file for NATS Scheduler connection
    #[arg(long = "scheduler-nats-tls-key")]
    pub scheduler_nats_tls_key: Option<PathBuf>,

    /// NATS URL for Data Plane communications
    #[arg(long = "data-nats-url", default_value = "nats://localhost:4222")]
    pub data_nats_url: String,

    /// The path to TLS CA certificate file for NATS Data connection
    #[arg(long = "data-nats-tls-ca")]
    pub data_nats_tls_ca: Option<PathBuf>,

    /// Enable TLS handshake first mode for NATS Data connection
    #[arg(long = "data-nats-tls-first", default_value_t = false)]
    pub data_nats_tls_first: bool,

    /// Path to NATS TLS certificate file for NATS Data connection
    #[arg(long = "data-nats-tls-cert")]
    pub data_nats_tls_cert: Option<PathBuf>,

    /// Path to NATS TLS private key file for NATS Data connection
    #[arg(long = "data-nats-tls-key")]
    pub data_nats_tls_key: Option<PathBuf>,

    /// The host name to assign to the host
    #[arg(long = "host-name")]
    pub host_name: Option<String>,

    /// Environment the host advertises in its heartbeat. For Kubernetes
    /// host pods this is typically the pod's namespace (passed by the
    /// runtime-operator chart via the downward API). The runtime-operator
    /// records this verbatim on the resulting Host CRD's
    /// `spec.environment` field; scheduling uses it to enforce per-tenant
    /// isolation.
    #[arg(long = "environment", env = "WASMCLOUD_HOST_ENVIRONMENT")]
    pub environment: Option<String>,

    /// The address on which the HTTP server will listen
    #[arg(long = "http-addr")]
    pub http_addr: Option<SocketAddr>,

    /// Path to TLS certificate file for the HTTP server
    #[arg(long = "tls-cert-path", requires = "tls_key_path")]
    pub tls_cert_path: Option<PathBuf>,

    /// Path to TLS private key file for the HTTP server
    #[arg(long = "tls-key-path", requires = "tls_cert_path")]
    pub tls_key_path: Option<PathBuf>,

    /// Path to CA certificate file for mutual TLS on the HTTP server
    #[arg(long = "tls-ca-path")]
    pub tls_ca_path: Option<PathBuf>,

    /// Extra CA certificate bundle files (PEM) trusted for outbound HTTPS
    /// requests made by components (`wasi:http` outgoing handler), layered on
    /// top of `--http-client-trust-roots`. Use this to reach hosts behind a
    /// corporate or otherwise private CA.
    ///
    /// Accepts a comma-separated list and/or repeated flags, e.g.
    /// `--http-client-ca-path /etc/wash/ca/corp.pem,/etc/wash/ca/staging.pem`.
    /// Paths must not contain commas.
    #[arg(
        long = "http-client-ca-path",
        env = "WASH_HTTP_CLIENT_CA_PATHS",
        value_delimiter = ','
    )]
    pub http_client_ca_paths: Vec<PathBuf>,

    /// Built-in trust roots for outbound HTTPS requests made by components,
    /// before `--http-client-ca-path` bundles are layered on top.
    /// `webpki-and-native` also trusts the platform store (honouring
    /// `SSL_CERT_FILE`/`SSL_CERT_DIR`); `extra-only` trusts exactly the
    /// configured bundles — the corporate-CA case of pinning a single
    /// private root.
    #[arg(
        long = "http-client-trust-roots",
        env = "WASH_HTTP_CLIENT_TRUST_ROOTS",
        value_enum,
        default_value = "webpki"
    )]
    pub http_client_trust_roots: HttpClientTrustRoots,

    /// Host-wide cap on live connections across every workload and surface
    /// combined — pooled HTTP, raw sockets, and inbound published ports.
    ///
    /// Size it for the number of concurrently busy workloads times their burst
    /// concurrency, kept inside the process's file-descriptor limit.
    #[arg(long = "max-connections", env = "WASH_MAX_CONNECTIONS")]
    pub max_connections: Option<usize>,

    /// Cap on live pooled HTTP and gRPC connections a single workload may
    /// hold, across all authorities it talks to. Idle keep-alive connections
    /// count, so this is really how large a workload's pool may grow.
    #[arg(
        long = "max-outbound-http-connections-per-workload",
        env = "WASH_MAX_OUTBOUND_HTTP_CONNECTIONS_PER_WORKLOAD",
        default_value_t = 128
    )]
    pub max_outbound_http_connections_per_workload: usize,

    /// Cap on raw `wasi:sockets` connections a single workload may hold.
    ///
    /// Refused immediately when reached rather than queued: a guest holds
    /// sockets across yield points, so making it wait for a slot only its own
    /// progress can free would deadlock it against itself.
    #[arg(
        long = "max-outbound-socket-connections-per-workload",
        env = "WASH_MAX_OUTBOUND_SOCKET_CONNECTIONS_PER_WORKLOAD",
        default_value_t = 256
    )]
    pub max_outbound_socket_connections_per_workload: usize,

    /// Cap on inbound published-port connections a single workload may serve
    /// at once. A separate surface from the two above, because a workload
    /// whose inbound traffic consumed its outbound allowance would deadlock
    /// serving requests that need to make outbound calls.
    #[arg(
        long = "max-inbound-socket-connections-per-workload",
        env = "WASH_MAX_INBOUND_SOCKET_CONNECTIONS_PER_WORKLOAD",
        default_value_t = 256
    )]
    pub max_inbound_socket_connections_per_workload: usize,

    /// Host-wide cap on messages being processed at once across every
    /// `wasmcloud:messaging` component on this host.
    ///
    /// A messaging-triggered component gets a fresh instance per message, so
    /// this equally bounds instances. Unset, it is derived from the host's
    /// instance pool: at the worst measured component shape (Componentize-Go,
    /// 5 core instances each) a `total_core_instances` of 1000 supports 200
    /// components, and messaging claims two thirds of that — 133 — leaving the
    /// rest for HTTP-triggered work, warm pools, and long-lived services.
    /// Raising `WASMTIME_POOLING_TOTAL_CORE_INSTANCES` raises this with it;
    /// with pooling disabled there is no budget to divide and the pinned
    /// default of 128 stands. Set, the number given is used as-is.
    //
    // Deliberately no `default_value_t`: a parse-time default is
    // indistinguishable downstream from an operator typing the same number, and
    // `MessagingLimits::resolve` needs to tell them apart to derive from the
    // pool. The default that would go here is documented above instead.
    #[arg(
        long = "wasmcloud-messaging-max-in-flight",
        env = "WASH_WASMCLOUD_MESSAGING_MAX_IN_FLIGHT"
    )]
    pub wasmcloud_messaging_max_in_flight: Option<usize>,

    /// What a component's `max_in_flight` config resolves to when it does not set one,
    /// and the most any single component may ask for.
    ///
    /// A per-component total, unlike `max_concurrency`, which is per warm
    /// instance. A component asking for more than this — or more than
    /// `--wasmcloud-messaging-max-in-flight` — is clamped to it, and the clamp
    /// is logged.
    ///
    /// "Per component" means per component of a deployment, not per replica of
    /// it: replicas that land on the same host share one ceiling, so a
    /// deployment cannot multiply its way past this by scaling out.
    ///
    /// Unset, this is a quarter of whatever `--wasmcloud-messaging-max-in-flight`
    /// resolved to, so the two cannot contradict each other: the pool-derived
    /// 133 of a stock host gives 33, and the pinned 128 of a host with pooling
    /// disabled gives 32.
    #[arg(
        long = "wasmcloud-messaging-max-in-flight-per-component",
        env = "WASH_WASMCLOUD_MESSAGING_MAX_IN_FLIGHT_PER_COMPONENT"
    )]
    pub wasmcloud_messaging_max_in_flight_per_component: Option<usize>,

    /// Total memory on this host that all guests may use (e.g. `8GiB`).
    ///
    /// Unset, it is derived: three quarters of the cgroup limit that would
    /// actually OOM-kill this process, falling back to the machine's total
    /// where there is no cgroup, clamped to 256 MiB..1 TiB. An unset flag means
    /// the derived number, never "unbounded".
    ///
    /// What this bounds is the *total* of every guest's linear memory, which
    /// no other knob does: `--default-heap-memory` bounds one memory and
    /// `--core-instances` bounds a count of slots. Whether it is enforced or
    /// only accounted is `--guest-memory-mode`, which counts by default.
    //
    // Deliberately no `default_value_t`: a parse-time default is
    // indistinguishable downstream from an operator typing the same number, and
    // the derivation has to tell them apart.
    #[arg(long = "max-guest-memory", env = "WASH_HOST_MAX_GUEST_MEMORY")]
    pub max_guest_memory: Option<String>,

    /// How `--max-guest-memory` is applied.
    ///
    /// `count` (the default) charges every guest `memory.grow` to the budget
    /// and records what it would have refused, but allows the growth anyway.
    /// Guest memory was never bounded in aggregate and the budget is derived
    /// when unset, so enforcing on upgrade would hand every host a ceiling
    /// nobody chose; run in `count` first, watch the reported high-water mark
    /// and `would_refuse` count, then switch to `enforce`.
    ///
    /// Under `enforce`, a growth past the budget makes the guest's
    /// `memory.grow` return -1 — the same failure it already sees on hitting
    /// `--default-heap-memory` — rather than trapping it.
    ///
    /// `enforce` makes `--max-guest-memory` a real ceiling, so it has to leave
    /// the host room to be a host: wasmtime, compiled module images, NATS, OCI
    /// pulls and HTTP buffers are not charged to this budget but do come out
    /// of the same container limit. An unset budget already reserves a quarter
    /// of the detected limit for them; a budget set to the whole container
    /// limit can be OOM-killed before it ever refuses a guest.
    //
    // Parsed through `parse_guest_memory_mode` rather than plain `value_enum`
    // so a blank value counts as unset, matching the sibling size knobs: a
    // ConfigMap key or `value: ""` reaches clap as `Some("")`, and failing to
    // parse that would refuse to start the host over a variable nobody set.
    #[arg(
        long = "guest-memory-mode",
        env = "WASH_GUEST_MEMORY_MODE",
        value_parser = parse_guest_memory_mode,
        default_value = "count"
    )]
    pub guest_memory_mode: GuestMemoryMode,

    /// Ceiling on how large any single guest linear memory may grow
    /// (e.g. `512MiB`).
    ///
    /// This is the pooling allocator's `max_memory_size`. Unset, it stays
    /// wasmtime's own default of 4 GiB — which is what every host has run to
    /// date, and why an instance count has never implied a byte count.
    ///
    /// Every slot is sized for this whether or not anything grows into it, so
    /// raising it raises the pool's whole address-space reservation.
    #[arg(long = "default-heap-memory", env = "WASH_DEFAULT_HEAP_MEMORY")]
    pub default_heap_memory: Option<String>,

    /// Instance slots the pooling allocator keeps.
    ///
    /// Unset, this stays wasmtime's default of 1000. Multiplied by
    /// `--default-heap-memory` it is the pool's address-space reservation, so
    /// the two are worth setting together.
    #[arg(long = "core-instances", env = "WASH_CORE_INSTANCES")]
    pub core_instances: Option<u32>,

    /// How long a pooled HTTP connect waits for a slot before failing with a
    /// connect timeout (e.g. `5s`, `500ms`).
    ///
    /// Only the HTTP surface waits — see
    /// `--max-outbound-socket-connections-per-workload`. A component's own
    /// `connect-timeout` bounds its request independently, so this only
    /// decides how long an attempt nothing is waiting on may camp on a slot.
    #[arg(
        long = "http-connection-wait",
        env = "WASH_HTTP_CONNECTION_WAIT",
        value_parser = humantime::parse_duration
    )]
    pub http_connection_wait: Option<Duration>,

    /// Enable WASI WebGPU support
    #[cfg(all(
        not(target_os = "windows"),
        not(target_arch = "s390x"),
        feature = "wasi-webgpu"
    ))]
    #[arg(long = "wasi-webgpu", default_value_t = false)]
    pub wasi_webgpu: bool,

    /// PostgreSQL connection URL for the wasmcloud:postgres plugin
    /// (e.g. postgres://user:pass@bouncer:6432?sslmode=require&pool_size=10)
    #[arg(long = "postgres-url", env = "WASH_POSTGRES_URL")]
    pub postgres_url: Option<String>,

    /// Allow insecure OCI Registries
    #[arg(long = "allow-insecure-registries", default_value_t = false)]
    pub allow_insecure_registries: bool,

    /// Extra CA certificate bundle files (PEM) trusted when pulling from OCI
    /// registries: for a registry behind a private or in-cluster CA, which the
    /// compiled-in public roots do not cover. Applies to every pull this host
    /// makes: workload components, host component plugins, and washlet
    /// artifacts alike.
    ///
    /// Prefer this to `--allow-insecure-registries`, which does not relax
    /// verification but replaces it: that flag switches every registry to
    /// plain HTTP, so credentials travel in the clear and no certificate is
    /// checked at all.
    ///
    /// Accepts a comma-separated list and/or repeated flags. Paths must not
    /// contain commas.
    #[arg(long = "oci-ca-path", env = "WASH_OCI_CA_PATHS", value_delimiter = ',')]
    pub oci_ca_paths: Vec<PathBuf>,

    /// Timeout for pulling artifacts from OCI registries
    #[arg(long = "registry-pull-timeout", value_parser = humantime::parse_duration, default_value = "30s")]
    pub registry_pull_timeout: Duration,

    /// The directory to use for caching OCI artifacts
    #[arg(long = "oci-cache-dir")]
    pub oci_cache_dir: Option<PathBuf>,

    /// Enable WASI OpenTelemetry plugin
    #[arg(long = "wasi-otel", default_value_t = false)]
    pub wasi_otel: bool,

    /// Let workloads and plugins reach the machine's own loopback through
    /// `host.wasmcloud.internal`.
    ///
    /// Off by default. A guest also needs its own `allowedHostLoopbackPorts` entry
    /// naming the port, so neither the operator nor the workload author can
    /// open this door alone. `127.0.0.1` keeps meaning the guest's own virtual
    /// network either way.
    #[arg(long = "allow-host-loopback", default_value_t = false)]
    pub allow_host_loopback: bool,

    /// How the raw-socket egress policy is applied.
    ///
    /// `count` (the default) evaluates the policy, records what it would refuse,
    /// and allows the connection anyway. Raw socket connect was never gated, so
    /// enforcing immediately would sever live traffic on upgrade; run in `count`
    /// first, watch the `would_deny` counters, then switch to `enforce`.
    #[arg(long = "socket-egress", value_enum, default_value = "count")]
    pub socket_egress: SocketEgressMode,

    /// Deny outbound connections to loopback, link-local (including the cloud
    /// metadata address), multicast, and documentation ranges — including
    /// whatever DNS returned for a permitted name.
    #[arg(long = "deny-special-ranges", default_value_t = true)]
    pub deny_special_ranges: bool,

    /// Deny outbound connections to private ranges (RFC1918, ULA, CGNAT).
    /// Off by default: reaching a sibling service on a private address is the
    /// ordinary in-cluster case.
    #[arg(long = "deny-private-ranges", default_value_t = false)]
    pub deny_private_ranges: bool,

    /// NATS the `wasmcloud:nats` plugin's bindings dial when the host's own
    /// declaration for a binding names no `servers`.
    ///
    /// Defaults to `--data-nats-url`, so a workload on the cluster's own NATS
    /// needs no address in its manifest and the same manifest runs in dev and
    /// on a cluster. Set this when the NATS a workload talks to is not the one
    /// backing the host's data plane.
    ///
    /// The data-plane fallback also carries the host's `--data-nats-tls-*`
    /// material, so a TLS-fronted cluster NATS works with no per-binding
    /// configuration. Setting this flag switches the fallback to address
    /// only — the data plane's certificates say nothing about another NATS —
    /// and it carries no grant either way: a binding that inherits it still
    /// reaches nothing until the host grants it something.
    #[arg(long = "wasmcloud-nats-url", env = "WASH_WASMCLOUD_NATS_URL")]
    pub wasmcloud_nats_url: Option<String>,

    /// Removed: the policy is `workloadConfig` on the `host.plugins` entry.
    ///
    /// Hidden, and kept only to fail loudly. Dropping the arg outright makes
    /// clap reject the *flag*, but `WASH_WASMCLOUD_NATS_WORKLOAD_CONFIG` in a
    /// pod spec would simply stop being read — and a host that quietly stops
    /// enforcing `deny` is the one failure this whole mechanism exists to
    /// prevent.
    #[arg(
        long = "wasmcloud-nats-workload-config",
        env = "WASH_WASMCLOUD_NATS_WORKLOAD_CONFIG",
        hide = true
    )]
    pub removed_wasmcloud_nats_workload_config: Option<String>,

    /// Enable additional wasm proposals on the engine. Accepts a comma-separated
    /// list and/or repeated flags, e.g. `--wasm-proposal gc,threads`. Accepted
    /// names: component-model-async, component-model-map, gc,
    /// exception-handling, wide-arithmetic, threads, tail-call.
    #[arg(
        long = "wasm-proposal",
        env = "WASH_WASM_PROPOSALS",
        value_delimiter = ','
    )]
    pub wasm_proposals: Vec<WasmProposal>,

    /// Load a host component plugin providing a host capability from its own supervised store.
    ///
    /// A WebAssembly component served to every workload that imports its
    /// interface. Repeatable; separate multiple with `;` or repeat the flag.
    /// Requires a wash build with the `host-component-plugins` feature. Each
    /// value is comma-separated `key=value` fields — required `id` and exactly
    /// one of `image`/`file`:
    ///   id=<name>,image=<oci-ref>[,pull=always|ifNotPresent|never][,max-restarts=N][,digest=sha256:..]
    ///   id=<name>,file=<path>[,max-restarts=N]
    #[arg(
        long = "host-plugin",
        env = "WASH_HOST_PLUGINS",
        value_delimiter = ';',
        value_parser = parse_host_plugin_spec
    )]
    pub host_plugins: Vec<wash_runtime::plugin::ComponentPluginSpec>,

    /// Username for authenticating to the registry when pulling host component
    /// plugins. Pair with `--host-plugin-registry-password`. Read from the
    /// environment so the credential never appears in a `--host-plugin` arg or
    /// the pod spec — in Kubernetes, source it from a Secret via `secretKeyRef`
    /// on the host container. When unset, plugin pulls fall back to the ambient
    /// docker credential helper (e.g. a mounted imagePullSecret) and then
    /// anonymous access. Applies to host-component-plugin pulls only; workload
    /// components authenticate with their own per-workload image pull secret.
    #[cfg(feature = "host-component-plugins")]
    #[arg(
        long = "host-plugin-registry-user",
        env = "WASH_HOST_PLUGIN_REGISTRY_USER",
        hide_env_values = true,
        requires = "host_plugin_registry_password"
    )]
    pub host_plugin_registry_user: Option<String>,

    /// Password paired with `--host-plugin-registry-user` /
    /// `WASH_HOST_PLUGIN_REGISTRY_USER`. Both are required together.
    #[cfg(feature = "host-component-plugins")]
    #[arg(
        long = "host-plugin-registry-password",
        env = "WASH_HOST_PLUGIN_REGISTRY_PASSWORD",
        hide_env_values = true,
        requires = "host_plugin_registry_user"
    )]
    pub host_plugin_registry_password: Option<String>,
}

/// clap value parser for `--host-plugin`: parse one spec, flattening the
/// `anyhow` error chain into the `String` clap wants.
fn parse_host_plugin_spec(s: &str) -> Result<wash_runtime::plugin::ComponentPluginSpec, String> {
    s.parse().map_err(|e: anyhow::Error| format!("{e:#}"))
}

/// Resolve explicit registry credentials for host-component-plugin pulls from
/// the CLI/env `(user, password)` pair. The two are required together (clap
/// `requires`); a lone half — reachable only defensively — is treated as no
/// credentials, leaving the pull to fall back to the docker credential helper
/// and then anonymous.
#[cfg(feature = "host-component-plugins")]
fn host_plugin_registry_credentials(
    user: Option<&str>,
    password: Option<&str>,
) -> Option<(String, String)> {
    match (user, password) {
        (Some(user), Some(password)) => Some((user.to_string(), password.to_string())),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => None,
    }
}

impl HostCommand {
    /// The operator's plugin binding declarations, plus the fallbacks this
    /// host's own flags supply.
    ///
    /// `wasmcloud:nats` is the only plugin with a fallback today: a binding
    /// that names no `servers` dials the data plane. The TLS material rides
    /// with it as an *anchored bundle* rather than as three independent
    /// defaults, because certs are only valid for the address they were issued
    /// for — an operator who points a binding at some other NATS and sets no
    /// TLS must not inherit the cluster's. The bundle is evaluated at resolve
    /// time, so a workload setting `servers` under `allow` skips it too.
    fn plugin_bindings(
        &self,
        config: &crate::config::Config,
        project_dir: &std::path::Path,
    ) -> anyhow::Result<wash_runtime::plugin::PluginBindings> {
        let declared = config
            .host()
            .to_plugin_bindings(config, project_dir, Some(project_dir))
            .context("failed to resolve host.plugins")?;

        let mut bundle: Vec<(&str, String)> = vec![(
            "servers",
            self.wasmcloud_nats_url
                .clone()
                .unwrap_or_else(|| self.data_nats_url.clone()),
        )];
        // Only when the fallback address *is* the data plane:
        // `--wasmcloud-nats-url` points at some other NATS, whose trust the
        // data plane's certs say nothing about.
        if self.wasmcloud_nats_url.is_none() {
            for (key, path) in [
                ("tls-ca", self.data_nats_tls_ca.as_deref()),
                ("tls-cert", self.data_nats_tls_cert.as_deref()),
                ("tls-key", self.data_nats_tls_key.as_deref()),
            ] {
                if let Some(path) = path {
                    bundle.push((key, path.display().to_string()));
                }
            }
            if self.data_nats_tls_first {
                bundle.push(("tls-first", "true".to_string()));
            }
        }

        let nats = declared
            .for_plugin(wash_runtime::plugin::wasmcloud_nats::PLUGIN_NATS_ID)
            .with_default_bundle("servers", bundle);
        Ok(declared.with_plugin(nats))
    }
}

impl CliCommand for HostCommand {
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        // Validated before anything is connected or built. A bad size is a typo
        // in a flag, and reporting it after a NATS dial has already failed
        // buries the actionable error under an unrelated one. The resolved
        // numbers are reported later, by the engine that installs them.
        let host_memory = wash_runtime::engine::host_memory::HostMemoryBudgets::resolve_strs(
            self.max_guest_memory.as_deref(),
            self.default_heap_memory.as_deref(),
            self.core_instances,
        )
        .map_err(|e| anyhow::anyhow!(e))?;

        // Installed before connect_nats so TLS-enabled NATS clusters have a
        // crypto provider available. Idempotent; also called by Ingress::new.
        wash_runtime::init_crypto();

        // Picked up the same way `wash dev` reads its own config file: global
        // config merged with the project-local one, if any. `wash host` has
        // no CLI-flags-as-config-overrides of its own (unlike `wash dev`'s
        // literal `Config` override for `wit.skip_fetch`) — this only reads
        // the `host:` section (currently `hostPlugins[]`), which has no CLI
        // equivalent to conflict with.
        let project_dir = ctx.project_dir();
        let config =
            load_config::<crate::config::Config>(&ctx.user_config_path(), Some(project_dir), None)
                .context("failed to load config for wash host")?;

        let scheduler_nats_client = wash_runtime::washlet::connect_nats(
            self.scheduler_nats_url.clone(),
            wash_runtime::washlet::NatsConnectionOptions {
                request_timeout: None,
                tls_ca: self.scheduler_nats_tls_ca.clone(),
                tls_first: self.scheduler_nats_tls_first,
                tls_cert: self.scheduler_nats_tls_cert.clone(),
                tls_key: self.scheduler_nats_tls_key.clone(),
            },
        )
        .await
        .context("failed to connect to NATS Scheduler URL")?;

        let data_nats_client = wash_runtime::washlet::connect_nats(
            self.data_nats_url.clone(),
            wash_runtime::washlet::NatsConnectionOptions {
                request_timeout: None,
                tls_ca: self.data_nats_tls_ca.clone(),
                tls_first: self.data_nats_tls_first,
                tls_cert: self.data_nats_tls_cert.clone(),
                tls_key: self.data_nats_tls_key.clone(),
            },
        )
        .await
        .context("failed to connect to NATS")?;
        let data_nats_client = Arc::new(data_nats_client);

        let host_config = wash_runtime::host::HostConfig {
            allow_oci_insecure: self.allow_insecure_registries,
            oci_pull_timeout: Some(self.registry_pull_timeout),
            oci_cache_dir: self.oci_cache_dir.clone(),
            oci_ca_paths: self.oci_ca_paths.clone(),
        };

        // The host applies these itself when it is built, but host component
        // plugins are pulled before that. Install them here too — the same
        // bundles a second time are a no-op — and fail now if any is invalid.
        wash_runtime::oci::set_extra_ca_certificates(&host_config.oci_ca_paths)
            .context("failed to load --oci-ca-path CA certificates")?;

        let mut engine_builder = Engine::builder()
            .with_pooling_allocator(true)
            .with_fuel_consumption(ctx.meters().consumes_fuel());
        for proposal in &self.wasm_proposals {
            engine_builder = engine_builder.with_wasm_proposal(*proposal);
        }
        // One registry for the whole host: a workload's HTTP pool, its raw
        // sockets, and its inbound published ports all draw on the allowance
        // it mints, so an operator configures one set of numbers and a
        // per-workload ceiling really is per workload.
        let quotas = crate::config::connection_quotas(
            self.max_connections,
            Some(self.max_outbound_http_connections_per_workload),
            Some(self.max_outbound_socket_connections_per_workload),
            Some(self.max_inbound_socket_connections_per_workload),
            self.http_connection_wait,
        )?;

        let socket_policy = Arc::new(wash_runtime::sockets::policy::SocketPolicy {
            host_loopback_enabled: self.allow_host_loopback,
            egress_mode: self.socket_egress.into(),
            egress_addrs: wash_runtime::host::egress_policy::EgressAddressPolicy {
                deny_special: self.deny_special_ranges,
                allow_private: !self.deny_private_ranges,
            },
            quotas: Some(Arc::clone(&quotas)),
            meters: Some(Arc::new(wash_runtime::host::quota::PolicyMeters::default())),
            ..Default::default()
        });
        engine_builder = engine_builder.with_socket_policy(Arc::clone(&socket_policy));
        engine_builder = engine_builder.with_host_memory(host_memory);
        engine_builder = engine_builder.with_guest_memory_mode(self.guest_memory_mode.into());

        let engine = engine_builder.build()?;

        // Likewise one set of messaging ceilings for the whole host: the
        // host-wide semaphore lives inside this value, so every messaging
        // backend must be handed the *same* one or each gets its own budget.
        //
        // Built after the engine, not before, because an unset ceiling is
        // derived from the pool the engine actually installed — which accounts
        // for `WASMTIME_POOLING_TOTAL_CORE_INSTANCES` and for pooling being
        // unavailable, neither of which is knowable from the flags alone.
        let messaging_limits = crate::config::wasmcloud_messaging_limits(
            self.wasmcloud_messaging_max_in_flight,
            self.wasmcloud_messaging_max_in_flight_per_component,
            engine.total_core_instances(),
        )?;

        if let Some(value) = &self.removed_wasmcloud_nats_workload_config {
            anyhow::bail!(
                "`--wasmcloud-nats-workload-config` (WASH_WASMCLOUD_NATS_WORKLOAD_CONFIG) has \
                 been removed. Set `workloadConfig: {value}` on the `host.plugins` entry with \
                 `id: wasmcloud-nats` instead, where it applies to that plugin's bindings alone"
            )
        }

        // Resolved before anything is built: a binding an operator declared
        // wrong is a typo in the host's config file, and the workload that
        // later asks for it is not the thing at fault.
        let plugin_bindings = self.plugin_bindings(&config, project_dir)?;
        // Stated at startup because it decides what every workload on the
        // plugin can reach, and the default declines to take a manifest's word
        // for it. `warn` is called out rather than merely reported: it enforces
        // nothing, so an operator who set it and forgot has no protection.
        for id in plugin_bindings.plugin_ids() {
            let declared = plugin_bindings.for_plugin(id);
            let policy = declared.workload_config();
            let names = declared.binding_names().collect::<Vec<_>>().join(",");
            if policy == wash_runtime::plugin::WorkloadConfigPolicy::Warn {
                tracing::warn!(
                    plugin_id = id,
                    bindings = names,
                    "workloadConfig is `warn`: nothing is refused, only reported"
                );
            } else {
                info!(
                    plugin_id = id,
                    workload_config = policy.as_str(),
                    bindings = names,
                    "plugin bindings resolved (see host.plugins)"
                );
            }
        }

        let mut cluster_host_builder = wash_runtime::washlet::ClusterHostBuilder::default()
            .with_engine(engine.clone())
            .with_host_config(host_config)
            .with_nats_client(Arc::new(scheduler_nats_client))
            .with_host_group(self.host_group.clone())
            .with_plugin(Arc::new(
                plugin::wasi_config::DynamicConfig::builder()
                    .copy_environment(true)
                    .build(),
            ))?
            .with_plugin(Arc::new(plugin::wasmcloud_secrets::WasmcloudSecrets::new()))?
            .with_plugin(Arc::new(plugin::wasi_logging::TracingLogger::default()))?
            .with_plugin(Arc::new(plugin::wasi_blobstore::NatsBlobstore::new(
                &data_nats_client,
            )))?
            .with_plugin(Arc::new(
                plugin::wasmcloud_messaging::NatsMessaging::with_limits(
                    data_nats_client.clone(),
                    messaging_limits.clone(),
                ),
            ))?
            .with_plugin(Arc::new(plugin::wasi_keyvalue::NatsKeyValue::new(
                &data_nats_client,
            )))?
            // Opens its own per-workload connections rather than borrowing the
            // host's client, and denies the host's control subjects to every
            // workload. What each binding *is* — its servers, its credentials,
            // and its grants — comes from the host's own declaration, so a
            // workload asks for a capability by name and cannot widen one.
            .with_plugin(Arc::new(
                plugin::wasmcloud_nats::WasmcloudNats::new()
                    // A subscription's byte budget is per subscription and
                    // this host's memory is not. Without the budget the plugin
                    // cannot tell whether the subscriptions it is about to
                    // start fit, and the first sign of the mismatch is an
                    // OOMKill.
                    .with_memory_budget(host_memory.max_guest_memory)
                    .with_lattice_prefixes(vec![
                        format!("{}.", wash_runtime::washlet::HOST_API_PREFIX),
                        format!("{}.", wash_runtime::washlet::OPERATOR_API_PREFIX),
                    ]),
            ))?
            .with_meters(Meters::new(ctx.meters()));

        #[cfg(feature = "wasm_component_model_implements")]
        {
            cluster_host_builder = cluster_host_builder.with_multiplexed_plugins()?;
        }

        if let Some(postgres_url) = &self.postgres_url {
            cluster_host_builder = cluster_host_builder.with_plugin(Arc::new(
                plugin::wasmcloud_postgres::WasmcloudPostgres::new(postgres_url)
                    .context("failed to configure postgres plugin")?,
            ))?;
        } else {
            // register postgres for `(implements ..)` named imports (each
            // carrying its own URL) are served.
            #[cfg(feature = "wasm_component_model_implements")]
            {
                cluster_host_builder = cluster_host_builder.with_plugin(Arc::new(
                    plugin::wasmcloud_postgres::WasmcloudPostgres::multiplex_only(),
                ))?;
            }
        }

        if let Some(host_name) = &self.host_name {
            cluster_host_builder = cluster_host_builder.with_host_name(host_name);
        }

        if let Some(environment) = &self.environment {
            cluster_host_builder = cluster_host_builder.with_environment(environment);
        }

        // One publishing context for the whole host: workloads and plugins
        // reserve from the same table, so a collision between them is a start
        // failure naming both rather than two listeners that each think they
        // own the address.
        if let Some(addr) = self.http_addr {
            let http_router = wash_runtime::host::http::DynamicRouter::default();

            // Outbound (egress) trust roots: extra CAs for components calling
            // HTTPS hosts behind a private CA. Distinct from the ingress TLS
            // options below, which configure the HTTP *server*.
            let outgoing_handler =
                wash_runtime::host::http::DefaultOutgoingHandler::from_tls_options(
                    wash_runtime::host::http_client::ClientTlsOptions {
                        roots: self.http_client_trust_roots.into(),
                        extra_ca_paths: self.http_client_ca_paths.clone(),
                    },
                )
                .context("failed to load --http-client-ca-path CA certificates")?
                // The same registry the socket policy uses, so a workload's
                // HTTP pool and its raw sockets share one configured
                // allowance rather than two.
                .with_quotas(Arc::clone(&quotas));

            let mut ingress_builder = wash_runtime::host::http::Ingress::builder(http_router, addr)
                .outgoing_handler(outgoing_handler);
            if let (Some(cert_path), Some(key_path)) = (&self.tls_cert_path, &self.tls_key_path) {
                let mut tls = wash_runtime::host::http::TlsConfig::new(cert_path, key_path);
                if let Some(ca) = self.tls_ca_path.as_deref() {
                    tls = tls.with_ca(ca);
                }
                ingress_builder = ingress_builder.tls(tls);
            }
            let ingress = ingress_builder.build().await?;
            cluster_host_builder = cluster_host_builder.with_http_handler(Arc::new(ingress));
        }

        // Enable otel plugin
        if self.wasi_otel {
            cluster_host_builder = cluster_host_builder
                .with_plugin(Arc::new(plugin::wasi_otel::WasiOtel::default()))?;
        }

        // Enable WASI WebGPU if requested
        #[cfg(all(
            not(target_os = "windows"),
            not(target_arch = "s390x"),
            feature = "wasi-webgpu"
        ))]
        if self.wasi_webgpu {
            tracing::info!("WASI WebGPU support enabled");
            cluster_host_builder = cluster_host_builder
                .with_plugin(Arc::new(plugin::wasi_webgpu::WebGpu::default()))?;
        }

        // Host component plugins: fetch each declared plugin's wasm and register
        // it before the host starts. Host-operator controlled only — nothing in a
        // workload request can register a host-global capability provider.
        #[cfg(feature = "host-component-plugins")]
        {
            // Explicit registry credentials for plugin pulls, taken from the
            // environment so the secret never appears in a --host-plugin arg or
            // the pod spec. When unset, resolution falls back to the ambient
            // docker credential helper (e.g. a mounted imagePullSecret) and then
            // anonymous access.
            let plugin_oci_config = wash_runtime::oci::OciConfig {
                credentials: host_plugin_registry_credentials(
                    self.host_plugin_registry_user.as_deref(),
                    self.host_plugin_registry_password.as_deref(),
                ),
                insecure: self.allow_insecure_registries,
                cache_dir: self.oci_cache_dir.clone(),
                timeout: Some(self.registry_pull_timeout),
            };
            let native_plugins = cluster_host_builder.native_plugins();
            let http_handler = cluster_host_builder.http_handler();

            // Config-file plugins (`host.hostPlugins`) first, so their
            // config/secretFrom/allowedHosts are honored; CLI/env
            // `--host-plugin` entries follow. Together, not one replacing
            // the other — an operator can declare most plugins in the
            // config file (where config/secrets/policy fit naturally) and
            // still add one ad hoc via `--host-plugin` without duplicating
            // the rest.
            let mut specs: Vec<wash_runtime::plugin::ComponentPluginSpec> = Vec::new();
            for hp in config.host().component_plugins()? {
                specs.push(
                    hp.to_spec(&config, project_dir, Some(project_dir))
                        .with_context(|| format!("failed to resolve host.plugins '{}'", hp.id))?,
                );
            }
            specs.extend(self.host_plugins.iter().cloned());

            for spec in &specs {
                let plugin = wash_runtime::plugin::component_host::load_component_plugin(
                    spec,
                    &engine,
                    plugin_oci_config.clone(),
                    &native_plugins,
                    http_handler.clone(),
                    Some(Arc::clone(&socket_policy)),
                )
                .await
                .with_context(|| format!("failed to load host component plugin '{}'", spec.id))?;
                cluster_host_builder = cluster_host_builder.with_plugin(plugin)?;
                info!(id = %spec.id, "loaded host component plugin");
            }
        }
        #[cfg(not(feature = "host-component-plugins"))]
        anyhow::ensure!(
            self.host_plugins.is_empty() && config.host().component_plugins()?.is_empty(),
            "--host-plugin/WASH_HOST_PLUGINS and a `host.plugins` entry with a `file`/`image` \
             require a wash build with the `host-component-plugins` feature"
        );

        // After every plugin is registered — including the component plugins
        // above — so `build()` can refuse a declaration naming an id this host
        // has no plugin for.
        cluster_host_builder = cluster_host_builder.with_plugin_bindings(plugin_bindings);

        let cluster_host = cluster_host_builder
            .build()
            .context("failed to build cluster host")?;
        let host_cleanup = wash_runtime::washlet::run_cluster_host(cluster_host)
            .await
            .context("failed to start cluster node")?;

        tokio::signal::ctrl_c()
            .await
            .context("failed to listen for shutdown signal")?;

        info!("Stopping host...");

        host_cleanup.await?;

        Ok(CommandOutput::ok(
            "Host exited successfully".to_string(),
            None,
        ))
    }
}

#[cfg(test)]
mod nats_tests {
    use clap::Parser;

    use super::HostCommand;
    use wash_runtime::plugin::{WorkloadConfigPolicy, bindings::never_narrows, wasmcloud_nats};

    /// `HostCommand` is an `Args` group, so give it a `Parser` to parse under.
    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        host: HostCommand,
    }

    fn parse(args: &[&str]) -> HostCommand {
        TestCli::parse_from(std::iter::once("wash-host").chain(args.iter().copied())).host
    }

    fn config_from(yaml: &str) -> crate::config::Config {
        serde_yaml_ng::from_str(yaml).expect("config must parse")
    }

    fn nats_bindings(
        host: &HostCommand,
        config: &crate::config::Config,
    ) -> anyhow::Result<wash_runtime::plugin::PluginBindingSet> {
        Ok(host
            .plugin_bindings(config, std::path::Path::new("."))?
            .for_plugin(wasmcloud_nats::PLUGIN_NATS_ID)
            .clone())
    }

    fn resolve(
        declared: &wash_runtime::plugin::PluginBindingSet,
        binding: &str,
        workload: &[(&str, &str)],
    ) -> anyhow::Result<std::collections::HashMap<String, String>> {
        declared.resolve(
            binding,
            &workload
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            &wasmcloud_nats::binding_schema(),
            never_narrows(),
        )
    }

    /// Restored with the mechanism: `--wasmcloud-nats-url` is now an anchored
    /// default bundle rather than a seeded `servers`, so it has to still beat
    /// the data plane at resolve time.
    #[test]
    fn the_address_flag_overrides_the_data_plane() {
        let host = parse(&[
            "--data-nats-url",
            "nats://data:4222",
            "--wasmcloud-nats-url",
            "nats://workloads:4222",
        ]);
        let declared = nats_bindings(&host, &config_from("{}")).unwrap();
        let resolved = resolve(&declared, "", &[]).unwrap();
        assert_eq!(
            resolved.get("servers").map(String::as_str),
            Some("nats://workloads:4222")
        );
    }

    /// With no `host.plugins` entry a host denies by default — and, owning
    /// nothing an operator declared, refuses nothing a manifest writes.
    #[test]
    fn workload_config_defaults_to_deny() {
        let declared = nats_bindings(&parse(&[]), &config_from("{}")).unwrap();
        assert_eq!(declared.workload_config(), WorkloadConfigPolicy::Deny);
    }

    /// A binding that names no servers falls back to the data plane, so a
    /// workload on the cluster's own NATS needs no address.
    #[test]
    fn the_address_falls_back_to_the_data_plane() {
        let host = parse(&["--data-nats-url", "nats://data:4222"]);
        let declared = nats_bindings(&host, &config_from("{}")).expect("the flags must resolve");
        let resolved = resolve(&declared, "", &[]).unwrap();
        assert_eq!(
            resolved.get("servers").map(String::as_str),
            Some("nats://data:4222")
        );
    }

    /// The data plane's TLS material rides with its address as one bundle. A
    /// binding pointed somewhere else takes neither — certs are only valid for
    /// the address they were issued for.
    #[test]
    fn the_tls_material_travels_with_the_address_it_belongs_to() {
        let host = parse(&[
            "--data-nats-url",
            "nats://data:4222",
            "--data-nats-tls-ca",
            "/certs/ca.crt",
        ]);
        let declared = nats_bindings(&host, &config_from("{}")).unwrap();

        let inherited = resolve(&declared, "", &[]).unwrap();
        assert_eq!(
            inherited.get("tls-ca").map(String::as_str),
            Some("/certs/ca.crt"),
            "a binding on the data plane gets its certs"
        );

        // Evaluated at resolve time, so a workload naming its own address under
        // `allow` skips the bundle too — not just an operator who declared one.
        let elsewhere = resolve(
            &declared
                .clone()
                .with_workload_config(WorkloadConfigPolicy::Allow),
            "",
            &[("servers", "nats://elsewhere:4222")],
        )
        .unwrap();
        assert_eq!(
            elsewhere.get("servers").map(String::as_str),
            Some("nats://elsewhere:4222")
        );
        assert!(
            !elsewhere.contains_key("tls-ca"),
            "the data plane's certs say nothing about another NATS: {elsewhere:?}"
        );
    }

    /// An operator's declaration reaches the binding, and `deny` refuses a
    /// manifest that would point itself elsewhere.
    #[test]
    fn a_declared_binding_is_the_whole_allowlist() {
        let config = config_from(
            r#"
host:
  plugins:
    - id: wasmcloud-nats
      bindings:
        orders:
          config:
            subject-allow: orders.processed
"#,
        );
        let host = parse(&["--data-nats-url", "nats://data:4222"]);
        let declared = nats_bindings(&host, &config).unwrap();

        resolve(&declared, "orders", &[]).expect("a workload that only asks is served");
        resolve(&declared, "orders", &[("servers", "nats://elsewhere:4222")])
            .expect_err("but one that points itself at another cluster is not");
    }

    /// `allow` puts the host's declaration back under the manifest instead of
    /// around it. It is written on the entry now, not on a flag.
    #[test]
    fn allow_lets_a_manifest_describe_its_own_binding() {
        let config = config_from(
            r#"
host:
  plugins:
    - id: wasmcloud-nats
      workloadConfig: allow
"#,
        );
        let host = parse(&["--data-nats-url", "nats://data:4222"]);
        let declared = nats_bindings(&host, &config).unwrap();

        let resolved = resolve(&declared, "", &[("subject-allow", "orders.>")])
            .expect("allow accepts a workload's own grant");
        assert_eq!(
            resolved.get("subject-allow").map(String::as_str),
            Some("orders.>")
        );
    }

    /// An unparseable policy is a config error, named as one.
    #[test]
    fn the_workload_config_value_is_typed() {
        serde_yaml_ng::from_str::<crate::config::Config>(
            "host:\n  plugins:\n    - id: wasmcloud-nats\n      workloadConfig: sometimes\n",
        )
        .expect_err("`sometimes` is not a policy");
    }
}

#[cfg(all(test, feature = "host-component-plugins"))]
mod tests {
    use super::host_plugin_registry_credentials;

    #[test]
    fn both_halves_yield_credentials() {
        assert_eq!(
            host_plugin_registry_credentials(Some("user"), Some("pass")),
            Some(("user".to_string(), "pass".to_string())),
        );
    }

    #[test]
    fn neither_half_yields_no_credentials() {
        assert_eq!(host_plugin_registry_credentials(None, None), None);
    }

    #[test]
    fn a_half_pair_is_ignored_not_half_applied() {
        // Only a username, or only a password, must resolve to no explicit
        // credentials — never a basic auth with an empty half.
        assert_eq!(host_plugin_registry_credentials(Some("user"), None), None);
        assert_eq!(host_plugin_registry_credentials(None, Some("pass")), None);
    }
}

/// CLI spelling of [`wash_runtime::engine::guest_memory::GuestMemoryMode`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GuestMemoryMode {
    /// Charge and report guest memory growth; allow it either way.
    #[default]
    Count,
    /// Refuse guest memory growth past `--max-guest-memory`.
    Enforce,
}

/// [`GuestMemoryMode`] from a flag or environment value, reading a blank one
/// as unset.
fn parse_guest_memory_mode(raw: &str) -> Result<GuestMemoryMode, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(GuestMemoryMode::default());
    }
    match raw.to_ascii_lowercase().as_str() {
        "count" => Ok(GuestMemoryMode::Count),
        "enforce" => Ok(GuestMemoryMode::Enforce),
        _ => Err(format!(
            "invalid guest-memory-mode {raw:?}; expected 'count' or 'enforce'"
        )),
    }
}

impl From<GuestMemoryMode> for wash_runtime::engine::guest_memory::GuestMemoryMode {
    fn from(mode: GuestMemoryMode) -> Self {
        match mode {
            GuestMemoryMode::Count => Self::Count,
            GuestMemoryMode::Enforce => Self::Enforce,
        }
    }
}

/// CLI spelling of [`wash_runtime::sockets::policy::EgressMode`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum SocketEgressMode {
    /// Record what the policy would refuse; allow it anyway.
    #[default]
    Count,
    /// Refuse what the policy refuses.
    Enforce,
}

impl From<SocketEgressMode> for wash_runtime::sockets::policy::EgressMode {
    fn from(mode: SocketEgressMode) -> Self {
        match mode {
            SocketEgressMode::Count => Self::Count,
            SocketEgressMode::Enforce => Self::Enforce,
        }
    }
}

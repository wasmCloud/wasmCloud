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

    /// OTLP endpoint for host/platform telemetry.
    /// Only takes effect with --wasi-otel.
    #[arg(long = "otel-host-endpoint", env = "WASH_OTEL_HOST_ENDPOINT")]
    pub otel_host_endpoint: Option<String>,

    /// OTLP endpoint for workload/application telemetry emitted by components via wasi:otel.
    /// Only takes effect with --wasi-otel.
    #[arg(long = "otel-workload-endpoint", env = "WASH_OTEL_WORKLOAD_ENDPOINT")]
    pub otel_workload_endpoint: Option<String>,

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

impl CliCommand for HostCommand {
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
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

        // Parse the CA bundles before anything pulls, and fail if any is
        // invalid.
        if !self.oci_ca_paths.is_empty() {
            wash_runtime::oci::set_extra_ca_certificates(&self.oci_ca_paths)
                .context("failed to load --oci-ca-path CA certificates")?;
        }

        let host_config = wash_runtime::host::HostConfig {
            allow_oci_insecure: self.allow_insecure_registries,
            oci_pull_timeout: Some(self.registry_pull_timeout),
            oci_cache_dir: self.oci_cache_dir.clone(),
        };

        let mut engine_builder = Engine::builder()
            .with_pooling_allocator(true)
            .with_fuel_consumption(ctx.enable_meters());
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
            .with_meters(Meters::new(ctx.enable_meters()));

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
            let otel_config = plugin::wasi_otel::WasiOtelConfig::builder()
                .maybe_host(
                    self.otel_host_endpoint
                        .as_ref()
                        .map(|e| plugin::wasi_otel::OtelTarget::builder().endpoint(e).build()),
                )
                .maybe_workload(
                    self.otel_workload_endpoint
                        .as_ref()
                        .map(|e| plugin::wasi_otel::OtelTarget::builder().endpoint(e).build()),
                )
                .build();
            cluster_host_builder = cluster_host_builder
                .with_plugin(Arc::new(plugin::wasi_otel::WasiOtel::new(otel_config)))?;
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
            for hp in &config.host().host_plugins {
                specs.push(
                    hp.to_spec(&config, project_dir, Some(project_dir))
                        .with_context(|| format!("failed to resolve host_plugins '{}'", hp.id))?,
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
            self.host_plugins.is_empty() && config.host().host_plugins.is_empty(),
            "--host-plugin/WASH_HOST_PLUGINS/host.hostPlugins requires a wash build with the \
             `host-component-plugins` feature"
        );

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

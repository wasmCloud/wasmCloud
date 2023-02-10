use std::collections::HashMap;

use crate::up::{credsfile::parse_credsfile, NatsOpts, WasmcloudOpts};

pub const DOWNLOADS_DIR: &str = "downloads";
pub const WASMCLOUD_PID_FILE: &str = "wasmcloud.pid";
// NATS configuration values
pub(crate) const NATS_SERVER_VERSION: &str = "v2.9.14";
pub(crate) const DEFAULT_NATS_HOST: &str = "127.0.0.1";
pub(crate) const DEFAULT_NATS_PORT: &str = "4222";
// wasmCloud configuration values, https://wasmcloud.dev/reference/host-runtime/host_configure/
pub(crate) const WASMCLOUD_HOST_VERSION: &str = "v0.60.0";
// NATS isolation configuration variables
pub(crate) const WASMCLOUD_LATTICE_PREFIX: &str = "WASMCLOUD_LATTICE_PREFIX";
pub(crate) const DEFAULT_LATTICE_PREFIX: &str = "default";
pub(crate) const WASMCLOUD_JS_DOMAIN: &str = "WASMCLOUD_JS_DOMAIN";
// Host / Cluster configuration
pub(crate) const WASMCLOUD_CLUSTER_ISSUERS: &str = "WASMCLOUD_CLUSTER_ISSUERS";
pub(crate) const WASMCLOUD_CLUSTER_SEED: &str = "WASMCLOUD_CLUSTER_SEED";
pub(crate) const WASMCLOUD_HOST_SEED: &str = "WASMCLOUD_HOST_SEED";
// NATS RPC connection configuration
pub(crate) const WASMCLOUD_RPC_HOST: &str = "WASMCLOUD_RPC_HOST";
pub(crate) const WASMCLOUD_RPC_PORT: &str = "WASMCLOUD_RPC_PORT";
pub(crate) const WASMCLOUD_RPC_TIMEOUT_MS: &str = "WASMCLOUD_RPC_TIMEOUT_MS";
pub(crate) const DEFAULT_RPC_TIMEOUT_MS: &str = "2000";
pub(crate) const WASMCLOUD_RPC_JWT: &str = "WASMCLOUD_RPC_JWT";
pub(crate) const WASMCLOUD_RPC_SEED: &str = "WASMCLOUD_RPC_SEED";
pub(crate) const WASMCLOUD_RPC_CREDSFILE: &str = "WASMCLOUD_RPC_CREDSFILE";
pub(crate) const WASMCLOUD_RPC_TLS: &str = "WASMCLOUD_RPC_TLS";
// NATS CTL connection configuration
pub(crate) const WASMCLOUD_CTL_HOST: &str = "WASMCLOUD_CTL_HOST";
pub(crate) const WASMCLOUD_CTL_PORT: &str = "WASMCLOUD_CTL_PORT";
pub(crate) const WASMCLOUD_CTL_SEED: &str = "WASMCLOUD_CTL_SEED";
pub(crate) const WASMCLOUD_CTL_JWT: &str = "WASMCLOUD_CTL_JWT";
pub(crate) const WASMCLOUD_CTL_CREDSFILE: &str = "WASMCLOUD_CTL_CREDSFILE";
pub(crate) const WASMCLOUD_CTL_TLS: &str = "WASMCLOUD_CTL_TLS";
// NATS Provider RPC connection configuration
pub(crate) const WASMCLOUD_PROV_RPC_HOST: &str = "WASMCLOUD_PROV_RPC_HOST";
pub(crate) const WASMCLOUD_PROV_RPC_PORT: &str = "WASMCLOUD_PROV_RPC_PORT";
pub(crate) const WASMCLOUD_PROV_SHUTDOWN_DELAY_MS: &str = "WASMCLOUD_PROV_SHUTDOWN_DELAY_MS";
pub(crate) const DEFAULT_PROV_SHUTDOWN_DELAY_MS: &str = "300";
pub(crate) const WASMCLOUD_PROV_RPC_SEED: &str = "WASMCLOUD_PROV_RPC_SEED";
pub(crate) const WASMCLOUD_PROV_RPC_JWT: &str = "WASMCLOUD_PROV_RPC_JWT";
pub(crate) const WASMCLOUD_PROV_RPC_CREDSFILE: &str = "WASMCLOUD_PROV_RPC_CREDSFILE";
pub(crate) const WASMCLOUD_PROV_RPC_TLS: &str = "WASMCLOUD_PROV_RPC_TLS";
pub(crate) const WASMCLOUD_OCI_ALLOWED_INSECURE: &str = "WASMCLOUD_OCI_ALLOWED_INSECURE";
pub(crate) const WASMCLOUD_OCI_ALLOW_LATEST: &str = "WASMCLOUD_OCI_ALLOW_LATEST";
// Extra configuration (logs, IPV6, config service)
pub(crate) const WASMCLOUD_STRUCTURED_LOG_LEVEL: &str = "WASMCLOUD_STRUCTURED_LOG_LEVEL";
pub(crate) const DEFAULT_STRUCTURED_LOG_LEVEL: &str = "info";
pub(crate) const WASMCLOUD_ENABLE_IPV6: &str = "WASMCLOUD_ENABLE_IPV6";
pub(crate) const WASMCLOUD_STRUCTURED_LOGGING_ENABLED: &str =
    "WASMCLOUD_STRUCTURED_LOGGING_ENABLED";
pub(crate) const WASMCLOUD_CONFIG_SERVICE: &str = "WASMCLOUD_CONFIG_SERVICE";

/// Helper function to convert WasmcloudOpts to the host environment map.
/// Takes NatsOpts as well to provide reasonable defaults
pub(crate) async fn configure_host_env(
    nats_opts: NatsOpts,
    wasmcloud_opts: WasmcloudOpts,
) -> HashMap<String, String> {
    let mut host_config = HashMap::new();
    // NATS isolation configuration variables
    host_config.insert(
        WASMCLOUD_LATTICE_PREFIX.to_string(),
        wasmcloud_opts.lattice_prefix,
    );
    if let Some(js_domain) = wasmcloud_opts.wasmcloud_js_domain {
        host_config.insert(WASMCLOUD_JS_DOMAIN.to_string(), js_domain);
    }

    // Host / Cluster configuration
    if let Some(seed) = wasmcloud_opts.host_seed {
        host_config.insert(WASMCLOUD_HOST_SEED.to_string(), seed);
    }
    if let Some(seed) = wasmcloud_opts.cluster_seed {
        host_config.insert(WASMCLOUD_CLUSTER_SEED.to_string(), seed);
    }
    if let Some(cluster_issuers) = wasmcloud_opts.cluster_issuers {
        host_config.insert(
            WASMCLOUD_CLUSTER_ISSUERS.to_string(),
            cluster_issuers.join(","),
        );
    }

    if wasmcloud_opts.allow_latest {
        host_config.insert(WASMCLOUD_OCI_ALLOW_LATEST.to_string(), "true".to_string());
    }
    if let Some(allowed_insecure) = wasmcloud_opts.allowed_insecure {
        host_config.insert(
            WASMCLOUD_OCI_ALLOWED_INSECURE.to_string(),
            allowed_insecure.join(","),
        );
    }

    // NATS RPC connection configuration
    if let Some(host) = wasmcloud_opts.rpc_host {
        host_config.insert(WASMCLOUD_RPC_HOST.to_string(), host);
    } else {
        host_config.insert(WASMCLOUD_RPC_HOST.to_string(), nats_opts.nats_host.clone());
    }
    if let Some(port) = wasmcloud_opts.rpc_port {
        host_config.insert(WASMCLOUD_RPC_PORT.to_string(), port.to_string());
    } else {
        host_config.insert(
            WASMCLOUD_RPC_PORT.to_string(),
            nats_opts.nats_port.to_string(),
        );
    }

    host_config.insert(
        WASMCLOUD_RPC_TIMEOUT_MS.to_string(),
        wasmcloud_opts.rpc_timeout_ms.to_string(),
    );
    if let Some(path) = wasmcloud_opts.rpc_credsfile {
        if let Ok((jwt, seed)) = parse_credsfile(path).await {
            host_config.insert(WASMCLOUD_RPC_JWT.to_string(), jwt);
            host_config.insert(WASMCLOUD_RPC_SEED.to_string(), seed);
        };
    } else {
        if let Some(jwt) = wasmcloud_opts.rpc_jwt {
            host_config.insert(WASMCLOUD_RPC_JWT.to_string(), jwt);
        }
        if let Some(seed) = wasmcloud_opts.rpc_seed {
            host_config.insert(WASMCLOUD_RPC_SEED.to_string(), seed);
        }
    }
    if wasmcloud_opts.rpc_tls {
        host_config.insert(WASMCLOUD_RPC_TLS.to_string(), "1".to_string());
    }

    // NATS CTL connection configuration
    if let Some(host) = wasmcloud_opts.ctl_host {
        host_config.insert(WASMCLOUD_CTL_HOST.to_string(), host);
    } else {
        host_config.insert(WASMCLOUD_CTL_HOST.to_string(), nats_opts.nats_host.clone());
    }
    if let Some(port) = wasmcloud_opts.ctl_port {
        host_config.insert(WASMCLOUD_CTL_PORT.to_string(), port.to_string());
    } else {
        host_config.insert(
            WASMCLOUD_CTL_PORT.to_string(),
            nats_opts.nats_port.to_string(),
        );
    }
    if let Some(path) = wasmcloud_opts.ctl_credsfile {
        if let Ok((jwt, seed)) = parse_credsfile(path).await {
            host_config.insert(WASMCLOUD_CTL_JWT.to_string(), jwt);
            host_config.insert(WASMCLOUD_CTL_SEED.to_string(), seed);
        };
    } else {
        if let Some(seed) = wasmcloud_opts.ctl_seed {
            host_config.insert(WASMCLOUD_CTL_SEED.to_string(), seed);
        }
        if let Some(jwt) = wasmcloud_opts.ctl_jwt {
            host_config.insert(WASMCLOUD_CTL_JWT.to_string(), jwt);
        }
    }
    if wasmcloud_opts.ctl_tls {
        host_config.insert(WASMCLOUD_CTL_TLS.to_string(), "1".to_string());
    }

    // NATS Provider RPC connection configuration
    if let Some(host) = wasmcloud_opts.prov_rpc_host {
        host_config.insert(WASMCLOUD_PROV_RPC_HOST.to_string(), host);
    } else {
        host_config.insert(WASMCLOUD_PROV_RPC_HOST.to_string(), nats_opts.nats_host);
    }
    if let Some(port) = wasmcloud_opts.prov_rpc_port {
        host_config.insert(WASMCLOUD_PROV_RPC_PORT.to_string(), port.to_string());
    } else {
        host_config.insert(
            WASMCLOUD_PROV_RPC_PORT.to_string(),
            nats_opts.nats_port.to_string(),
        );
    }
    if let Some(path) = wasmcloud_opts.prov_rpc_credsfile {
        if let Ok((jwt, seed)) = parse_credsfile(path).await {
            host_config.insert(WASMCLOUD_PROV_RPC_JWT.to_string(), jwt);
            host_config.insert(WASMCLOUD_PROV_RPC_SEED.to_string(), seed);
        };
    } else {
        if let Some(seed) = wasmcloud_opts.prov_rpc_seed {
            host_config.insert(WASMCLOUD_PROV_RPC_SEED.to_string(), seed);
        }
        if let Some(jwt) = wasmcloud_opts.prov_rpc_jwt {
            host_config.insert(WASMCLOUD_PROV_RPC_JWT.to_string(), jwt);
        }
    }
    if wasmcloud_opts.prov_rpc_tls {
        host_config.insert(WASMCLOUD_PROV_RPC_TLS.to_string(), "1".to_string());
    }
    host_config.insert(
        WASMCLOUD_PROV_SHUTDOWN_DELAY_MS.to_string(),
        wasmcloud_opts.provider_delay.to_string(),
    );

    // Extras configuration
    if wasmcloud_opts.config_service_enabled {
        host_config.insert(WASMCLOUD_CONFIG_SERVICE.to_string(), "1".to_string());
    }
    if wasmcloud_opts.enable_structured_logging {
        host_config.insert(
            WASMCLOUD_STRUCTURED_LOGGING_ENABLED.to_string(),
            "true".to_string(),
        );
    }
    host_config.insert(
        WASMCLOUD_STRUCTURED_LOG_LEVEL.to_string(),
        wasmcloud_opts.structured_log_level,
    );
    if wasmcloud_opts.enable_ipv6 {
        host_config.insert(WASMCLOUD_ENABLE_IPV6.to_string(), "1".to_string());
    }
    host_config
}

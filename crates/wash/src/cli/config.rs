use std::collections::HashMap;

use anyhow::{anyhow, Result};

use crate::cmd::up::WasmcloudOpts;
use crate::creds::parse_credsfile;

// NATS configuration values
pub const NATS_SERVER_VERSION: &str = "v2.10.20";
pub const DEFAULT_NATS_HOST: &str = "127.0.0.1";
pub const DEFAULT_NATS_PORT: &str = "4222";
pub const DEFAULT_NATS_WEBSOCKET_PORT: &str = "4223";

// wadm configuration values
pub const WADM_VERSION: &str = "v0.20.2";

// wasmCloud configuration values, https://wasmcloud.com/docs/reference/host-config
pub const WASMCLOUD_HOST_VERSION: &str = "v1.6.1";

// NATS isolation configuration variables
pub const WASMCLOUD_LATTICE: &str = "WASMCLOUD_LATTICE";
pub const DEFAULT_LATTICE: &str = "default";
pub const WASMCLOUD_JS_DOMAIN: &str = "WASMCLOUD_JS_DOMAIN";
pub const WASMCLOUD_POLICY_TOPIC: &str = "WASMCLOUD_POLICY_TOPIC";
pub const WASMCLOUD_SECRETS_TOPIC: &str = "WASMCLOUD_SECRETS_TOPIC";

// Host / Cluster configuration
pub const WASMCLOUD_CLUSTER_ISSUERS: &str = "WASMCLOUD_CLUSTER_ISSUERS";
pub const WASMCLOUD_CLUSTER_SEED: &str = "WASMCLOUD_CLUSTER_SEED";
pub const WASMCLOUD_HOST_SEED: &str = "WASMCLOUD_HOST_SEED";
pub const WASMCLOUD_MAX_EXECUTION_TIME_MS: &str = "WASMCLOUD_MAX_EXECUTION_TIME_MS";
pub const DEFAULT_MAX_EXECUTION_TIME_MS: &str = "600000";

// NATS RPC connection configuration
pub const WASMCLOUD_RPC_HOST: &str = "WASMCLOUD_RPC_HOST";
pub const WASMCLOUD_RPC_PORT: &str = "WASMCLOUD_RPC_PORT";
pub const WASMCLOUD_RPC_TIMEOUT_MS: &str = "WASMCLOUD_RPC_TIMEOUT_MS";
pub const DEFAULT_RPC_TIMEOUT_MS: &str = "2000";
pub const WASMCLOUD_RPC_JWT: &str = "WASMCLOUD_RPC_JWT";
pub const WASMCLOUD_RPC_SEED: &str = "WASMCLOUD_RPC_SEED";
pub const WASMCLOUD_RPC_CREDSFILE: &str = "WASMCLOUD_RPC_CREDSFILE";
pub const WASMCLOUD_RPC_TLS: &str = "WASMCLOUD_RPC_TLS";
pub const WASMCLOUD_RPC_TLS_FIRST: &str = "WASMCLOUD_RPC_TLS_FIRST";
pub const WASMCLOUD_RPC_TLS_CA_FILE: &str = "WASMCLOUD_RPC_TLS_CA_FILE";

// NATS CTL connection configuration
pub const WASMCLOUD_CTL_HOST: &str = "WASMCLOUD_CTL_HOST";
pub const WASMCLOUD_CTL_PORT: &str = "WASMCLOUD_CTL_PORT";
pub const WASMCLOUD_CTL_SEED: &str = "WASMCLOUD_CTL_SEED";
pub const WASMCLOUD_CTL_JWT: &str = "WASMCLOUD_CTL_JWT";
pub const WASMCLOUD_CTL_CREDSFILE: &str = "WASMCLOUD_CTL_CREDSFILE";
pub const WASMCLOUD_CTL_TLS: &str = "WASMCLOUD_CTL_TLS";
pub const WASMCLOUD_CTL_TLS_FIRST: &str = "WASMCLOUD_CTL_TLS_FIRST";
pub const WASMCLOUD_CTL_TLS_CA_FILE: &str = "WASMCLOUD_CTL_TLS_CA_FILE";

// NATS Provider RPC connection configuration
pub const WASMCLOUD_PROV_SHUTDOWN_DELAY_MS: &str = "WASMCLOUD_PROV_SHUTDOWN_DELAY_MS";
pub const DEFAULT_PROV_SHUTDOWN_DELAY_MS: &str = "300";
pub const WASMCLOUD_OCI_ALLOWED_INSECURE: &str = "WASMCLOUD_OCI_ALLOWED_INSECURE";
pub const WASMCLOUD_OCI_ALLOW_LATEST: &str = "WASMCLOUD_OCI_ALLOW_LATEST";

// Extra configuration (logs, IPV6, config service)
pub const WASMCLOUD_LOG_LEVEL: &str = "WASMCLOUD_LOG_LEVEL";
pub const WASMCLOUD_HOST_LOG_PATH: &str = "WASMCLOUD_HOST_LOG_PATH";
pub const WASMCLOUD_HOST_PATH: &str = "WASMCLOUD_HOST_PATH";
pub const DEFAULT_STRUCTURED_LOG_LEVEL: &str = "info";
pub const WASMCLOUD_ENABLE_IPV6: &str = "WASMCLOUD_ENABLE_IPV6";
pub const WASMCLOUD_STRUCTURED_LOGGING_ENABLED: &str = "WASMCLOUD_STRUCTURED_LOGGING_ENABLED";
pub const WASMCLOUD_CONFIG_SERVICE: &str = "WASMCLOUD_CONFIG_SERVICE";
pub const WASMCLOUD_ALLOW_FILE_LOAD: &str = "WASMCLOUD_ALLOW_FILE_LOAD";
pub const DEFAULT_ALLOW_FILE_LOAD: &str = "true";

/// Helper function to convert `WasmcloudOpts` to the host environment map.
/// Takes `NatsOpts` as well to provide reasonable defaults
pub async fn configure_host_env(wasmcloud_opts: WasmcloudOpts) -> Result<HashMap<String, String>> {
    let mut host_config = HashMap::new();
    // NATS isolation configuration variables
    host_config.insert(
        WASMCLOUD_LATTICE.to_string(),
        wasmcloud_opts
            .lattice
            .unwrap_or(DEFAULT_LATTICE.to_string()),
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
    host_config.insert(
        WASMCLOUD_MAX_EXECUTION_TIME_MS.to_string(),
        wasmcloud_opts.max_execution_time.to_string(),
    );
    if let Some(policy_topic) = wasmcloud_opts.policy_topic {
        host_config.insert(WASMCLOUD_POLICY_TOPIC.to_string(), policy_topic);
    }

    if let Some(secrets_topic) = wasmcloud_opts.secrets_topic {
        host_config.insert(
            WASMCLOUD_SECRETS_TOPIC.to_string(),
            secrets_topic,
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
    }
    if let Some(port) = wasmcloud_opts.rpc_port {
        host_config.insert(WASMCLOUD_RPC_PORT.to_string(), port.to_string());
    }
    if let Some(rpc_timeout_ms) = wasmcloud_opts.rpc_timeout_ms {
        host_config.insert(
            WASMCLOUD_RPC_TIMEOUT_MS.to_string(),
            rpc_timeout_ms.to_string(),
        );
    }
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
        host_config.insert(WASMCLOUD_RPC_TLS.to_string(), "true".to_string());
    }

    // NATS CTL connection configuration
    if let Some(host) = wasmcloud_opts.ctl_host {
        host_config.insert(WASMCLOUD_CTL_HOST.to_string(), host);
    }
    if let Some(port) = wasmcloud_opts.ctl_port {
        host_config.insert(WASMCLOUD_CTL_PORT.to_string(), port.to_string());
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
        host_config.insert(WASMCLOUD_CTL_TLS.to_string(), "true".to_string());
    }

    host_config.insert(
        WASMCLOUD_PROV_SHUTDOWN_DELAY_MS.to_string(),
        wasmcloud_opts.provider_delay.to_string(),
    );

    // Extras configuration
    if wasmcloud_opts.config_service_enabled {
        host_config.insert(WASMCLOUD_CONFIG_SERVICE.to_string(), "true".to_string());
    }
    if wasmcloud_opts.allow_file_load.unwrap_or_default() {
        host_config.insert(WASMCLOUD_ALLOW_FILE_LOAD.to_string(), "true".to_string());
    }
    if wasmcloud_opts.enable_structured_logging {
        host_config.insert(
            WASMCLOUD_STRUCTURED_LOGGING_ENABLED.to_string(),
            "true".to_string(),
        );
    }

    let labels: Vec<(String, String)> = wasmcloud_opts
        .label
        .unwrap_or_default()
        .iter()
        .map(
            |labelpair| match labelpair.split('=').collect::<Vec<&str>>()[..] {
                [k, v] => Ok((k.to_string(), v.to_string())),
                _ => Err(anyhow!(
                    "invalid label format `{labelpair}`. Expected `key=value`"
                )),
            },
        )
        .collect::<Result<Vec<(String, String)>>>()?;
    for (key, value) in labels {
        host_config.insert(format!("WASMCLOUD_LABEL_{key}"), value.to_string());
    }

    host_config.insert(
        WASMCLOUD_LOG_LEVEL.to_string(),
        wasmcloud_opts.structured_log_level,
    );
    if wasmcloud_opts.enable_ipv6 {
        host_config.insert(WASMCLOUD_ENABLE_IPV6.to_string(), "true".to_string());
    }
    Ok(host_config)
}

//! Configuration for sqldb-postgres capability provider
//!
use std::{str::FromStr, time::Duration};

use bb8_postgres::{bb8, tokio_postgres, tokio_postgres::NoTls};
use serde::Deserialize;
use wasmbus_rpc::{core::LinkDefinition, error::RpcError};

/// Configuration for this provider (from link definitions)
/// For complete documentation on the uri syntax,
///    see https://docs.rs/tokio-postgres/0.7.2/tokio_postgres/config/struct.Config.html
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Config {
    /// Database connection uri
    uri: String,
    /// Optional path to root cert (for TLS)
    #[allow(dead_code)]
    root_cert: Option<String>,
    /// Optional connection pool information
    #[serde(default)]
    pool: PoolOptions,
}

/// max size of connection pool
const DEFAULT_MAX_CONNECTIONS: u32 = 8;
/// minimum number of idle connections to maintain in reserve
const DEFAULT_MIN_IDLE: u32 = 0;
/// when connection reaches this age, and after it has finished
/// its current request, it will be dropped, so its associated
/// resources (caches, etc.) can be cleaned up.
const DEFAULT_MAX_LIFETIME_SEC: u32 = 7200;
/// amount of time a connection can remain unused before it
/// is closed and reclaimed (subject to DEFAULT_MIN_IDLE)
const DEFAULT_IDLE_TIMEOUT_SEC: u32 = 600;
/// period of inactivity after which keepalive message is sent to the backend server
/// This value is not configurable in settings
const DEFAULT_KEEPALIVE_IDLE: Duration = Duration::from_secs(15 * 60);
/// amount of time to wait to receive a connection from the pool
const DEFAULT_CONNECTION_TIMEOUT_MILLIS: u32 = 1000;

/// Options for configuring connection pool
#[derive(Debug, Default, Deserialize)]
pub(crate) struct PoolOptions {
    /// sets the maximum number of connections the pool should maintain
    /// Default: 8
    max_connections: Option<u32>,

    /// sets the minimum number of idle connections the pool should maintain
    /// Default: 0 (None)
    min_idle: Option<u32>,

    /// sets the maximum lifetime of individual connections.
    /// any connection with a lifetime greater than this will be closed.
    /// It is usually recommended to retire connections periodiically to
    /// clean up data structures and caches associated with a session.
    /// Default: 7200 (2 hours)
    max_lifetime_secs: Option<u32>,

    /// maximum idle duration for an individual connection.
    /// Any connection with an idle timeout longer than this will be closed.
    /// For usage-based database server billing, this can be a cost saver.
    /// Default: 600 (10 minutes)
    idle_timeout_secs: Option<u32>,

    /// maximum time to wait for a connection from the pool before assuming
    /// the database isunreachable.
    /// Default: 1000ms
    connection_timeout_millis: Option<u32>,
}

/// Load configuration from 'values' field of LinkDefinition.
/// Support a variety of configuration possibilities:
///  'uri' (only) - sets the uri, and uses a default connection pool
///  'config_json' - json with 'uri' and 'pool' settings
///  'config_b64' - base64-encoded json wih 'uri' and 'pool' settings
pub(crate) fn load_config(ld: &LinkDefinition) -> Result<Config, RpcError> {
    let mut config = Config::default();
    if let Some(cj) = ld.values.get("config_b64") {
        config = serde_json::from_slice(
            &base64::decode(cj)
                .map_err(|()| RpcError::ProviderInit("invalid config_base64 encoding".into()))?,
        )
        .map_err(|e| RpcError::ProviderInit(format!("invalid json config: {}", e)))?;
    }
    if let Some(cj) = ld.values.get("config_json") {
        config = serde_json::from_str(cj.as_str())
            .map_err(|e| RpcError::ProviderInit(format!("invalid json config: {}", e)))?;
    }
    if let Some(uri) = ld.values.get("uri") {
        config.uri = uri.to_string();
    }
    if config.uri.is_empty() {
        Err(RpcError::ProviderInit(
            "link params values are missing 'uri'".into(),
        ))
    } else {
        Ok(config)
    }
}

/// Create the connection pool based on config settings. This function will not return
/// until the required number of idle connections has been established.
pub(crate) async fn create_pool(config: Config) -> Result<crate::Pool, RpcError> {
    let mut pg_config = tokio_postgres::Config::from_str(&config.uri)
        .map_err(|e| RpcError::ProviderInit(format!("Invalid db connect string: {}", e)))?;
    pg_config.keepalives_idle(DEFAULT_KEEPALIVE_IDLE);

    /*
    let tls = if let Some(path) = config.root_cert {
        let cert = std::fs::read(&path)?;
        let cert = Certificate::fom_pem(&cert);
        TlsConnector::builder()
            .add_root_certificate(cert).build()?
    } else {
        MakeTlsConnector::new(TlsConnector::default())
    };
     */

    let pool = bb8::Builder::new()
        .max_size(
            config
                .pool
                .max_connections
                .unwrap_or(DEFAULT_MAX_CONNECTIONS),
        )
        .min_idle(Some(config.pool.min_idle.unwrap_or(DEFAULT_MIN_IDLE)))
        .max_lifetime(Some(std::time::Duration::from_secs(
            config
                .pool
                .max_lifetime_secs
                .unwrap_or(DEFAULT_MAX_LIFETIME_SEC) as u64,
        )))
        .idle_timeout(Some(std::time::Duration::from_secs(
            config
                .pool
                .idle_timeout_secs
                .unwrap_or(DEFAULT_IDLE_TIMEOUT_SEC) as u64,
        )))
        .connection_timeout(std::time::Duration::from_millis(
            config
                .pool
                .connection_timeout_millis
                .unwrap_or(DEFAULT_CONNECTION_TIMEOUT_MILLIS) as u64,
        ))
        .build(bb8_postgres::PostgresConnectionManager::new(
            pg_config, NoTls,
        ))
        .await
        .map_err(|e| RpcError::ProviderInit(format!("initializing db connection pool: {}", e)))?;
    Ok(pool)
}

use tracing::warn;
use wasmcloud_provider_sdk::{core::secrets::SecretValue, types::InterfaceConfig};

const POSTGRES_DEFAULT_PORT: u16 = 5432;

/// Creation options for a Postgres connection
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectionCreateOptions {
    /// Hostname of the Postgres cluster to connect to
    pub host: String,
    /// Port on which to connect to the Postgres cluster
    pub port: u16,
    /// Username used when accessing the Postgres cluster
    pub username: String,
    /// Password used when accessing the Postgres cluster
    pub password: String,
    /// Database to connect to
    pub database: String,
    /// Whether TLS is required for the connection
    pub tls_required: bool,
    /// Optional connection pool size
    pub pool_size: Option<usize>,
}

impl From<ConnectionCreateOptions> for deadpool_postgres::Config {
    fn from(opts: ConnectionCreateOptions) -> Self {
        let mut cfg = deadpool_postgres::Config::new();
        cfg.host = Some(opts.host);
        cfg.user = Some(opts.username);
        cfg.password = Some(opts.password);
        cfg.dbname = Some(opts.database);
        cfg.port = Some(opts.port);
        if let Some(pool_size) = opts.pool_size {
            cfg.pool = Some(deadpool_postgres::PoolConfig {
                max_size: pool_size,
                ..deadpool_postgres::PoolConfig::default()
            });
        }
        cfg
    }
}

/// Parse the options for Postgres configuration from a [`HashMap`], with a given prefix to the keys
///
/// For example given a prefix like `EXAMPLE_`, and a Hashmap that contains an entry like ("EXAMPLE_HOST", "localhost"),
/// the parsed [`ConnectionCreateOptions`] would contain "localhost" as the host.
pub(crate) fn extract_prefixed_conn_config(
    prefix: &str,
    link_config: &InterfaceConfig,
) -> Option<ConnectionCreateOptions> {
    // Convert config Vec to HashMap for easier access
    let config: std::collections::HashMap<String, String> =
        link_config.config.iter().cloned().collect();
    let secrets = &link_config.secrets;

    // Helper to get secret value by key
    let get_secret = |k: &str| -> Option<String> {
        secrets
            .as_ref()
            .and_then(|s| s.iter().find(|(key, _)| key == k))
            .and_then(|(_, v)| {
                let secret: SecretValue = v.into();
                secret.as_string().map(String::from)
            })
    };

    let keys = [
        format!("{prefix}HOST"),
        format!("{prefix}PORT"),
        format!("{prefix}USERNAME"),
        format!("{prefix}PASSWORD"),
        format!("{prefix}DATABASE"),
        format!("{prefix}TLS_REQUIRED"),
        format!("{prefix}POOL_SIZE"),
    ];
    match &keys
        .iter()
        .map(|k| {
            // Prefer fetching from secrets, but fall back to config if not found
            match (get_secret(k), config.get(k)) {
                (Some(s), Some(_)) => {
                    warn!("secret value [{k}] was found in secrets, but also exists in config. The value in secrets will be used.");
                    Some(s)
                }
                (Some(s), _) => Some(s),
                // Offer a warning for the password, but other values are fine to be in config
                (None, Some(c)) if k == &format!("{prefix}PASSWORD") => {
                    warn!("secret value [{k}] was not found in secrets, but exists in config. Prefer using secrets for sensitive values.");
                    Some(c.clone())
                }
                (None, Some(c)) => {
                    Some(c.clone())
                }
                (_, None) => None,
            }
        })
        .collect::<Vec<Option<String>>>()[..]
    {
        [Some(host), Some(port), Some(username), Some(password), Some(database), tls_required, pool_size] =>
        {
            let pool_size = pool_size.as_ref().and_then(|pool_size| {
                pool_size.parse::<usize>().ok().or_else(|| {
                    warn!("invalid pool size value [{pool_size}], using default");
                    None
                })
            });

            Some(ConnectionCreateOptions {
                host: host.to_string(),
                port: port.parse::<u16>().unwrap_or_else(|_e| {
                    warn!("invalid port value [{port}], using {POSTGRES_DEFAULT_PORT}");
                    POSTGRES_DEFAULT_PORT
                }),
                username: username.to_string(),
                password: password.to_string(),
                tls_required: tls_required.as_ref().is_some_and(|tls_required| {
                    matches!(tls_required.to_lowercase().as_str(), "true" | "yes")
                }),
                database: database.to_string(),
                pool_size,
            })
        }
        _ => {
            warn!("failed to find required keys in configuration: [{:?}]", keys);
            None
        }
    }
}

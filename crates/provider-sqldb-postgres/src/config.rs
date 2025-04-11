use tracing::warn;
use wasmcloud_provider_sdk::{core::secrets::SecretValue, LinkConfig};

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
    link_config: &LinkConfig,
) -> Option<ConnectionCreateOptions> {
    let LinkConfig {
        config, secrets, ..
    } = link_config;

    let keys = [
        format!("{prefix}HOST"),
        format!("{prefix}PORT"),
        format!("{prefix}USERNAME"),
        format!("{prefix}PASSWORD"),
        format!("{prefix}DATABASE"),
        format!("{prefix}TLS_REQUIRED"),
        format!("{prefix}POOL_SIZE"),
    ];
    match keys
        .iter()
        .map(|k| config.get(k))
        .collect::<Vec<Option<&String>>>()[..]
    {
        [Some(host), Some(port), Some(username), config_password, Some(database), tls_required, pool_size] =>
        {
            let secret_password = secrets
                .get(&format!("{prefix}PASSWORD"))
                .and_then(SecretValue::as_string);
            // Check that the password was pulled from secrets, not config
            let password = match (secret_password, config_password) {
                (Some(s), _) => s,
                (None, Some(c)) => {
                    warn!("secret value [{prefix}PASSWORD] was not found in secrets, but exists in config. Prefer using secrets for sensitive values.", );
                    c
                }
                (_, None) => {
                    warn!("failed to find password in config and secrets");
                    return None;
                }
            };

            let pool_size = pool_size.and_then(|pool_size| {
                pool_size.as_str().parse::<usize>().ok().or_else(|| {
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
                tls_required: tls_required.is_some_and(|tls_required| {
                    matches!(tls_required.to_lowercase().as_str(), "true" | "yes")
                }),
                database: database.to_string(),
                pool_size,
            })
        }
        _ => {
            warn!("failed to find keys in configuration: [{:?}]", keys);
            None
        }
    }
}

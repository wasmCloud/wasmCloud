use std::collections::HashMap;

use tracing::warn;

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
}

impl From<ConnectionCreateOptions> for deadpool_postgres::Config {
    fn from(opts: ConnectionCreateOptions) -> Self {
        let mut cfg = deadpool_postgres::Config::new();
        cfg.host = Some(opts.host);
        cfg.user = Some(opts.username);
        cfg.password = Some(opts.password);
        cfg.dbname = Some(opts.database);
        cfg.port = Some(opts.port);
        cfg
    }
}

/// Parse the options for Postgres configuration from a [`HashMap`], with a given prefix to the keys
///
/// For example given a prefix like `EXAMPLE_`, and a Hashmap that contains an entry like ("EXAMPLE_HOST", "localhost"),
/// the parsed [`ConnectionCreateOptions`] would contain "localhost" as the host.
pub(crate) fn parse_prefixed_config_from_map(
    prefix: &str,
    config: &HashMap<String, String>,
) -> Option<ConnectionCreateOptions> {
    let keys = [
        format!("{prefix}HOST"),
        format!("{prefix}PORT"),
        format!("{prefix}USERNAME"),
        format!("{prefix}PASSWORD"),
        format!("{prefix}DATABASE"),
        format!("{prefix}TLS_REQUIRED"),
    ];
    match keys
        .iter()
        .map(|k| config.get(k))
        .collect::<Vec<Option<&String>>>()[..]
    {
        [Some(host), Some(port), Some(username), Some(password), Some(database), Some(tls_required)] => {
            Some(ConnectionCreateOptions {
                host: host.to_string(),
                port: port.parse::<u16>().unwrap_or_else(|_e| {
                    warn!("invalid port value [{port}], using {POSTGRES_DEFAULT_PORT}");
                    POSTGRES_DEFAULT_PORT
                }),
                username: username.to_string(),
                password: password.to_string(),
                tls_required: matches!(tls_required.to_lowercase().as_str(), "true" | "yes"),
                database: database.to_string(),
            })
        }
        _ => {
            warn!("failed to find keys in configuration: [{:?}]", keys);
            None
        }
    }
}

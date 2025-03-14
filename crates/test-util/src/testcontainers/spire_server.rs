use testcontainers::{
    core::{ContainerPort, WaitFor},
    CopyDataSource, CopyToContainer, Image,
};

const SPIRE_SERVER_CONFIG_PATH: &str = "/opt/spire/conf/server/server.conf";

// based on https://github.com/spiffe/spire/blob/v1.11.2/conf/server/server_container.conf
const SPIRE_SERVER_CONFIG: &str = r#"server {
    bind_address = "0.0.0.0"
    bind_port = "8081"
    socket_path = "/tmp/spire-server/private/api.sock"
    trust_domain = "wasmcloud.dev"
    data_dir = "/var/lib/spire/server/.data"
    log_level = "DEBUG"
}

plugins {
    DataStore "sql" {
        plugin_data {
            database_type = "sqlite3"
            connection_string = "/var/lib/spire/server/.data/datastore.sqlite3"
        }
    }

    NodeAttestor "join_token" {
        plugin_data {
        }
    }

    KeyManager "memory" {
        plugin_data = {}
    }

    UpstreamAuthority "disk" {
        plugin_data {
            key_file_path = "/etc/spire/server/dummy_upstream_ca.key"
            cert_file_path = "/etc/spire/server/dummy_upstream_ca.crt"
        }
    }
}"#;

const SPIRE_SERVER_DUMMY_CA_CERT_PATH: &str = "/etc/spire/server/dummy_upstream_ca.crt";
// https://github.com/spiffe/spire/blob/v1.11.2/conf/server/dummy_upstream_ca.crt
const SPIRE_SERVER_DUMMY_CA_CERT: &str = r#"
-----BEGIN CERTIFICATE-----
MIIB2DCCAV6gAwIBAgIURJ20yIzal3ZT9NXkdwrsm0selwwwCgYIKoZIzj0EAwQw
HjELMAkGA1UEBhMCVVMxDzANBgNVBAoMBlNQSUZGRTAeFw0yMzA1MTUwMjA1MDZa
Fw0yODA1MTMwMjA1MDZaMB4xCzAJBgNVBAYTAlVTMQ8wDQYDVQQKDAZTUElGRkUw
djAQBgcqhkjOPQIBBgUrgQQAIgNiAAT1cHO3Lxb97HhevRF3NQGCJ7+iR1pROF5I
XQ9C9UBpOxdo/UnvK/QOGVrDjkjsK/0c/bUc6YzEiVnRd6qw6X2wzkfnscFBa7Rs
g1d/DiN14d0Hm+TVfI3IFBDF5SlLGGejXTBbMB0GA1UdDgQWBBSSiuNgxqqnz2r/
jRcWsARqphwQ/zAPBgNVHRMBAf8EBTADAQH/MA4GA1UdDwEB/wQEAwIBBjAZBgNV
HREEEjAQhg5zcGlmZmU6Ly9sb2NhbDAKBggqhkjOPQQDBANoADBlAjEA54Q8hfhE
d4qVycwbLNzOm/HQrp1n1+a2xc88iU036FMPancR1PLqgsODPfWyttdRAjAKIodU
i4eYiMa9+I2rVbj8gOxJAFn0hLLEF3QDmXtGPpARs9qC+KbiklTu5Fpik2Q=
-----END CERTIFICATE-----
"#;

const SPIRE_SERVER_DUMMY_CA_KEY_PATH: &str = "/etc/spire/server/dummy_upstream_ca.key";
// https://github.com/spiffe/spire/blob/v1.11.2/conf/server/dummy_upstream_ca.key
const SPIRE_SERVER_DUMMY_CA_KEY: &str = r#"
-----BEGIN EC PRIVATE KEY-----
MIGkAgEBBDAfomBEfciCLhcaSQeaDSf0lsxSaRGooKp0r1MSzYkk6uV57cYRT2O9
VR6wqqtiBEmgBwYFK4EEACKhZANiAAT1cHO3Lxb97HhevRF3NQGCJ7+iR1pROF5I
XQ9C9UBpOxdo/UnvK/QOGVrDjkjsK/0c/bUc6YzEiVnRd6qw6X2wzkfnscFBa7Rs
g1d/DiN14d0Hm+TVfI3IFBDF5SlLGGc=
-----END EC PRIVATE KEY-----
"#;

#[derive(Debug, Clone)]
pub struct SpireServer {
    _priv: (),
    copy_to_sources: Vec<CopyToContainer>,
}

impl Default for SpireServer {
    fn default() -> Self {
        Self {
            _priv: (),
            copy_to_sources: vec![
                CopyToContainer::new(
                    CopyDataSource::Data(SPIRE_SERVER_CONFIG.into()),
                    SPIRE_SERVER_CONFIG_PATH,
                ),
                CopyToContainer::new(
                    CopyDataSource::Data(SPIRE_SERVER_DUMMY_CA_CERT.into()),
                    SPIRE_SERVER_DUMMY_CA_CERT_PATH,
                ),
                CopyToContainer::new(
                    CopyDataSource::Data(SPIRE_SERVER_DUMMY_CA_KEY.into()),
                    SPIRE_SERVER_DUMMY_CA_KEY_PATH,
                ),
            ],
        }
    }
}

impl Image for SpireServer {
    fn name(&self) -> &str {
        "ghcr.io/spiffe/spire-server"
    }

    fn tag(&self) -> &str {
        "1.11.2"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![
            WaitFor::message_on_stdout("Starting Server APIs"),
            // WaitFor::message_on_stdout("Health check recovered"),
            WaitFor::seconds(3),
        ]
    }

    fn copy_to_sources(&self) -> impl IntoIterator<Item = &CopyToContainer> {
        &self.copy_to_sources
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &[ContainerPort::Tcp(8081)]
    }
}

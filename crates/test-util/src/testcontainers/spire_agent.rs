use std::{net::IpAddr, path::PathBuf};

use testcontainers::core::{ContainerPort, Mount, WaitFor};
use testcontainers::{CopyDataSource, CopyToContainer, Image};

const AGENT_SOCKET_DIR: &str = "/tmp/spire-agent/public";

const SPIRE_AGENT_CONFIG_PATH: &str = "/opt/spire/conf/agent/agent.conf";

// based on https://github.com/spiffe/spire/blob/v1.11.2/conf/agent/agent_container.conf
const SPIRE_AGENT_CONFIG: &str = r#"agent {
    data_dir = "/var/lib/spire/agent/.data"
    log_level = "DEBUG"
    server_address = "127.0.0.1"
    server_port = "8081"
    #socket_path = "/tmp/spire-agent/public/api.sock"
    socket_path ="tcp:///0.0.0.0:8082"
    trust_bundle_path = "/etc/spire/agent/dummy_root_ca.crt"
    trust_domain = "wasmcloud.dev"
}

plugins {
    NodeAttestor "join_token" {
        plugin_data {
        }
    }
    KeyManager "disk" {
        plugin_data {
            directory = "/var/lib/spire/agent/.data"
        }
    }
    WorkloadAttestor "unix" {
        plugin_data {
        }
    }
}
"#;

const SPIRE_AGENT_DUMMY_ROOT_CA_PATH: &str = "/etc/spire/agent/dummy_root_ca.crt";
// https://github.com/spiffe/spire/blob/v1.11.2/conf/agent/dummy_root_ca.crt
const SPIRE_AGENT_DUMMY_ROOT_CA: &str = r#"
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

#[derive(Debug, Clone)]
pub struct SpireAgent {
    _priv: (),
    copy_to_sources: Vec<CopyToContainer>,
    cmd: Vec<String>,
    mounts: Vec<Mount>,
}

impl Default for SpireAgent {
    fn default() -> Self {
        Self {
            _priv: (),
            copy_to_sources: vec![
                CopyToContainer::new(
                    CopyDataSource::Data(SPIRE_AGENT_DUMMY_ROOT_CA.into()),
                    SPIRE_AGENT_DUMMY_ROOT_CA_PATH,
                ),
                CopyToContainer::new(
                    CopyDataSource::Data(SPIRE_AGENT_CONFIG.into()),
                    SPIRE_AGENT_CONFIG_PATH,
                ),
            ],
            cmd: vec![],
            mounts: vec![],
        }
    }
}

impl SpireAgent {
    pub fn with_join_token(mut self, join_token: &str) -> Self {
        self.cmd.push("-joinToken".to_string());
        self.cmd.push(join_token.to_string());
        self
    }

    pub fn with_server_address(mut self, address: IpAddr) -> Self {
        self.cmd.push("-serverAddress".to_string());
        self.cmd.push(address.to_string());
        self
    }

    pub fn with_server_port(mut self, port: u16) -> Self {
        self.cmd.push("-serverPort".to_string());
        self.cmd.push(port.to_string());
        self
    }

    pub fn with_socket_path(mut self, socket_path: PathBuf) -> Self {
        self.mounts.push(Mount::bind_mount(
            socket_path.display().to_string(),
            AGENT_SOCKET_DIR,
        ));
        self.cmd.push("-socketPath".to_string());
        self.cmd
            .push(format!("{}/{}", AGENT_SOCKET_DIR, "api.sock"));
        self
    }
}

// could not find config file /opt/spire/conf/server/server.conf: please use the -config flag
impl Image for SpireAgent {
    fn name(&self) -> &str {
        "ghcr.io/spiffe/spire-agent"
    }

    fn tag(&self) -> &str {
        "1.11.2"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![
            WaitFor::message_on_stdout("Starting Workload and SDS APIs"),
            WaitFor::seconds(3),
        ]
    }

    fn copy_to_sources(&self) -> impl IntoIterator<Item = &CopyToContainer> {
        &self.copy_to_sources
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &[ContainerPort::Tcp(8082)]
    }

    fn cmd(&self) -> impl IntoIterator<Item = impl Into<std::borrow::Cow<'_, str>>> {
        &self.cmd
    }

    fn mounts(&self) -> impl IntoIterator<Item = &Mount> {
        &self.mounts
    }
}

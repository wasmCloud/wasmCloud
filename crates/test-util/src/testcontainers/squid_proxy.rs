use testcontainers::{
    core::{ContainerPort, WaitFor},
    Image,
};

#[derive(Default, Debug, Clone)]
pub struct SquidProxy {
    _priv: (),
}

impl Image for SquidProxy {
    fn name(&self) -> &str {
        "cgr.dev/chainguard/squid-proxy"
    }

    fn tag(&self) -> &str {
        "latest"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![
            WaitFor::message_on_stdout("listening port: 3128"),
            WaitFor::seconds(3),
        ]
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &[ContainerPort::Tcp(3128)]
    }
}

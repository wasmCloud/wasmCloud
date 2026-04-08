//! P3 IP name lookup host trait implementation.

use super::WasiSocketsCtxView;
use crate::sockets::WasiSockets;
use crate::sockets::util::{from_ipv4_addr, from_ipv6_addr, parse_host};

use wasmtime::component::Accessor;
use wasmtime_wasi::p3::bindings::sockets::ip_name_lookup::{Host, HostWithStore};
use wasmtime_wasi::p3::bindings::sockets::{ip_name_lookup::ErrorCode, types};

impl HostWithStore for WasiSockets {
    async fn resolve_addresses<U>(
        store: &Accessor<U, Self>,
        name: String,
    ) -> wasmtime::Result<Result<Vec<types::IpAddress>, ErrorCode>> {
        let allow_lookup =
            store.with(|mut view| view.get().ctx.allowed_network_uses.ip_name_lookup);
        if !allow_lookup {
            return Ok(Err(ErrorCode::PermanentResolverFailure));
        }

        let host = match parse_host(&name) {
            Ok(host) => host,
            Err(_) => return Ok(Err(ErrorCode::InvalidArgument)),
        };

        // Perform DNS resolution in a blocking task
        let result = tokio::task::spawn_blocking(move || blocking_resolve(&host))
            .await
            .map_err(|e| wasmtime::format_err!("DNS resolution task failed: {e}"))?;

        Ok(result)
    }
}

impl Host for WasiSocketsCtxView<'_> {}

fn blocking_resolve(host: &url::Host) -> Result<Vec<types::IpAddress>, ErrorCode> {
    use std::net::ToSocketAddrs;

    match host {
        url::Host::Ipv4(v4addr) => Ok(vec![types::IpAddress::Ipv4(from_ipv4_addr(*v4addr))]),
        url::Host::Ipv6(v6addr) => Ok(vec![types::IpAddress::Ipv6(from_ipv6_addr(*v6addr))]),
        url::Host::Domain(domain) => {
            let addresses = (domain.as_str(), 0)
                .to_socket_addrs()
                .map_err(|_| ErrorCode::NameUnresolvable)?
                .map(|addr| {
                    let ip = addr.ip().to_canonical();
                    match ip {
                        std::net::IpAddr::V4(v4) => types::IpAddress::Ipv4(from_ipv4_addr(v4)),
                        std::net::IpAddr::V6(v6) => types::IpAddress::Ipv6(from_ipv6_addr(v6)),
                    }
                })
                .collect();
            Ok(addresses)
        }
    }
}

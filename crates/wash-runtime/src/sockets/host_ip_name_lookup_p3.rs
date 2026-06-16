//! P3 IP name lookup host trait implementation.

use super::WasiSocketsCtxView;
use crate::sockets::WasiSockets;
use crate::sockets::util::{from_ipv4_addr, from_ipv6_addr, parse_host};

use wasmtime::component::Accessor;
use wasmtime_wasi::p3::bindings::sockets::ip_name_lookup::{Host, HostWithStore};
use wasmtime_wasi::p3::bindings::sockets::{ip_name_lookup::ErrorCode, types};

impl<U> HostWithStore<U> for WasiSockets {
    async fn resolve_addresses(
        store: &Accessor<U, Self>,
        name: String,
    ) -> wasmtime::Result<Result<Vec<types::IpAddress>, ErrorCode>> {
        // Mirror the ordering of the upstream wasmtime implementation: parse the
        // name before consulting the capability so a malformed name reports
        // `InvalidArgument` regardless of whether lookups are permitted.
        let Ok(host) = parse_host(&name) else {
            return Ok(Err(ErrorCode::InvalidArgument));
        };
        if !store.with(|mut view| view.get().ctx.allowed_network_uses.ip_name_lookup) {
            return Ok(Err(ErrorCode::PermanentResolverFailure));
        }
        Ok(resolve(host).await)
    }
}

impl Host for WasiSocketsCtxView<'_> {}

/// Resolve a host to a list of IP addresses.
///
/// Literal IPv4/IPv6 hosts are returned directly. Domains are resolved with
/// [`tokio::net::lookup_host`], which performs the blocking `getaddrinfo` call
/// on a dedicated blocking task internally, so we don't wrap it ourselves.
async fn resolve(host: url::Host) -> Result<Vec<types::IpAddress>, ErrorCode> {
    match host {
        url::Host::Ipv4(addr) => Ok(vec![types::IpAddress::Ipv4(from_ipv4_addr(addr))]),
        url::Host::Ipv6(addr) => Ok(vec![types::IpAddress::Ipv6(from_ipv6_addr(addr))]),
        url::Host::Domain(domain) => {
            // Only names are resolved here, not ports, so force the port to 0.
            let addrs = tokio::net::lookup_host((domain.as_str(), 0))
                .await
                .map_err(|_| ErrorCode::NameUnresolvable)?;
            // `IpAddr` -> `types::IpAddress` via the conversion wasmtime defines.
            Ok(addrs.map(|addr| addr.ip().to_canonical().into()).collect())
        }
    }
}

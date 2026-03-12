use super::WasiSocketsCtxView;
use super::network::SocketError;
use std::mem;
use std::net::ToSocketAddrs;
use std::pin::Pin;
use std::vec;
use wasmtime::Result;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::sockets::ip_name_lookup::{Host, HostResolveAddressStream};
use wasmtime_wasi::p2::bindings::sockets::network::{ErrorCode, IpAddress};
use wasmtime_wasi::runtime::{AbortOnDropJoinHandle, spawn_blocking};
use wasmtime_wasi_io::poll::{DynPollable, Pollable, subscribe};

use super::host_network::ip_addr_to_ip_address;
use super::util::{from_ipv4_addr, from_ipv6_addr, parse_host};

type UpstreamNetwork = wasmtime_wasi::p2::Network;
// The upstream ResolveAddressStream type is in a private module.
// We use the generated bindings type alias instead.
use wasmtime_wasi::p2::bindings::sockets::ip_name_lookup::ResolveAddressStream as UpstreamResolveAddressStream;

pub enum ResolveAddressStream {
    Waiting(AbortOnDropJoinHandle<Result<Vec<IpAddress>, SocketError>>),
    Done(Result<vec::IntoIter<IpAddress>, SocketError>),
}

impl Host for WasiSocketsCtxView<'_> {
    fn resolve_addresses(
        &mut self,
        network: Resource<UpstreamNetwork>,
        name: String,
    ) -> Result<Resource<UpstreamResolveAddressStream>, SocketError> {
        let network = Resource::<super::network::Network>::new_borrow(network.rep());
        let network = self.table.get(&network)?;

        let host = parse_host(&name).map_err(super::network::socket_error_from_util)?;

        if !network.allow_ip_name_lookup {
            return Err(ErrorCode::PermanentResolverFailure.into());
        }

        let task = spawn_blocking(move || blocking_resolve(&host));
        let resource = self.table.push(ResolveAddressStream::Waiting(task))?;
        Ok(Resource::new_own(resource.rep()))
    }
}

impl HostResolveAddressStream for WasiSocketsCtxView<'_> {
    fn resolve_next_address(
        &mut self,
        resource: Resource<UpstreamResolveAddressStream>,
    ) -> Result<Option<IpAddress>, SocketError> {
        let resource = Resource::<ResolveAddressStream>::new_borrow(resource.rep());
        let stream: &mut ResolveAddressStream = self.table.get_mut(&resource)?;
        loop {
            match stream {
                ResolveAddressStream::Waiting(future) => {
                    match wasmtime_wasi::runtime::poll_noop(Pin::new(future)) {
                        Some(result) => {
                            *stream = ResolveAddressStream::Done(result.map(|v| v.into_iter()));
                        }
                        None => return Err(ErrorCode::WouldBlock.into()),
                    }
                }
                ResolveAddressStream::Done(slot @ Err(_)) => {
                    mem::replace(slot, Ok(Vec::new().into_iter()))?;
                    unreachable!();
                }
                ResolveAddressStream::Done(Ok(iter)) => return Ok(iter.next()),
            }
        }
    }

    fn subscribe(
        &mut self,
        resource: Resource<UpstreamResolveAddressStream>,
    ) -> Result<Resource<DynPollable>> {
        let resource = Resource::<ResolveAddressStream>::new_borrow(resource.rep());
        subscribe(self.table, resource)
    }

    fn drop(&mut self, resource: Resource<UpstreamResolveAddressStream>) -> Result<()> {
        let resource = Resource::<ResolveAddressStream>::new_own(resource.rep());
        self.table.delete(resource)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Pollable for ResolveAddressStream {
    async fn ready(&mut self) {
        if let ResolveAddressStream::Waiting(future) = self {
            *self = ResolveAddressStream::Done(future.await.map(|v| v.into_iter()));
        }
    }
}

fn blocking_resolve(host: &url::Host) -> Result<Vec<IpAddress>, SocketError> {
    match host {
        url::Host::Ipv4(v4addr) => Ok(vec![IpAddress::Ipv4(from_ipv4_addr(*v4addr))]),
        url::Host::Ipv6(v6addr) => Ok(vec![IpAddress::Ipv6(from_ipv6_addr(*v6addr))]),
        url::Host::Domain(domain) => {
            // For now use the standard library to perform actual resolution through
            // the usage of the `ToSocketAddrs` trait. This is only
            // resolving names, not ports, so force the port to be 0.
            let addresses = (domain.as_str(), 0)
                .to_socket_addrs()
                .map_err(|_| ErrorCode::NameUnresolvable)? // If/when we use `getaddrinfo` directly, map the error properly.
                .map(|addr| ip_addr_to_ip_address(addr.ip().to_canonical()))
                .collect();

            Ok(addresses)
        }
    }
}

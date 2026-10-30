use super::WasiSocketsCtxView;
use super::network::SocketError;
use std::mem;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::vec;
use wasmtime::Result;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::sockets::ip_name_lookup::{Host, HostResolveAddressStream};
use wasmtime_wasi::p2::bindings::sockets::network::{ErrorCode, IpAddress};
use wasmtime_wasi::runtime::{AbortOnDropJoinHandle, spawn_blocking};
use wasmtime_wasi_io::poll::{DynPollable, Pollable, subscribe};

use super::host_network::ip_addr_to_ip_address;
use super::resolved_names::{Lookup, Observation, ResolvedNames, lookup_blocking};
use super::util::parse_host;

type UpstreamNetwork = wasmtime_wasi::p2::Network;
// The upstream ResolveAddressStream type is in a private module.
// We use the generated bindings type alias instead.
use wasmtime_wasi::p2::bindings::sockets::ip_name_lookup::ResolveAddressStream as UpstreamResolveAddressStream;

pub struct ResolveAddressStream {
    /// Where each address is recorded as it is handed to the guest.
    names: Arc<ResolvedNames>,
    state: State,
}

enum State {
    Waiting(AbortOnDropJoinHandle<Result<Lookup, SocketError>>),
    Done(Result<Answer, SocketError>),
}

struct Answer {
    addresses: vec::IntoIter<IpAddr>,
    observation: Option<Observation>,
}

impl From<Lookup> for Answer {
    fn from(lookup: Lookup) -> Self {
        Self {
            addresses: lookup.addresses.into_iter(),
            observation: lookup.observation,
        }
    }
}

impl Host for WasiSocketsCtxView<'_> {
    fn resolve_addresses(
        &mut self,
        network: Resource<UpstreamNetwork>,
        name: String,
    ) -> Result<Resource<UpstreamResolveAddressStream>, SocketError> {
        let network = Resource::<super::network::Network>::new_borrow(network.rep());
        let network = self.table.get(&network)?;
        let names = Arc::clone(&network.resolved_names);

        // The reserved zone answers before `allowedIpNameLookups` and before any
        // resolver. That allowlist exists because resolution reaches the network
        // ahead of any connection, so a guest can encode data in the labels it
        // looks up; these names never leave the process. Reaching the host is
        // still gated, at connect. See [`super::internal_names`].
        if let Some(internal) = super::internal_names::resolve(&name) {
            let addr = internal
                .map_err(|_| SocketError::from(ErrorCode::NameUnresolvable))?
                .address();
            let resource = self.table.push(ResolveAddressStream {
                names,
                state: State::Done(Ok(Lookup::fixed(vec![addr]).into())),
            })?;
            return Ok(Resource::new_own(resource.rep()));
        }

        let host = parse_host(&name).map_err(super::network::socket_error_from_util)?;

        if !crate::host::allowed_ip_name::check_allowed_lookup(
            &network.allowed_ip_name_lookups,
            &network.allowed_hosts,
            &host,
        ) {
            tracing::warn!(
                "{}",
                crate::host::allowed_ip_name::lookup_denial(&network.allowed_hosts, &host)
            );
            return Err(ErrorCode::PermanentResolverFailure.into());
        }

        // If/when `getaddrinfo` is called directly, map its error properly.
        let task = spawn_blocking(move || {
            lookup_blocking(&host).map_err(|_| SocketError::from(ErrorCode::NameUnresolvable))
        });
        let resource = self.table.push(ResolveAddressStream {
            names,
            state: State::Waiting(task),
        })?;
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
            match &mut stream.state {
                State::Waiting(future) => {
                    match wasmtime_wasi::runtime::poll_noop(Pin::new(future)) {
                        Some(result) => {
                            stream.state = State::Done(result.map(Answer::from));
                        }
                        None => return Err(ErrorCode::WouldBlock.into()),
                    }
                }
                State::Done(slot @ Err(_)) => {
                    mem::replace(slot, Ok(Lookup::fixed(Vec::new()).into()))?;
                    unreachable!();
                }
                // Recorded as each address is handed over, so a stream that is
                // only polled, or dropped unread, grants nothing.
                State::Done(Ok(answer)) => {
                    let next = answer.addresses.next();
                    if let (Some(address), Some(observation)) = (next, &answer.observation) {
                        stream
                            .names
                            .record(&observation.name, [address], observation.observed);
                    }
                    return Ok(next.map(ip_addr_to_ip_address));
                }
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
        if let State::Waiting(future) = &mut self.state {
            self.state = State::Done(future.await.map(Answer::from));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::allowed_hosts::AllowedHost;
    use crate::sockets::WasiSocketsCtx;
    use std::time::Instant;
    use wasmtime::component::ResourceTable;

    fn answered(names: &Arc<ResolvedNames>, addresses: &[[u8; 4]]) -> ResolveAddressStream {
        ResolveAddressStream {
            names: Arc::clone(names),
            state: State::Done(Ok(Lookup {
                addresses: addresses
                    .iter()
                    .map(|octets| IpAddr::from(*octets))
                    .collect(),
                observation: Some(Observation {
                    name: "db.internal".to_string(),
                    observed: Instant::now(),
                }),
            }
            .into())),
        }
    }

    fn permitted(names: &ResolvedNames, addr: &str) -> bool {
        let policy: [AllowedHost; 1] = ["db.internal".parse().unwrap()];
        names.permits(&policy, addr.parse().unwrap())
    }

    /// An address is granted when the guest is handed it, one at a time: an
    /// answer that is ready but unread grants nothing, and neither does one
    /// dropped part-way.
    #[tokio::test]
    async fn only_addresses_handed_to_the_guest_are_recorded() {
        let mut ctx = WasiSocketsCtx::default();
        let mut table = ResourceTable::new();
        let names = Arc::clone(&ctx.resolved_names);
        let mut stream = answered(&names, &[[10, 0, 0, 1], [10, 0, 0, 2]]);
        stream.ready().await;
        let resource = table.push(stream).unwrap();
        let rep = resource.rep();
        let mut view = WasiSocketsCtxView {
            ctx: &mut ctx,
            table: &mut table,
        };
        assert!(!permitted(&names, "10.0.0.1:5432"));

        let first = view
            .resolve_next_address(Resource::new_borrow(rep))
            .unwrap();
        assert!(matches!(first, Some(IpAddress::Ipv4((10, 0, 0, 1)))));
        assert!(permitted(&names, "10.0.0.1:5432"));
        assert!(!permitted(&names, "10.0.0.2:5432"));

        HostResolveAddressStream::drop(&mut view, Resource::new_own(rep)).unwrap();
        assert!(!permitted(&names, "10.0.0.2:5432"));
    }
}

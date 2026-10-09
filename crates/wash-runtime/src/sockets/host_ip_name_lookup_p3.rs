//! P3 IP name lookup host trait implementation.

use super::WasiSocketsCtxView;
use crate::sockets::WasiSockets;
use crate::sockets::resolved_names::lookup_blocking;
use crate::sockets::util::parse_host;
use std::sync::Arc;

use wasmtime::component::Accessor;
use wasmtime_wasi::p3::bindings::sockets::ip_name_lookup::{Host, HostWithStore};
use wasmtime_wasi::p3::bindings::sockets::{ip_name_lookup::ErrorCode, types};

impl<U> HostWithStore<U> for WasiSockets {
    async fn resolve_addresses(
        store: &Accessor<U, Self>,
        name: String,
    ) -> wasmtime::Result<Result<Vec<types::IpAddress>, ErrorCode>> {
        // The reserved zone answers before `allowedIpNameLookups` and before any
        // resolver — see [`crate::sockets::internal_names`]. Ahead of the parse,
        // too: these are host-internal names, not names a resolver would ever
        // see, so `parse_host`'s opinion of them is irrelevant.
        if let Some(internal) = crate::sockets::internal_names::resolve(&name) {
            return Ok(match internal {
                Ok(internal) => Ok(vec![internal.address().into()]),
                Err(_) => Err(ErrorCode::NameUnresolvable),
            });
        }

        // Mirror the ordering of the upstream wasmtime implementation: parse the
        // name before consulting the capability so a malformed name reports
        // `InvalidArgument` regardless of whether lookups are permitted.
        let Ok(host) = parse_host(&name) else {
            return Ok(Err(ErrorCode::InvalidArgument));
        };
        let (allowed, names) = store.with(|mut view| {
            let ctx = &view.get().ctx;
            let allowed = crate::host::allowed_ip_name::check_allowed_lookup(
                &ctx.allowed_ip_name_lookups,
                &ctx.allowed_hosts,
                &host,
            );
            if !allowed {
                tracing::warn!(
                    "{}",
                    crate::host::allowed_ip_name::lookup_denial(&ctx.allowed_hosts, &host)
                );
            }
            (allowed, Arc::clone(&ctx.resolved_names))
        });
        if !allowed {
            return Ok(Err(ErrorCode::PermanentResolverFailure));
        }

        // The same lookup p2 runs, on the blocking pool.
        let lookup = tokio::task::spawn_blocking(move || lookup_blocking(&host)).await;
        let Ok(Ok(lookup)) = lookup else {
            return Ok(Err(ErrorCode::NameUnresolvable));
        };
        // Recorded as the batch is returned, dated when resolution completed.
        if let Some(observation) = &lookup.observation {
            names.record(
                &observation.name,
                lookup.addresses.iter().copied(),
                observation.observed,
            );
        }
        Ok(Ok(lookup
            .addresses
            .into_iter()
            .map(types::IpAddress::from)
            .collect()))
    }
}

impl Host for WasiSocketsCtxView<'_> {}

[![crates.io](https://img.shields.io/crates/v/wasmcloud-nats-kvcache.svg)](https://crates.io/crates/wasmcloud-nats-kvcache)
![Rust](https://github.com/wasmcloud/capability-providers/workflows/NATS-KVCACHE/badge.svg)
![license](https://img.shields.io/crates/l/wasmcloud-nats-kvcache.svg)
[![documentation](https://docs.rs/wasmcloud-nats-kvcache/badge.svg)](https://docs.rs/wasmcloud-nats-kvcache)

# wasmCloud Key-Value Provider (NATS Distributed Key-Value Cache)

This is an _in-memory_ key-value cache that is distributed by replicating state changes across a specific set of
NATS topics. It is understood that in the "bare NATS" version of this provider, message loss can potentially happen
and thus the cache values can potentially diverge from host to host.

## CAUTION ⚠️

It is recommended that this key-value store be used in development, debugging, and R&D purposes only and you
not use this for a production distributed cache unless periodic node divergence is an acceptable use case (for example, you re-query
data upon cache misses or time-delayed/heartbeat reconciliation is acceptable). This provider will attempt to use high water marks + event sourcing to reconcile global state, but that system is not infallible.

As per the `wasmcloud:keyvalue` contract, all values (except for atomics) stored by this provider are treated as strings. `Sets` are treated as sets of strings, etc. Atomic values _cannot guarantee_ global atomicity. Increments and decrements will be done relative to the currently known (local) value at the time, which means concurrent, distributed so-called "atomic" updates can result in inconsistent values.

Also note that caches are _not isolated per actor_. This is not a multi-tenant cache. If two different actors bound to the same named link instance
of this provider request the same key, they will get the same value. If you need to segment distributed caches, do so by providing different
link names during start-up.

This provider will take the configuration received from the _first linked actor_ and use that data to connect to NATS. All subsequent attempts
to provide link configuration will be ignored in order to satisfy the "idempotent provider" requirement. If no NATS connection string information is provided during the initial link configuration, then this provider will effectively perform like a single, standalone in-memory cache and not replicate or subscribe to any state changes.

## ANOTHER WARNING ⚠️

Do not rely on this provider to return meaningful data in response to mutation operations. For example, the atomic add operation, by contract, returns
an integer that should represent the new value. For optimization purposes, this provider will return the default (empty) version of all responses. This
is based on the notion that when you are setting data in this distributed cache, you are not interested in immediately retrieving the value.

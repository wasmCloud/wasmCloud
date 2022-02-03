# wasmbus-rpc Changelog

## 0.7.0

### Breaking changes (since 0.6.x)

- Some of the crate exported symbols have moved to sub-modules. The intent is to resolve some linking problems
  resulting from multiple inconsistent references to these symbols.
  Most of these changes will require only a recompile, for Actors and Providers 
  that import `wasmbus_rpc::actor::prelude::*` or `wasmbus_rpc::provider::prelude::*`, respectively.
  - wasmbus_rpc::{RpcError,RpcResult} -> wasmbus_rpc::error::{RpcError,RpcResult}
  - wasmbus_rpc::{Message,MessageDispatch,Transport} -> wasmbus_rpc::common::{Message,MessageDispatch,Transport}
  - wasmbus_rpc::context::Context -> wasmbus_rpc::common::Context
  - To help avoid external breakage, the crate-level symbols have been marked deprecated
  
- removed feature options [ser_json] and [ser_msgpack] - ser_msgpack was always, and remains, the default.
- added a `cbor` module to wrap `minicor`, so the choice of cbor implementation is not exposed.
- Depends on codegen-0.3.0


## 0.7.0-alpha.1

### Features

- replaced `ratsio` with `nats-aflowt`
  - `nats-aflowt` is reexported as `wasmbus_rpc::anats`
- removed dependency on nats-io/nats.rs (official nats client crate)
- `RpcClient::request` obeys the `timeout` parameter in `RpcClient::new(..)`

### Breaking changes (since 0.6.x)

- removed support for rpc_client types `Sync` and `Asynk`. Only `Async` now.
- `provider::NatsClient` changed type and is `anats::Connection`
- type `Subscription` is no longer exported (now: anats::Subscription)
- `HostBridge::new` - nats parameter no longer enclosed in Arc<>
- `get_async` returns `Option<anats::Connection>` instead of `Option<Arc<NatsClient>>`

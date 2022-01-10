# wasmbus-rpc Changelog

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

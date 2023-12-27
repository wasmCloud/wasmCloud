# Lattice Controller Capability Provider

A capability provider that allows actors to interact with the lattice control interface (`wasmcloud:latticecontrol`) by
remotely communicating with lattices and the hosts contained within them via NATS.

## Configuration
This capability provider is designed to facilitate connections to multiple lattices. For each connection that actors need to utilize, there must be an accompanying `set_lattice_credentials` call, _even if you are establishing a connection to the `default` lattice_.

It may not be immediately obvious, but because of the flexible design of wasmCloud's lattice, the actor(s) that set credentials need not be the same actor(s) that utilize the established connections so long as they are all bound to the provider via empty link definitions.

Further, you can run multiple instances of this provider in a source lattice (e.g. not necessarily one you're remotely managing) and it will automatically scale, with each instance maintaining its own cached connection to the appropriate lattices.

## ⚠️ Compatibility Warning for versions < 0.9.0 ⚠️
In previous versions of this capability provider, the provider would only ever establish one lattice control connection per instance, typically associated with the link name. The configuration for the lattice connection would come from the data on the link definition.

This is _not_ how the current version of the provider works. The current provider is a _multiplexed_ provider supporting multiple lattices. An actor needs to establish a link once via an empty link definition, and then establish connections to remote lattices by using the `set_lattice_credentials` operation on the provider.

This provider no longer supports fallback connections supplied via the provider configuration parameter at startup. In other words, you _must_ invoke `set_lattice_credentials` at least once to use this provider.


## Actor Usage Example
The following is an example of what it looks like for an actor to utilize this capability provider. In this sample, the actor is requesting that the `echo` actor (10 instances of it) be started in the `default` lattice:

```rust
async fn start_actor(ctx: &Context) -> RpcResult<CtlOperationAck> {
    let lattice = LatticeControllerSender::new();
    // Instruct provider to use anonymous local for NATS client for `default` lattice
    let _ = lattice.set_lattice_credentials(SetLatticeCredentialsRequest {
        lattice_id: "default".to_string(),
        nats_url: None,
        user_jwt: None,
        user_seed: None
    }).await;

    let cmd = StartActorCommand {
        lattice_id: "default".to_string(),
        actor_ref: "wasmcloud.azurecr.io/echo:0.3.4".to_string(),
        annotations: None,
        count: 10,
        host_id: "NB67YNOVU5YB3526RUNCKNZBCQDH2L5NZJKQ6FWOVWGSHNHHEO65RP4A".to_string(),
    };

    debug!("Starting 10 instances of the echo actor...");

    lattice.start_actor(ctx, &cmd).await
}
```

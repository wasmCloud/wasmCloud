[![crates.io](https://img.shields.io/crates/v/wascc-nats.svg)](https://crates.io/crates/wascc-nats)&nbsp;
![Rust](https://github.com/wascc/nats-provider/workflows/Rust/badge.svg)
![license](https://img.shields.io/crates/l/wascc-nats.svg)&nbsp;
[![documentation](https://docs.rs/wascc-nats/badge.svg)](https://docs.rs/wascc-nats)

# waSCC Messaging Provider (NATS)

The waSCC NATS capability provider exposes publish and subscribe functionality to actors. The following configuration values can be passed to the waSCC host runtime for each actor binding:

* `SUBSCRIPTION` - The subscription string. Each guest module can create _one_ subscription. This string can contain wildcard characters.
* `QUEUEGROUP_NAME` - If you want all instances of the same actor to share round-robin delivery of messages, then set a unique queue group name for them.
* `URL` - The URL to initially connect with a server. Should use the `nats://` scheme prefix.
* `CLIENT_JWT` - If not using anonymous authentication, this is the _signed user JWT_ used for client authentication against the NATS 2.x+ server.
* `CLIENT_SEED` - If you have supplied a value for the client JWT, the seed is required for authentication. This should be the nats-style "nkeys" encoded string for the seed and NOT a raw binary value.

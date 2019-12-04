# NATS Provider

The Waxosuit NATS capability provider exposes publish and subscribe functionality to Waxosuit guest modules. The following environment variables are used by this provider for configuration:

* `NATS_SUBSCRIPTION` - The subscription string. Each guest module can create _one_ subscription. This string can contain wildcard characters.
* `NATS_QUEUEGROUP_NAME` - If you want all instances of the same module to share round-robin delivery of messages, then set a unique queue group name for them.
* `NATS_URL` - The URL to initially connect with a server. Should use the `nats://` scheme prefix.
* `NATS_CLIENT_JWT` - If not using anonymous authentication, this is the _signed user JWT_ used for client authentication against the NATS 2.x+ server.
* `NATS_CLIENT_SEED` - If you have supplied a value for the client JWT, the seed is required for authentication. This should be the nats-style "nkeys" encoded string for the seed and NOT a raw binary value.

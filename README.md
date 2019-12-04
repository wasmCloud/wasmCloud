![travis](https://travis-ci.org/wascc/nats-provider.svg?branch=master)&nbsp;


# waSCC Messaging Provider (NATS)

The waSCC NATS capability provider exposes publish and subscribe functionality to actors. The following configuration values can be passed to the waSCC host runtime to configure the NATS connection on a per-actor basis:

* `SUBSCRIPTION` - The subscription string. Each guest module can create _one_ subscription. This string can contain wildcard characters.
* `QUEUEGROUP_NAME` - If you want all instances of the same actor to share round-robin delivery of messages, then set a unique queue group name for them.
* `URL` - The URL to initially connect with a server. Should use the `nats://` scheme prefix.
* `CLIENT_JWT` - If not using anonymous authentication, this is the _signed user JWT_ used for client authentication against the NATS 2.x+ server.
* `CLIENT_SEED` - If you have supplied a value for the client JWT, the seed is required for authentication. This should be the nats-style "nkeys" encoded string for the seed and NOT a raw binary value.

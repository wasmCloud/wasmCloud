# waSCC Capability Providers

This repository includes the waSCC first party and sample capability providers.

<<<<<<< HEAD
1. fs
1. http-client
1. http-server
1. logging
1. nats
1. redis
1. redis-streams
1. redisgraph
1. s3
1. telnet
=======
The waSCC NATS capability provider exposes publish and subscribe functionality to actors. The following configuration values can be passed to the waSCC host runtime for each actor binding:

* `SUBSCRIPTION` - The subscription string. This can contain wildcards. Use a comma-separated list for multiple subscriptions.
* `QUEUEGROUP_NAME` - If you want all instances of the same actor to share round-robin delivery of messages, then set a unique queue group name for them. This queue group name will apply to all configured subscriptions.
* `URL` - The URL to initially connect with a server. Should use the `nats://` scheme prefix.
* `CREDSFILE` - The path to a `.creds` file (which can be generated via NATS client(s)) containing the client JWT and the seed used to sign the nonce. If this value is not included in the binding, the provider will use _anonymous_ authentication.
* `CLIENT_JWT` - Supported prior to 0.9, not _currently_ supported.
* `CLIENT_SEED` - Supported prior to 0.9, not _currently_ supported.
>>>>>>> nats/upgrade

![travis](https://travis-ci.org/wascc/redis-provider.svg?branch=master)&nbsp;


# waSCC Key-Value Provider (Redis)

The waSCC Redis capability provider exposes an implementation of the key-value store interface built using Redis. Each actor module within a host runtime will be given its own unique Redis client connection. The following configuration parameters are accepted:

* `URL` - The connection string URL. This will default to `redis://0.0.0.0:6379` if a configuration is supplied without this value.

**NOTE** As with all native capability providers, they will not activate or provision resources on behalf of an actor module until the `host::configure_actor()` function is called for an actor on this capability ID (`wascc:keyvalue`).
[![crates.io](https://img.shields.io/crates/v/wascc-redis-provider.svg)](https://crates.io/crates/wascc-redis-provider)&nbsp;
![Rust](https://github.com/wascc/redis-provider/workflows/Rust/badge.svg)&nbsp;
![license](https://img.shields.io/crates/l/wascc-redis-provider.svg)&nbsp;
[![documentation](https://docs.rs/wascc-host/badge.svg)](https://docs.rs/wascc-redis-provider)

# waSCC Key-Value Provider (Redis)

The waSCC Redis capability provider exposes an implementation of the key-value store interface built using Redis. Each actor module within a host runtime will be given its own unique Redis client connection. The following configuration parameters are accepted:

* `URL` - The connection string URL. This will default to `redis://0.0.0.0:6379` if a configuration is supplied without this value.

If you want to statically link (embed) this plugin in a custom waSCC host rather than use it as a dynamic plugin, then enable the `static_plugin` feature in your dependencies section as shown:

```
redis-provider = { version = "??", features = ["static_plugin"] }
```

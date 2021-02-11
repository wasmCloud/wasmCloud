[![crates.io](https://img.shields.io/crates/v/wasmcloud-redis.svg)](https://crates.io/crates/wasmcloud-redis)
![Rust](https://github.com/wasmcloud/capability-providers/workflows/REDIS/badge.svg)
![license](https://img.shields.io/crates/l/wasmcloud-redis.svg)
[![documentation](https://docs.rs/wasmcloud-redis/badge.svg)](https://docs.rs/wasmcloud-redis)

# wasmCloud Key-Value Provider (Redis)

The wasmCloud Redis capability provider exposes an implementation of the key-value store interface built using Redis. Each actor module within a host runtime will be given its own unique Redis client connection. The following configuration parameters are accepted:

* `URL` - The connection string URL. This will default to `redis://0.0.0.0:6379` if a configuration is supplied without this value.

If you want to statically link (embed) this plugin in a custom wasmCloud host rather than use it as a dynamic plugin, then enable the `static_plugin` feature in your dependencies section as shown:

```
wasmcloud-redis = { version = "0.9.0", features = ["static_plugin"] }
```

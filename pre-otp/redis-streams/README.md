[![crates.io](https://img.shields.io/crates/v/wasmcloud-streams-redis.svg)](https://crates.io/crates/wasmcloud-streams-redis)
![Rust](https://github.com/wasmcloud/capability-providers/workflows/REDISSTREAMS/badge.svg)
![license](https://img.shields.io/crates/l/wasmcloud-streams-redis.svg)
[![documentation](https://docs.rs/wasmcloud-streams-redis/badge.svg)](https://docs.rs/wasmcloud-streams-redis)

# Redis Streams

This is a **wasmCloud** capability provider for `wasmscloud:eventstreams`, an abstraction around the concept of an append-only event stream service. This provider relies on _Redis Streams_ to support this functionality. At the moment, the API between actor and provider is very limited and we would like to grow that out so pull requests and discussion is greatly encouraged.

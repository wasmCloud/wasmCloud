[![crates.io](https://img.shields.io/crates/v/wascc-streams-redis.svg)](https://crates.io/crates/wascc-streams-redis)&nbsp;
![Rust](https://github.com/wascc/redis-streams/workflows/Rust/badge.svg)&nbsp;
![license](https://img.shields.io/crates/l/wascc-streams-redis.svg)&nbsp;
[![documentation](https://docs.rs/wascc-streams-redis/badge.svg)](https://docs.rs/wascc-streams-redis)

# Redis Streams

This is a **waSCC** capability provider for `wascc:eventstreams`, an abstraction around the concept of an append-only event stream service. This provider relies on _Redis Streams_ to support this functionality. At the moment, the API between actor and provider is very limited and we would like to grow that out so pull requests and discussion is greatly encouraged.

# New Actor Template

Use cargo generate to create a new actor module from this template.

[Learn how to write](https://wascc.dev/tutorials/first-actor/) waSCC actors, or [explore the actor concept](https://wascc.dev/docs/concepts/actors/).

To generate new keys, use `make keys`. This will use `nk` to generate both sets of keys for you, and then write them to the `.keys` directory.

To build your new module, use `make build`. This will compile your code with `cargo`, and then sign it with `wascap` using the keys in `.keys`.

## Tool Requirements

- Cargo and Rust are required
- Make is recommended, but not strictly necessary
- [wascap](https://github.com/wascc/wascap) is required for signing actor modules
- [nk](https://github.com/encabulators/nkeys) is required if you need to generate keys (which you almost certainly do)


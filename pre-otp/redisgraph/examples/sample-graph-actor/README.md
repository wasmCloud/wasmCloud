# New Actor Template

Use cargo generate to create a new actor module from this template:

```
cargo generate --git https://github.com/wasmcloud/new-actor-template --branch main
```

Use the `wash` CLI to sign your WebAssembly module after you have created it. The `Makefile` created by this template will sign your module for you with `make build` and `make release`.

You should modify this `Makefile` after creation to have the right actor name, revision, tags, and claims.

## Tool Requirements

- Cargo and Rust are required
- Make is recommended, but not strictly necessary
- [wash](https://github.com/wasmcloud/wash) - wasmcloud's multi-purpose CLI

### Note

The `Makefile` will use keys that are generated _locally_ for you. Once you move this actor into a pipeline toward a production deployment, you will want to explicitly specify the key locations and disable key generation in `wash` with the `--disable-keygen` option.

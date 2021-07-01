# wasmcloud:httpserver

This folder contains 
- the model definition for wasmcloud:httpserver 
- generated documentation (in html)
- generated rust library (in rust)

For a crate to be published on crates.io, both codegen.toml and the
`.smithy` model file need to be in the published source tree,
i.e., in the `rust` source directory, so that it can be built correctly.


Any rust actor or capability provider using wasmcloud:httpserver imports this library. A capability provider implements the trait HttpServerReceiver.



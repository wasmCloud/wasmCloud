# Weld - using smithy models with wasmCloud

This repository contains

- [codegen](https://github.com/wasmCloud/weld/blob/main/codegen/README.md) code generators to turn smithy models into target language libraries. Currently supported target languages: Rust and Html (for documentation). We plan to implement more targets in the future.
- [macros](https://github.com/wasmCloud/weld/blob/main/macros/README.md) derive macros for wasmCloud Rust projects. These are published as [wasmbus-macros](https://docs.rs/wasmbus-macros/), but they are not usually imported directly, but through wasmbus-rpc.
- [wasmbus-rpc](https://docs.rs/wasmbus-rpc) the Rust library for wasmCloud actors and capability providers.
 
You can find wasmcloud-related interfaces defined with smithy IDL in [interfaces](https://github.com/wasmcloud/interfaces/) and [examples](https://github.com/wasmCloud/examples/tree/main/interface/).

## Smithy References and tools

- [Smithy home page](https://awslabs.github.io/smithy/index.html)
- [IDL spec v1.0](https://awslabs.github.io/smithy/1.0/spec/core/idl.html)
- [Visual Studio plugin](https://github.com/awslabs/smithy-vscode) (in the extension marketplace)
- [Rust-atelier](https://github.com/johnstonskj/rust-atelier) rust smithy sdk that weld tools are built on
  
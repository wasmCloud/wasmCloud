## Wasmbus-core models

This folder contains core models 
- wasmcloud-model.smithy (namespace org.wasmcloud.model)
- wasmcloud-core.smithy (namespace org.wasmcloud.core)

To generate documentation,
    `make doc`

To regenerate the rust libraries based on these files,
run `make` from this folder or from the top-level folder in this repo.

A little more background:

The `.smithy` models here are used to generate these files
 - codegen/src/wasmbus_model.rs
 - rpc-rs/src/wasmbus_model.rs
 - rpc-rs/src/wasmbus_core.rs

The files in rpc-rs are updated as-needed by the regular cargo build process.
The file `build.rs` in that crate uses the codegen library to regenerate the 
wasmbus_*.rs files from the smithy models,
so they are not mentioned in the `codegen.toml` file.

The file `codegen/src/wasmcloud_model.rs` is identical to the one in rpc-rs, and is updated
either by running `make` in this folder or `make update-model` in the top project folder.
That build also depends on the codegen crate, but to avoid
a circular dependency, codegen cannot explicitly depend on this folder.
(That's also why we can't have an core model library that is shared by both codegen and rpc-rs).
To resolve the circular dependency, the build runs in two passes.
This is only applicable if wasmcloud-model.smithy changes!

After an edit to wasmcloud-model.smithy, the first build pass requires running either
`make` in this folder, or `make update-model` in the top-level folder. This requires an existing
weld bindary (which incorporates the codegen library) to regenerate codegen/src/wasmbus_model.rs,
On the second pass, codegen is rebuilt, and then codegen can be used
to generate code for all the other interface libraries.

If you just did a checkout from git, only one pass is required, because all the .rs files
should already be in sync with the .smithy model files.
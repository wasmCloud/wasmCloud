# Publishing on crates.io

If you want to publish an interface crate on crates.io, all the files necessary to generate the code for that library need to be in the published folder. You'll have to move a few files from their default locations.

If you are primarily working in rust, the easiest thing to do is:

- Put codegen.toml and the *.model files into the same folder as Cargo.toml, usually `./rust`. In `build.rs`, edit the path to codegen.toml to change it from `../codegen.toml` to `./codegen.toml`.


There are other ways to accomplish the same effect:

- Instead of moving codegen.toml and *.smithy down to the folder containing Cargo.toml, you can move Cargo.toml up. Edit Cargo.toml to set two non-default paths: under `[lib]`, add `path="rust/src/lib.rs"`, and under `[package]` add `build="rust/build.rs"`. Then publish from Cargo's new folder.
  
- It is possible for `codegen.toml` to reference model files from a public url, and replace the `path` setting to a local directory with a `url` setting to a public-facing server. If you make this change, those .smithy files don't need to be in the crate's source tree.  See [`rpc-rs/codegen.toml`](../rpc-rs/codegen.toml) for an example.
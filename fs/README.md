[![crates.io](https://img.shields.io/crates/v/wasmcloud-fs.svg)](https://crates.io/crates/wasmcloud-fs)&nbsp;
![Rust](https://github.com/wasmcloud/capability-providers/workflows/FS/badge.svg)
![license](https://img.shields.io/crates/l/wasmcloud-fs.svg)&nbsp;
[![documentation](https://docs.rs/wasmcloud-fs/badge.svg)](https://docs.rs/wasmcloud-fs)

# wasmCloud File System Provider

The **wasmCloud** File System provider is a capability provider for the `wasmcloud:blobstore` protocol. This generic protocol can be used to support capability providers like Amazon S3, Azure blob storage, Google blob storage, and more. This crate is an implementation of the protocol that operates on top of a designated root directory and can be used interchangeably with the larger cloud blob providers.

For this provider, the concept of a `container` is a directory beneath the root (specified via the `ROOT` configuration variable), while a `blob` corresponds to a file stored within one of the containers.

Because of the way WebAssembly and the wasmCloud host work, all `wasmcloud:blobstore` capability providers must _stream_ files to and from the actor. This allows actors to unblock long enough to allow other messages from other providers to be processed and keeps the WebAssembly module from allocating too much memory.

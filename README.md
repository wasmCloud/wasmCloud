[![crates.io](https://img.shields.io/crates/v/wascc-fs.svg)](https://crates.io/crates/wascc-fs)&nbsp;
![Rust](https://github.com/wascc/fs-provider/workflows/Rust/badge.svg)
![license](https://img.shields.io/crates/l/wascc-fs.svg)&nbsp;
[![documentation](https://docs.rs/wascc-fs/badge.svg)](https://docs.rs/wascc-fs)

# File System Provider

The **waSCC** File System provider is a capability provider for the `wascap:blobstore` protocol. This generic protocol allows for capability providers like Amazon S3, Azure blob storage, Google blob storage, and more. This is an implementation of this protocol that operates on top of a designated root directory.

For this provider, the concept of a `container` is a directory beneath the root (specified via the `ROOT` configuration variable), while a `blob` corresponds to a file stored within one of the containers.

Because of the way WebAssembly and the waSCC host work, all `wascap:blobstore` capability providers must _stream_ files to and from the actor. This allows actors to unblock long enough to allow other messages from other providers to be processed and keeps the WebAssembly module from allocating too much memory.

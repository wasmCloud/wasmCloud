# Provider Archive
Until the [WASI](https://wasi.dev) specification includes robust networking support _and_ the available WebAssembly tooling (**wasm3** , **wasmtime**, etc) supports this WASI specification, _and_ the Rust compiler is able to generate the right set of WASI imports when compiling "regular" socket code ... our support for portable capability providers will be limited.

In the absence of useful portable capability providers, we need the ability to store, retrieve, and schedule _native_ capability providers. A native capability provider is an FFI plugin stored in a binary file that is specific to a particular CPU architecture and Operating System. The issue with these binary files (_shared object_ files on linux) is that we cannot embed secure claims JWTs in these like we can in WebAssembly files. With actors, we use these signed tokens to get a verifiable, globally unique public key (identity) as well as a hash of the associated file to verify that the file has not been tampered with since being signed.

To give us the ability to store, retrieve, schedule, and _sign_ capability providers, we need a **Provider Archive** (PAR). This is a simple ZIP file that contains a signed JWT, as well as a binary file for each of the supported OS/CPU combinations.

## Provider Archive File Format
Each provider archive file contains a root `claims.jwt` file that holds a signed set of claims (see appendix). Also in the root directory of the archive are binary files containing the bytes of the native capability provider plugin with a filename of the format `[arch]-[os].bin`.

The following is an example of the contents of a provider archive file:

```
+ provider_archive.zip
|
+---- claims.jwt
|
|---- x86_64-linux.bin
|---- aarch64-linux.bin
|---- x86_64-macos.bin
`---- aarch64-ios.bin
```

Until we gain the ability to create network-capable WASI modules that can support robust capability provider functionality (like DB clients, web servers, raw TCP or UDP control, etc), Gantry will be storing and retrieving **par** files for each capability provider.

## Appendix A - Architecture values
The following is a list of some of the possible architectures (_NOTE_ not all of these architectures may be supported by the waSCC host):

* x86
* x86_64
* arm
* aarch64
* mips
* mips64

## Appendix B - Operating System Values
The following is a list of some of the possible operating systems (_NOTE_ not all of these operating systems may be supported by the waSCC host):

* linux
* macos
* ios
* freebsd
* android
* windows

## Appendix C - JSON Web Token Claims
The following is a list of the custom claims that will appear in the `wascap` section beneath the standard JWT fields. This is the same nesting style used by actor claims when embedded into a WebAssembly file:

* `hashes` - This is a map where the key is an `[arch]-[os]` string and the value is the hash for that particular file. Having these hashes inside the signed token means we can verify that the plugin binaries have not been tampered with.
* `name` - Friendly name of the capability provider.
* `vendor` - A vendor string helping to identify the provider (e.g. `Redis` or `Cassandra` or `PostgreSQL` etc). This is an information-only field and is not used as any kind of key or unique identifier.
* `capid` - The capability contract ID (e.g. `wascc:messaging` or `wascc:keyvalue`, etc). Note that the plugin itself is required to expose this information to the runtime when it receives the "query descriptor" message. This value is to allow processes other than the waSCC runtime (e.g. Gantry) to interrogate the core metadata.
* `version` - Friendly version string
* `revision` - A monotonically increasing revision value. This value will be used to retrieve / store version-specific files.


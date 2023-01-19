# Capability Providers

This repository contains capability providers for wasmCloud. The providers
in the root level of this repository are _only_ compatible with version `0.50`
and _newer_ of wasmCloud. All of the pre-existing capability providers compatible
with `0.18` (aka "pre-OTP") or earlier can be found in the [pre-otp](./pre-otp) folder.

## First-Party Capability Providers

The following is a list of first-party supported capability providers developed by the
wasmCloud team.

| Provider                                   | Contract                                                                                              | OCI Reference & Description                                                                                                                                                                                                            |
| :----------------------------------------- | :---------------------------------------------------------------------------------------------------- | :------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [blobstore-fs](./blobstore-fs)             | [`wasmcloud:blobstore`](https://github.com/wasmCloud/interfaces/tree/main/blobstore-fs)               | <img alt='blobstore fs oci reference' src='https://img.shields.io/endpoint?url=https%3A%2F%2Fdamp-firefly-3711.cosmonic.app%2Fblobstore_fs' /> <br /> Blobstore implementation where blobs are local files and containers are folders |
| [blobstore-s3](./blobstore-s3)             | [`wasmcloud:blobstore`](https://github.com/wasmCloud/interfaces/tree/main/blobstore-s3)               | <img alt='blobstore s3 oci reference' src='https://img.shields.io/endpoint?url=https%3A%2F%2Fdamp-firefly-3711.cosmonic.app%2Fblobstore-s3' /> <br /> Blobstore implementation with AWS S3                                            |
| [httpserver](./httpserver-rs)              | [`wasmcloud:httpserver`](https://github.com/wasmCloud/interfaces/tree/main/httpserver)                | <img alt='httpserver oci reference' src='https://img.shields.io/endpoint?url=https%3A%2F%2Fdamp-firefly-3711.cosmonic.app%2Fhttpserver' /> <br /> HTTP web server built with Rust and warp/hyper                                      |
| [httpclient](./httpclient)                 | [`wasmcloud:httpclient`](https://github.com/wasmCloud/interfaces/tree/main/httpclient)                | <img alt='httpclient oci reference' src='https://img.shields.io/endpoint?url=https%3A%2F%2Fdamp-firefly-3711.cosmonic.app%2Fhttpclient' /> <br />HTTP client built in Rust                                                            |
| [redis](./kvredis)                         | [`wasmcloud:keyvalue`](https://github.com/wasmCloud/interfaces/tree/main/keyvalue)                    | <img alt='kvredis oci reference' src='https://img.shields.io/endpoint?url=https%3A%2F%2Fdamp-firefly-3711.cosmonic.app%2Fkvredis' /> <br /> Redis-backed key-value implementation                                                     |
| [vault](./kv-vault)                        | [`wasmcloud:keyvalue`](https://github.com/wasmCloud/interfaces/tree/main/keyvalue)                    | <img alt='kv-vault oci reference' src='https://img.shields.io/endpoint?url=https%3A%2F%2Fdamp-firefly-3711.cosmonic.app%2Fkv-vault' /> <br /> Vault-backed key-value implementation for secrets                                       |
| [nats](./nats)                             | [`wasmcloud:messaging`](https://github.com/wasmCloud/interfaces/tree/main/messaging)                  | <img alt='nats oci reference' src='https://img.shields.io/endpoint?url=https%3A%2F%2Fdamp-firefly-3711.cosmonic.app%2Fnats_messaging' /> <br />[NATS](https://nats.io)-based message broker                                           |
| [lattice-controller](./lattice-controller) | [`wasmcloud:latticecontroller`](https://github.com/wasmCloud/interfaces/tree/main/lattice-controller) | <img alt='lattice-controller oci reference' src='https://img.shields.io/endpoint?url=https%3A%2F%2Fdamp-firefly-3711.cosmonic.app%2Flattice-controller' /> <br /> Lattice Controller interface                                        |
| [postgres](./sqldb-postgres)               | [`wasmcloud:sqldb`](https://github.com/wasmCloud/interfaces/tree/main/sqldb)                          | <img alt='sqldb-postgres oci reference' src='https://img.shields.io/endpoint?url=https%3A%2F%2Fdamp-firefly-3711.cosmonic.app%2Fsqldb-postgres' /> <br /> Postgres-based SQL database capability provider                             |

## Built-in Capability Providers

The following capability providers are included automatically in every host runtime:

| Provider | Contract                                                                                     | Description                                                 |
| :------- | :------------------------------------------------------------------------------------------- | :---------------------------------------------------------- |
| **N/A**  | [`wasmcloud:builtin:numbergen`](https://github.com/wasmCloud/interfaces/tree/main/numbergen) | Number generator, including random numbers and GUID strings |
| **N/A**  | [`wasmcloud:builtin:logging`](https://github.com/wasmCloud/interfaces/tree/main/logging)     | Basic level-categorized text logging capability             |

While neither of these providers requires a _link definition_, to use either of them your actors _must_ be signed with their contract IDs.

## Community Capability Providers

The following is a list of community supported capability providers developed by members of the wasmCloud community. Please see the [CONTRIBUTING](./CONTRIBUTING.md) guide for information on how to submit your capability provider.

| Provider                                                                                       | Contract                                                                                                   | Description                                                                                                                                                                                                                 |
| :--------------------------------------------------------------------------------------------- | :--------------------------------------------------------------------------------------------------------- | :-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [mlinference](https://github.com/Finfalter/wasmCloudArtefacts/tree/main/providers/mlinference) | [`wasmcloud:mlinference`](https://github.com/Finfalter/wasmCloudArtefacts/tree/main/providers/mlinference) | This repository provides a wasmCloud capability provider and actors to perform inference using machine learning models for ONNX and Tensorflow. [Additional Documentation](https://finfalter.github.io/wasmCloudArtefacts/) |

## Additional Examples

Additional capability provider examples and sample code can be found in the [wasmCloud examples](https://github.com/wasmCloud/examples) repository.

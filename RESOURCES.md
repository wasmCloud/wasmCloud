# Resources

Despite archiving the pure-Rust wasmCloud host, we have pinned versions of our tooling that can still be used. These resources will not be actively maintained, and they are here primarily for reference and to aid in the migration process to the [OTP Host](https://github.com/wasmCloud/wasmcloud-otp).

## Crates
| Name | Version | 
| ----------- | ----------- |
| `wasmcloud-control-interface` | [0.3.1](https://crates.io/crates/wasmcloud-control-interface/0.3.1)| 
| `wasmcloud-host` | [0.19.0](https://crates.io/crates/wasmcloud-host/0.19.0)| 
| `wasmcloud` | [0.18.2](https://crates.io/crates/wasmcloud/0.18.2)| 

## CLI Tools
| Name | Version | Notes |
| ----------- | ----------- | ----------- |
| `wash-cli` | [0.5.1](https://crates.io/crates/wash-cli)| For Windows compatiblity, you can install this crate with the `--no-default-features` flag to remove the incompatible `termion` dependency. This will remove the REPL, but allow administration on Windows machines. |

## Actor Interfaces
| Interface | 🦀 Rust | TinyGo | AssemblyScript |
| --- | :---: | :---: | :---: |
| [Core](./actor-core/core.widl) | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-actor-core)](https://crates.io/crates/wasmcloud-actor-core) | [![GitHub go.mod Go version (subdirectory of monorepo)](https://img.shields.io/github/go-mod/go-version/wasmcloud/actor-interfaces?filename=actor-core%2Fgo%2Fgo.mod)](https://pkg.go.dev/github.com/wasmcloud/actor-interfaces/actor-core/go) | [![npm](https://img.shields.io/npm/v/@wasmcloud/actor-core?color=green)](https://www.npmjs.com/package/@wasmcloud/actor-core) |
| [HTTP Server](./http-server/http.widl) | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-actor-http-server)](https://crates.io/crates/wasmcloud-actor-http-server) | [![GitHub go.mod Go version (subdirectory of monorepo)](https://img.shields.io/github/go-mod/go-version/wasmcloud/actor-interfaces?filename=http-server%2Fgo%2Fgo.mod)](https://pkg.go.dev/github.com/wasmcloud/actor-interfaces/http-server/go) | [![npm](https://img.shields.io/npm/v/@wasmcloud/actor-http-server?color=green)](https://www.npmjs.com/package/@wasmcloud/actor-http-server) |
| [HTTP Client](./http-client/http_client.widl) | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-actor-http-client)](https://crates.io/crates/wasmcloud-actor-http-client) | ⛔ | ⛔ |
| [Key-Value Store](./keyvalue/keyvalue.widl) |  [![Crates.io](https://img.shields.io/crates/v/wasmcloud-actor-keyvalue)](https://crates.io/crates/wasmcloud-actor-keyvalue)  | ⛔ | [![npm](https://img.shields.io/npm/v/@wasmcloud/actor-keyvalue?color=green)](https://www.npmjs.com/package/@wasmcloud/actor-keyvalue) |
| [Messaging](./messaging//messaging.widl) |  [![Crates.io](https://img.shields.io/crates/v/wasmcloud-actor-messaging)](https://crates.io/crates/wasmcloud-actor-messaging)  | ⛔ | ⛔ |
| [Telnet](./telnet/telnet.widl) |  [![Crates.io](https://img.shields.io/crates/v/wasmcloud-actor-telnet)](https://crates.io/crates/wasmcloud-actor-telnet)  | ⛔ | ⛔ |
| [GraphDB](./graphdb/graphdb.widl) |  [![Crates.io](https://img.shields.io/crates/v/wasmcloud-actor-graphdb)](https://crates.io/crates/wasmcloud-actor-graphdb)  | ⛔ | ⛔ |
| [Blob Store](./blobstore/blobstore.widl) |  [![Crates.io](https://img.shields.io/crates/v/wasmcloud-actor-blobstore)](https://crates.io/crates/wasmcloud-actor-blobstore)  | [![GitHub go.mod Go version (subdirectory of monorepo)](https://img.shields.io/github/go-mod/go-version/wasmcloud/actor-interfaces?filename=blobstore%2Fgo%2Fgo.mod)](https://pkg.go.dev/github.com/wasmcloud/actor-interfaces/blobstore/go) | [![npm](https://img.shields.io/npm/v/@wasmcloud/actor-blobstore?color=green)](https://www.npmjs.com/package/@wasmcloud/actor-blobstore) |
| [Event Streams](./eventstreams/eventstreams.widl) |  [![Crates.io](https://img.shields.io/crates/v/wasmcloud-actor-eventstreams)](https://crates.io/crates/wasmcloud-actor-eventstreams)  | ⛔ | ⛔ |
| [Logging](./logging/logging.widl) |  [![Crates.io](https://img.shields.io/crates/v/wasmcloud-actor-logging)](https://crates.io/crates/wasmcloud-actor-logging)  | [![GitHub go.mod Go version (subdirectory of monorepo)](https://img.shields.io/github/go-mod/go-version/wasmcloud/actor-interfaces?filename=logging%2Fgo%2Fgo.mod)](https://pkg.go.dev/github.com/wasmcloud/actor-interfaces/logging/go) | ⛔ |
| [Extras](./extras/extras.widl) |  [![Crates.io](https://img.shields.io/crates/v/wasmcloud-actor-extras)](https://crates.io/crates/wasmcloud-actor-extras)  | ⛔ | ⛔ |

## Actor OCI References
| Name | OCI Reference | 
| ----------- | ----------- |
| `echo` | `wasmcloud.azurecr.io/echo:0.2.1` | 
| `extras` | `wasmcloud.azurecr.io/extras:0.2.1` | 
| `kvcounter` | `wasmcloud.azurecr.io/kvcounter:0.2.0` | 
| `kvcounter-as` | `wasmcloud.azurecr.io/kvcounter-as:0.1.0` | 
| `logger` | `wasmcloud.azurecr.io/lgoger:0.1.0` | 
| `subscriber` | `wasmcloud.azurecr.io/subscriber:0.2.0` | 

## Capability Provider OCI References
| Capability Provider | Crate | Provider Archive OCI Reference |
|---|---|---|
| FS (File system) | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-fs)](https://crates.io/crates/wasmcloud-fs) | wasmcloud.azurecr.io/fs:0.4.1 |
| HTTP Client | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-httpclient)](https://crates.io/crates/wasmcloud-httpclient) | wasmcloud.azurecr.io/httpclient:0.2.4 |
| HTTP Server | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-httpserver)](https://crates.io/crates/wasmcloud-httpserver) | wasmcloud.azurecr.io/httpserver:0.12.3 |
| Logging | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-logging)](https://crates.io/crates/wasmcloud-logging) | wasmcloud.azurecr.io/logging:0.9.4 |
| NATS | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-nats)](https://crates.io/crates/wasmcloud-nats) | wasmcloud.azurecr.io/nats:0.10.4 |
| Redis | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-redis)](https://crates.io/crates/wasmcloud-redis) | wasmcloud.azurecr.io/redis:0.11.3 |
| Redis Streams | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-streams-redis)](https://crates.io/crates/wasmcloud-streams-redis) | wasmcloud.azurecr.io/streams-redis:0.5.3 |
| RedisGraph | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-redisgraph)](https://crates.io/crates/wasmcloud-redisgraph) | wasmcloud.azurecr.io/redisgraph:0.3.3 |
| S3 | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-s3)](https://crates.io/crates/wasmcloud-s3) | wasmcloud.azurecr.io/s3:0.10.1 |
| Telnet | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-telnet)](https://crates.io/crates/wasmcloud-telnet) | wasmcloud.azurecr.io/telnet:0.1.3 |
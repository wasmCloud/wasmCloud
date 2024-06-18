# NATS Key-Value Capability Provider

This capability provider is an implementation of the following interfaces of `wasi:keyvalue` proposal:

*   wasi:keyvalue/store\*

*   wasi:keyvalue/atomics

*   wasi:keyvalue/batch

> The NATS Kv store doesn't support a cursor, when using the `list_keys` function; therefore, all keys will be returned, irrespective of if a cursor value was provided by the user or not.

This provider is multi-threaded and can handle concurrent requests from multiple consumer components. Furthermore, consumer components can share a host supplied default configuration, or provide their bespoke provider configuration, using wasmCloud's link definitions. Each link definition declared for this provider will result in a single NATS cluster connection managed on behalf of the linked component. Connections are maintained within the provider process, so multiple instances of this provider running in the same lattice will not share connections.

## Link Definition Configuration Settings

To configure this provider, use the following settings in link definitions:

| **Property**  | **Description**                                                                                                                                                                                                                                                                                                 |
| :------------ | :-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CLUSTER_URI` | NATS cluster connection URI. If not specified, the default is `nats://0.0.0.0:4222`                                                                                                                                                                                                                             |
| `JS_DOMAIN`   | Optional NATS Jetstream domain to connect to.                                                                                                                                                                                                                                                                   |
| `BUCKET`      | The name of an existing NATS Kv Store. It must be specified. Additional links could be added if access to more Kv stores is needed; the buckets could be referenced by their respective `link_names` (please see the Rust ***keyvalue-messaging*** example for a comprehensive demonstration of this approach). |
| `CLIENT_JWT`  | Optional JWT auth token. For JWT authentication, both `CLIENT_JWT` and `CLIENT_SEED` must be provided.                                                                                                                                                                                                          |
| `CLIENT_SEED` | Private seed for JWT authentication.                                                                                                                                                                                                                                                                            |
| `TLS_CA`      | To secure communications with the NATS server, the public key of its CA could be provided as an encoded string.                                                                                                                                                                                                 |
| `TLS_CA_FILE` | Alternatively, the path qualified name of the CA public key could be provided. If both are provided, the TLS\_CA will be used.                                                                                                                                                                                  |


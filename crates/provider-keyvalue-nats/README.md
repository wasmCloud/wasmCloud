# NATS Key-Value Capability Provider

This capability provider is an implementation of the following interfaces of `wasi:keyvalue` proposal:

- wasi:keyvalue/store\*

- wasi:keyvalue/atomics

- wasi:keyvalue/batch

> The NATS Kv store doesn't support a cursor, when using the `list_keys` function; therefore, all keys will be returned, irrespective of if a cursor value was provided by the user or not.

This provider is multi-threaded and can handle concurrent requests from multiple consumer components. Furthermore, consumer components can share a host supplied default configuration, or provide their bespoke provider configuration, using wasmCloud's link definitions. Each link definition declared for this provider will result in a single NATS cluster connection managed on behalf of the linked component. Connections are maintained within the provider process, so multiple instances of this provider running in the same lattice will not share connections.

## Link Definition Configuration Settings

To configure this provider, use the following settings in link definitions:

| **Property**                | **Description**                                                                                                                                                                                                                                                                                         |
|:----------------------------|:--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `bucket`                    | **Required**: The name of an existing NATS Kv Store. Additional links could be added if access to more Kv stores is needed; the buckets could be referenced by their respective `link_names` (please see the Rust **_keyvalue-messaging_** example for a comprehensive demonstration of this approach). |
| `cluster_uri`               | NATS cluster connection URI. If not specified, the default is `nats://0.0.0.0:4222`                                                                                                                                                                                                                     |
| `js_domain`                 | Optional NATS Jetstream domain to connect to.                                                                                                                                                                                                                                                           |
| `tls_ca_file`               | Alternatively, the path qualified name of the CA public key could be provided. If both are provided, the `tls_ca` will be used.                                                                                                                                                                         |
| `enable_bucket_auto_create` | Enable automatic creation of buckets when links are established. If a bucket cannot be created, a warning is produced.                                                                                                                                                                                                                                        |

## Link Definition Secret Settings

While the provider supports receiving the following values via configuration (similar to values outlined in the configuration section above), the values below are _sensitive_, and thus _should_ be configured via link-time secrets.

| **Property**  | **Description**                                                                                                 |
| :------------ | :-------------------------------------------------------------------------------------------------------------- |
| `client_jwt`  | Optional JWT auth token. For JWT authentication, both `client_jwt` and `client_seed` must be provided.          |
| `client_seed` | Private seed for JWT authentication.                                                                            |
| `tls_ca`      | To secure communications with the NATS server, the public key of its CA could be provided as an encoded string. |

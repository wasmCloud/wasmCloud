# NATS Key-Value Capability Provider

This capability provider is an implementation of the following interfaces of `wasi:keyvalue` proposal:

*   wasi:keyvalue/store

*   wasi:keyvalue/atomics\*

*   wasi:keyvalue/batch

> ⚠️ NATS server doesn't provide atomic operation guarantees. So, this provider uses the combination of the ***NATS connection URL***, ***Jetstream Domain***,  ***NATS Bucket name***, and the ***Key name*** to guarantee the atomicity of the `increment` function calls, across its consuming components.
>
> However, this best efforts approach cannot guarantee atomicity, if multiple instances of this provider are used, targeting the same *URL + domain + bucket + key* combination for the aforementioned function.

This provider is multi-threaded and can handle concurrent requests from multiple consumer components. Furthermore, consumer components can share a host supplied default configuration, or provide their required provider configuration, using wasmCloud's link definitions. Each link definition declared for this provider will result in a single NATS cluster connection managed on behalf of the linked component. Connections are maintained within the provider process, so multiple instances of this provider running in the same lattice will not share connections.

## Link Definition Configuration Settings

To configure this provider, use the following settings in link definitions:

| **Property**  | **Description**                                                                                                                |
| :------------ | :----------------------------------------------------------------------------------------------------------------------------- |
| `CLUSTER_URI` | NATS cluster connection URI. If not specified, the default is `0.0.0.0:4222`                                                   |
| `JS_DOMAIN`   | The NATS Jetstream domain to connect to. If not provided it defaults to `core`.                                                |
| `CLIENT_JWT`  | Optional JWT auth token. For JWT authentication, both `CLIENT_JWT` and `CLIENT_SEED` must be provided.                         |
| `CLIENT_SEED` | Private seed for JWT authentication.                                                                                           |
| `TLS_CA`      | To secure communications with the NATS server, the public key of its CA could be provided as an encoded string.                |
| `TLS_CA_FILE` | Alternatively, the path qualified name of the CA public key could be provided. If both are provided, the TLS\_CA will be used. |

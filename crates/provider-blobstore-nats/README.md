# NATS Blobstore Capability Provider

This capability provider is an implementation of the following interfaces of `wasi:blobstore` proposal, backed by NATS [Object Store](https://docs.nats.io/nats-concepts/jetstream/obj_store):

- wasi:blobstore/blobstore

This provider is multi-threaded and can handle concurrent requests from multiple consumer components. Furthermore, consumer components can share a host supplied default configuration, or provide their bespoke provider configuration, using wasmCloud's link definitions. Each link definition declared for this provider will result in a single NATS cluster connection managed on behalf of the linked component. Connections are maintained within the provider process, so multiple instances of this provider running in the same lattice will not share connections.

## Link Definition Configuration Settings

To configure this provider, use the following settings in link definitions:

| **Property**    | **Environment Variable** | **Description**                                                                                                | **Default** |
|:----------------|:------------------------|:--------------------------------------------------------------------------------------------------------------|:------------|
| `cluster_uri`   | `CONFIG_NATS_URI`      | NATS cluster connection URI                                                                                    | `nats://0.0.0.0:4222` |
| `js_domain`     | `CONFIG_NATS_JETSTREAM_DOMAIN` | Optional NATS JetStream domain to connect to                                                           | None |
| `tls_ca_file`   | `CONFIG_NATS_TLS_CA_FILE` | Path qualified name of the CA public key. If both `tls_ca` and `tls_ca_file` are provided, `tls_ca` will be used | None |
| `max_write_wait` | `CONFIG_NATS_MAX_WRITE_WAIT` | Timeout for write operations in seconds. Provides better control over write operations in different environments | `30` |

### Storage Configuration Settings

The following storage-specific settings can also be configured via link definitions:

| **Property**    | **Environment Variable** | **Description**                                                                    | **Default** |
|:----------------|:------------------------|:----------------------------------------------------------------------------------|:------------|
| `max_age`       | `CONFIG_NATS_STORAGE_MAX_AGE` | Maximum age of any blob in the container, expressed in seconds              | `315,569,520` (10 years) |
| `storage_type`  | `CONFIG_NATS_STORAGE_TYPE` | The type of storage backend, either `file` or `memory`                         | `file` |
| `num_replicas`  | `CONFIG_NATS_STORAGE_NUM_REPLICAS` | How many replicas to keep for each blob in a NATS cluster             | `1` |
| `compression`   | `CONFIG_NATS_STORAGE_COMPRESSION` | Whether the underlying stream should be compressed                       | `false` |

## Link Definition Secret Settings

While the provider supports receiving the following values via configuration, these values are _sensitive_ and thus _should_ be configured via link-time secrets.

| **Property**  | **Environment Variable** | **Description**                                                                                    | **Default** |
|:--------------|:-----------------------|:--------------------------------------------------------------------------------------------------|:------------|
| `client_jwt`  | `CONFIG_NATS_CLIENT_JWT` | Optional JWT auth token. For JWT authentication, both `client_jwt` and `client_seed` must be provided | None |
| `client_seed` | `CONFIG_NATS_CLIENT_SEED` | Private seed for JWT authentication                                                              | None |
| `tls_ca`      | `CONFIG_NATS_TLS_CA` | To secure communications with the NATS server, the public key of its CA could be provided as an encoded string | None |

## Integration Tests

The provider includes comprehensive integration tests that verify its functionality against a live NATS server. These tests cover blobstore container operations, blob storage and retrieval, and a few edge cases by taking advantage of [testcontainers](https://testcontainers.org/).

### Running the Tests

To run the integration tests:

```bash
# Set the environment variables
export TESTCONTAINERS_NATS_STARTUP_TIMEOUT=30
export CONFIG_NATS_MAX_WRITE_WAIT=30

# Run the tests in release mode
cargo test --release --test integration -- --include-ignored
```

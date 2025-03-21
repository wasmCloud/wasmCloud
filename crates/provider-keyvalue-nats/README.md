# NATS Key-Value Capability Provider

This capability provider is an implementation of the following interfaces of `wasi:keyvalue` proposal:

- wasi:keyvalue/store\*

- wasi:keyvalue/atomics

- wasi:keyvalue/batch

> The NATS Kv store doesn't support a cursor, when using the `list_keys` function; therefore, all keys will be returned, irrespective of if a cursor value was provided by the user or not.

This provider is multi-threaded and can handle concurrent requests from multiple consumer components. Furthermore, consumer components can share a host supplied default configuration, or provide their bespoke provider configuration, using wasmCloud's link definitions. Each link definition declared for this provider will result in a single NATS cluster connection managed on behalf of the linked component. Connections are maintained within the provider process, so multiple instances of this provider running in the same lattice will not share connections.

## wasmCloud Application Deployment Manager (`wadm`)

If you follow the [wasmCloud Quickstart](https://wasmcloud.com/docs/tour/hello-world) to the end of the [Extend and Deploy](https://wasmcloud.com/docs/tour/extend-and-deploy) step, you can swap out the contents of the application manifest (`wadm.yaml`) with the manifest below to use `keyvalue-nats` as your provider.

```yaml
apiVersion: core.oam.dev/v1beta1
kind: Application
metadata:
  name: hello-world
  annotations:
    description: 'HTTP hello world demo'
spec:
  components:
  - name: keyvalue-nats
    type: capability
    properties:
      image: ghcr.io/wasmcloud/keyvalue-nats:0.3.1
    traits: []
  - name: http-component
    type: component
    properties:
      image: file://./build/http_hello_world_s.wasm
      id: http-component
    traits:
    - type: spreadscaler
      properties:
        instances: 100
    - type: link
      properties:
        namespace: wasi
        package: keyvalue
        interfaces:
        - store
        - atomics
        target:
          name: keyvalue-nats
          config:
          - name: wasi-keyvalue-config
            properties:
              bucket: wasmcloud
              enable_bucket_auto_create: 'true'
  - name: http-server
    type: capability
    properties:
      image: ghcr.io/wasmcloud/http-server:0.24.0
    traits:
    - type: link
      properties:
        namespace: wasi
        package: http
        interfaces:
        - incoming-handler
        source:
          config:
          - name: wasi-http-config
            properties:
              address: 127.0.0.1:8000
        target:
          name: http-component
```

When you run `wash app deploy wadm.yaml`, wasmCloud sets up:

* An HTTP server that listens on port 8000.
* A NATS-backed keyvalue provider that connects to an existing NATS environment. (Note: You will already have a NATS environment available if you are running wasmCloud with `wash up`).
* A WebAssembly component that serves HTTP requests and communicates with the keyvalue provider. (You will need to have built the component according to the Quickstart.)

After a few seconds of initialization, you can interact with the running application.

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

# NATS Key-Value Capability Provider

This capability provider is an implementation of the following interfaces of `wasi:keyvalue` proposal:

- wasi:keyvalue/store\*

- wasi:keyvalue/atomics

- wasi:keyvalue/batch

> The NATS Kv store doesn't support a cursor, when using the `list_keys` function; therefore, all keys will be returned, irrespective of if a cursor value was provided by the user or not.

This provider is multi-threaded and can handle concurrent requests from multiple consumer components. Furthermore, consumer components can share a host supplied default configuration, or provide their bespoke provider configuration, using wasmCloud's link definitions. Each link definition declared for this provider will result in a single NATS cluster connection managed on behalf of the linked component. Connections are maintained within the provider process, so multiple instances of this provider running in the same lattice will not share connections.

## wasmCloud Application Deployment Manager (`wadm`)

To use the NATS Key-Value provider with an example component similar to the [wasmCloud Quickstart](https://wasmcloud.com/docs/tour/hello-world),
use `wash new` to create a new component that with the [`http-keyvalue-counter` template][example-http-kv-counter]:

```console
wash new component \
    http-kv-counter
    --git github.com/wasmcloud/wasmcloud \
    --subfolder examples/rust/components/http-keyvalue-counter
```

> [!INFO]
> For more information, check out the [code for the `http-keyvalue-counter` example][example-http-kv-counter]

After entering the created `http-kv-counter` directory, you can `wash build` the project, and use the generated
`build/http_keyvalue_counter_s.wasm` along with the application manifest below to see the provider in action:

```yaml
apiVersion: core.oam.dev/v1beta1
kind: Application
metadata:
  name: http-kv-counter
  annotations:
    version: v0.0.1
    description: 'HTTP counter demo in Rust, using the WebAssembly Component Model and WebAssembly Interfaces Types (WIT)'
spec:
  components:
    - name: counter
      type: component
      properties:
        image: file://./build/http_keyvalue_counter_s.wasm
      traits:
        - type: spreadscaler
          properties:
            instances: 1
        # Link the component to the NATS KV provider
        # for when it does KV calls
        - type: link
          properties:
            namespace: wasi
            package: keyvalue
            interfaces:
            - store
            - atomics
            target:
              name: kv-nats
              config:
              - name: wasi-keyvalue-config
                properties:
                  bucket: wasmcloud
                  enable_bucket_auto_create: 'true'

    # Add a capability provider for key-value access using NATS
    - name: kv-nats
      type: capability
      properties:
        image: ghcr.io/wasmcloud/keyvalue-nats:0.3.1

    # Add a capability provider that enables HTTP access
    - name: httpserver
      type: capability
      properties:
        image: ghcr.io/wasmcloud/http-server:0.27.0
      traits:
        # Link the httpserver to the component, and configure the HTTP server
        # to listen on port 8000 for incoming requests
        - type: link
          properties:
            namespace: wasi
            package: http
            interfaces: [incoming-handler]
            target:
              name: counter
            source:
              config:
                - name: default-http
                  properties:
                    address: 0.0.0.0:8000
```

When you run `wash app deploy <path to yaml>`, wasmCloud sets up:

* A NATS-powered Key-Value provider that connects to an existing NATS environment. (Note: You will already have a NATS environment available if you are running wasmCloud with `wash up`).
* A WebAssembly component that enables access to the Key-Value NATS provider, incrementing a key when invoked
* An HTTP server that listens on port 8000 and exposes the component via HTTP

After a few seconds of initialization, you can interact with the running application.

To increment a counter value, you can use `curl`:

```console
curl http://127.0.0.1:8000/counter
```

You should see output like the following, indicating that the counter was incremented (from zero):

```
Counter /counter: 1
```

If you repeat the `curl` command, you should see:

```
Counter /counter: 2
```

[example-http-kv-counter]: https://github.com/wasmCloud/wasmCloud/tree/main/examples/rust/components/http-keyvalue-counter

## Link Definition Configuration Settings

To configure this provider, use the following settings in link definitions:

| **Property**                | **Description**                                                                                                                                                                                                                                                                                         |
|-----------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `bucket`                    | **Required**: The name of an existing NATS Kv Store. Additional links could be added if access to more Kv stores is needed; the buckets could be referenced by their respective `link_names` (please see the Rust **_keyvalue-messaging_** example for a comprehensive demonstration of this approach). |
| `cluster_uri`               | NATS cluster connection URI. If not specified, the default is `nats://0.0.0.0:4222`                                                                                                                                                                                                                     |
| `js_domain`                 | Optional NATS Jetstream domain to connect to.                                                                                                                                                                                                                                                           |
| `tls_ca_file`               | Alternatively, the path qualified name of the CA public key could be provided. If both are provided, the `tls_ca` will be used.                                                                                                                                                                         |
| `enable_bucket_auto_create` | Enable automatic creation of buckets when links are established. If a bucket cannot be created, a warning is produced.                                                                                                                                                                                  |

## Link Definition Secret Settings

While the provider supports receiving the following values via configuration (similar to values outlined in the configuration section above), the values below are _sensitive_, and thus _should_ be configured via link-time secrets.

| **Property**  | **Description**                                                                                                 |
| :------------ | :-------------------------------------------------------------------------------------------------------------- |
| `client_jwt`  | Optional JWT auth token. For JWT authentication, both `client_jwt` and `client_seed` must be provided.          |
| `client_seed` | Private seed for JWT authentication.                                                                            |
| `tls_ca`      | To secure communications with the NATS server, the public key of its CA could be provided as an encoded string. |

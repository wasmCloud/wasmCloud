<img alt='kvredis oci reference' src='https://img.shields.io/endpoint?url=https%3A%2F%2Fwasmcloud-ocireferences.cosmonic.app%2Fkvredis' />

# Redis Key Value provider

This capability provider implements the [wasi:keyvalue WIT interface](https://github.com/WebAssembly/wasi-keyvalue) with a [Redis][redis] back-end.

This provider is multi-threaded and can handle concurrent requests from multiple components. Each link definition declared for this provider will result in a single Redis connection managed on behalf of the linked component. Connections are maintained within the provider process, so multiple instances of this provider running in the same lattice will not share connections.

If you want multiple components to share the same keyspace/database then you will need to provide the same Redis URL for multiple link definitions (or utilize start-up configuration as discussed below).

[redis]: https://redis.io/docs/latest

## Quickstart

### Manual start

The easiest way to use this provider is to pass `ghcr.io/wasmcloud/kvredis:0.26.0` (or newer, check the badge at the top of this README) as the OCI reference parameter to a `wash start provider` command.

```console
wash start provider ghcr.io/wasmcloud/kvredis:0.26.0
```

### wasmCloud Application Deployment Manager ("WADM")

This provider can be easily orchestrated with real workloads via [`wash app deploy`][wasmcloud-docs-wash-app-deploy].

With the following in a `wadm.yaml` file:

```yaml
apiVersion: core.oam.dev/v1beta1
kind: Application
metadata:
  name: counter-app
  annotations:
    version: v0.0.1
    description: "HTTP counter demo website"
spec:
  components:
    - name: counter-app
      type: component
      properties:
        image: ghcr.io/wasmcloud/components/http-keyvalue-counter-rust:0.1.0
      traits:
        # Govern the spread/scheduling of the component
        - type: spreadscaler
          properties:
            replicas: 1
        # Link the component to Redis on the default Redis port
        #
        # Establish a unidirectional link to the `kvredis` (the keyvalue capability provider),
        # so the `counter-app` component can make use of keyvalue functionality provided by the Redis
        # (i.e. using a keyvalue cache)
        - type: link
          properties:
            target: kvredis
            namespace: wasi
            package: keyvalue
            interfaces: [atomics, store]
            target_config:
              - name: redis-url
                properties:
                  url: redis://127.0.0.1:6379

    # Add a capability provider that enables Redis access
    - name: kvredis
      type: capability
      properties:
        image: ghcr.io/wasmcloud/keyvalue-redis:0.25.0

    # Add a capability provider that enables HTTP access
    - name: httpserver
      type: capability
      properties:
        image: ghcr.io/wasmcloud/http-server:0.21.0
      traits:
        # Link the httpserver to the component, and configure the HTTP server
        # to listen on port 8080 for incoming requests
        #
        # Since the HTTP server calls the `counter-app` component, we establish
        # a unidirectional link from this `httpserver` provider (the "source")
        # to the `counter-app` component (the "target"), so the server can invoke
        # the component to handle a request.
        - type: link
          properties:
            target: counter-app
            namespace: wasi
            package: http
            interfaces: [incoming-handler]
            source_config:
              - name: default-http
                properties:
                  address: 127.0.0.1:8080
```

By running `wash app deploy wadm.yaml`, WADM sets up:

- A HTTP server which listens on port 8080
- A Redis keyvalue provider which receives connects to a local redis instance on port 6379
- `counter-app` component that serves HTTP requests *and* communicates with the keyvalue provider

After a few seconds of initialization, you can interact with the running application.

For more information on what you can do with the component, see the [`http-keyvalue-counter` example in the wasmCloud examples folder](https://github.com/wasmCloud/wasmCloud/tree/main/examples/rust/components/http-keyvalue-counter)

[wasmcloud-docs-wash-app-deploy]: https://wasmcloud.com/docs/cli/app#deploy

## Link Definition Secret Settings

| Name  | Description                                                                                                                                                                                                |
|-------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `URL` | The connection string for the Redis database. Note that all authentication information must also be contained in this URL. The URL _must_ start with the `redis://` scheme. (ex. `redis://127.0.0.1:6379`) |

> ![WARNING]
> Putting sensitive configuration values in WADM files should be avoided.
>
> Please use the secrets feature to provide sensitive values like a Redis connection URL
> (i.e. specifying the secret name in `wadm.yaml` and ensuring to run `wash secret put` separately).
>
> While this provider will still accept `URL` values from [named configuration][wasmcloud-docs-named-config] for the
> sake of backwards compatibility, such functionality will be removed in a future version.

[wasmcloud-docs-named-config]: https://wasmcloud.com/docs/developer/components/configure#supplying-multiple-configurations

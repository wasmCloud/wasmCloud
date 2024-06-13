# HTTP Client Capability Provider

This capability provider implements the `wasi:http/outgoing-handler` interface, and enables an component to make outgoing HTTP(s) requests. It is implemented in Rust using the [hyper](https://hyper.rs/) library.

This capability provider is multi-threaded and can handle concurrent requests from multiple components.

## Configuration

| Key                 | Value                | Description                                                                                                                                                                                | Default |
| ------------------- | -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------- |
| `load_native_certs` | "true" / "false"     | Use the platform's native certificate store at runtime. Any value other than "true" will be assumed as "false"                                                                             | "true"  |
| `load_webpki_certs` | "true" / "false"     | Uses a compiled-in set of root certificates trusted by Mozilla. Any value other than "true" will be assumed as "false"                                                                     | "true"  |
| `ssl_certs_file`    | "/path/to/certs.pem" | Path to a file available on the machine where the HTTP client runs that contains one or more root certificates to trust. The provider will fail to instantiate if the file is not present. | N/A     |

An example of starting this provider with all of the configuration values looks like this in `wash`:

```bash
wash config put http-client-config load_native_certs=true load_webpki_certs=true ssl_certs_file=/tmp/certs.pem
wash start provider ghcr.io/wasmcloud/http-client:0.11.0 http-client --config http-client-config
```

An example of starting this provider with all of the configuration values looks like this in `wadm`:

```yaml
- name: httpclient
  type: capability
  properties:
    image: ghcr.io/wasmcloud/http-client:0.11.0
    config:
      - name: http-client-config
        properties:
          load_native_certs: 'true'
          load_webpki_certs: 'true'
          ssl_certs_file: /tmp/certs.pem
```

## Link Definition Values

This capability provider does not have any link definition configuration values.

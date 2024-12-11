# HTTP Client Example

This is a simple TinyGo Wasm example that makes accepts a http request, then makes a http call to an external endpoint and returns a response for the request.

## Prerequisites

- `go` 1.23.2
- `tinygo` 0.33
- [`wash`](https://wasmcloud.com/docs/installation) 0.36.1

## Building

```bash
wash build
```

## Running with wasmCloud

Make sure to follow the build steps above, and replace the file path in [the wadm manifest](./wadm.yaml) with the absolute path to your local built component.

```
wash up -d
wash app deploy ./wadm.yaml
curl http://localhost:8000
```

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=tinygo) section of the wasmCloud documentation.

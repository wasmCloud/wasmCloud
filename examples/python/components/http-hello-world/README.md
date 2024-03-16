# HTTP Hello World

This is a simple Python Wasm example that responds with a "Hello World" message for each request.

## Prerequisites

- `python` 3.10 or greater
- `pip`
- `componentize-py` 0.11.0
- `wash` 0.26.0
- `wasmtime` 17.0.0 (if running with wasmtime)

## Installing componentize-py

After installing Python and pip, run the following command to install `componentize-py`:

```bash
pip install componentize-py
```

## Building

```bash
wash build
```

## Running with wasmtime

You must have wasmtime 17.0.0 for this to work. Make sure to follow the build step above first.

```bash
wasmtime serve -Scommon ./build/http_hello_world_s.wasm
```

## Running with wasmCloud

Make sure to follow the build steps above, and replace the file path in [the wadm manifest](./wadm.yaml) with the absolute path to your local built component.

```
wash up -d
wash app deploy ./wadm.yaml
curl http://localhost:8080
```

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=python) section of the wasmCloud documentation.

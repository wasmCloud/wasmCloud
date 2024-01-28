# HTTP Hello World

This is a simple Python Wasm example that responds with a "Hello World" message for each request.

## Prerequisites

- `python` 3.10 or greater
- `pip`
- `componentize-py` 0.9.2
- `wash` 0.25.0
- `wasmtime` 16.0.0 (if running with wasmtime)

## Installing componentize-py

After installing Python and pip, run the following command to install `componentize-py`:

```bash
pip install componentize-py==0.9.2
```

## Building

```bash
wash build
```

## Running with wasmtime

You must have wasmtime 16.0.0 for this to work. Make sure to follow the build step above first.

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

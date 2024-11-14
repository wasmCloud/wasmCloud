# HTTP Blobstore

This is a Rust Wasm example that serves HTTP requests to store and retrieve files from a linked blobstore.

## Prerequisites

- `cargo` 1.75
- [`wash`](https://wasmcloud.com/docs/installation) 0.26.0

## Building

First, build and sign this component using `wash`:

```bash
wash build
```

Next you'll need a compatible blobstore capability provider. You can use the [blobstore-fs](../../../../crates/providers/blobstore-fs/) capability provider, simply navigate to that folder and run the following commands to build and package the provider:

```bash
cargo build --release
wash par create \
    --capid wasmcloud:blobstore \
    --vendor wasmcloud \
    --name blobstore-fs \
    --binary ../target/release/blobstore_fs \
    --compress
```

## Running with wasmCloud

Ensuring you've built your component with `wash build`, you can launch wasmCloud and deploy the full blobstore application with the following commands. Once the application reports as **Deployed** in the application list, you can use `curl` to send a request to the running HTTP server.

```shell
wash up -d
wash app deploy ./wadm.yaml
wash app get
curl http://localhost:8000
```

## Where are the Files coming from?

For each HTTP request that you send this component, it will retrieve the number of objects that are in your `/tmp/<component_id>` folder. The component ID used is based on the public key of the component that we automatically generate for you, which you can find by inspecting the component:

```bash
wash inspect ./build/http_blobstore_s.wasm
```

```bash
➜ wash inspect ./build/http_blobstore_s.wasm
                           http-blobstore - Component
  Account         ACDYADNIGBP3CHAXB2BITGZY74QPPJIWA2XHXMGXNKRSBPTC2FQPX422
  # The 56 character key starting with `M` is your component ID
  Component           MC6I62ZMZGOSLG4WONRNAKUYXN6GOCCVBWOPL5QNIXG5JP4Y6ADM6NK5
  Expires                                                            never
  ...
```

Using the example above, you can list the files for this component by running:

```
ls /tmp/MC6I62ZMZGOSLG4WONRNAKUYXN6GOCCVBWOPL5QNIXG5JP4Y6ADM6NK5
```

If you're looking to extend this example to read, write, or list files, you can simply place them in the temporary directory above so that your component can access them.

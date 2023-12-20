# HTTP Dog Fetcher

This is a Rust Wasm example that uses `wasi:http` and `wasi:blobstore` to receive HTTP requests, fetch a random dog image from [https://random.dog](https://random.dog), and write the image to the blobstore. This example uses `wasi:logging` via convenience macros from [wasmcloud-actor](../../../../crates/actor) to log messages.

## WIT World

```
$ wash inspect --wit build/http_dog_fetcher_s.wasm

package root:component;

world root {
  import wasi:io/poll@0.2.0-rc-2023-11-10;
  import wasi:io/error@0.2.0-rc-2023-11-10;
  import wasi:io/streams@0.2.0-rc-2023-11-10;
  import wasi:http/types@0.2.0-rc-2023-12-05;
  import wasi:http/outgoing-handler@0.2.0-rc-2023-12-05;
  import wasi:blobstore/types;
  import wasi:blobstore/container;
  import wasi:blobstore/blobstore;
  import wasi:logging/logging;
  import wasi:cli/environment@0.2.0-rc-2023-12-05;
  import wasi:cli/exit@0.2.0-rc-2023-12-05;
  import wasi:cli/stdin@0.2.0-rc-2023-12-05;
  import wasi:cli/stdout@0.2.0-rc-2023-12-05;
  import wasi:cli/stderr@0.2.0-rc-2023-12-05;
  import wasi:clocks/wall-clock@0.2.0-rc-2023-11-10;
  import wasi:filesystem/types@0.2.0-rc-2023-11-10;
  import wasi:filesystem/preopens@0.2.0-rc-2023-11-10;

  export wasi:http/incoming-handler@0.2.0-rc-2023-12-05;
}
```

## Prerequisites

- `cargo` 1.74
- `wash` 0.25.0

## Building

```bash
wash build
```

## Running with wasmCloud

Make sure to follow the build steps above, and replace the file path in [the wadm manifest](./wadm.yaml) with the absolute path to your local built component.

```
wash up -d
wash app deploy ./wadm.yaml
curl http://localhost:8081
```

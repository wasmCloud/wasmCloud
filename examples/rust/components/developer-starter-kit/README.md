# Rust Developer Starter Kit

This is a Rust component example that can serve as a starter kit for all of your wasmCloud applications. It uses the [wasmcloud_component](https://crates.io/crates/wasmcloud-component) and [wasi](https://crates.io/crates/wasi) crates to demonstrate the standard capabilities that wasmCloud supports. This is an instructional example, and is meant to be modified to remove unneeded capabilities to create your application.

## Prerequisites

- `cargo` 1.81
- [`wash`](https://wasmcloud.com/docs/installation) 0.33.0

## Build & Run

Developing this application and customizing it for your own needs is best done with `wash dev`, which will automatically build this component, run wasmCloud and deploy capability providers based on the capabilities used in the code.

```bash
wash dev
```

Make sure to take note of the logs directory, HTTP server endpoint, and messaging subscription for viewing logs and invoking the component.

```plaintext
🔧 Successfully started wasmCloud instance
✅ Successfully started host, logs writing to /Users/brooks/.wash/dev/PHrCNP/wasmcloud.log
🚧 Building project...
ℹ️ Detected component dependencies: {"messaging-nats", "http-server", "blobstore-fs", "http-client", "keyvalue-nats"}
🔁 Deployed development manifest for application [dev-phrcnp-starter-kit]
✨ Messaging NATS: Listening on the following subscriptions [wasmcloud.dev]
✨ HTTP Server: Access your application at http://127.0.0.1:8000
```

## Functionality

This component implements a variety of HTTP API endpoints, and a custom messaging handler, to showcase how to use different capability interfaces in wasmCloud.

### HTTP API

#### Hello World

```plaintext
GET /
GET /hello
```

Handle a basic HTTP request with a hello world message. Both the `/` and `/hello` endpoints are handled by the same function, so they respond the same.

**Request**:

```shell
curl http://localhost:8000/
```

```shell
curl http://localhost:8000/hello
```

**Response**:

```plaintext
Hello from Starter Kit!
```

#### Streaming Response

```plaintext
POST /echo
```

Stream the request body back to the client using WASI streams.

**Request**:

```bash
curl -X POST localhost:8000/echo -d "Hello there"
```

**Response**:

```plaintext
Hello there%
```

**Request**:
This example streams this Wasm component and outputs the response bytes to a file, comparing the output to the original file afterwards.

```bash
curl -X POST localhost:8000/echo -T ./build/starter_kit.wasm > ./streamed.wasm
diff ./build/starter_kit.wasm ./streamed.wasm
```

**Response**:

No output from `diff` means the files were identical.

```plaintext
  % Total    % Received % Xferd  Average Speed   Time    Time     Time  Current
                                 Dload  Upload   Total   Spent    Left  Speed
100  293k  100  146k  100  146k  72.0M  72.0M --:--:-- --:--:-- --:--:--  286M
```

#### Streaming to a File

```plaintext
POST /file
```

Stream the request body directly to a file using WASI streams.

This example streams this Wasm component over HTTP and compares the output file to the original. If running this example with `wash dev`, the default location for files will be the `/tmp` directory.

**Request:**

```bash
curl -X POST localhost:8000/file -T ./build/starter_kit.wasm
diff /tmp/starter_kit/streaming_data ./build/starter_kit.wasm
```

**Response**:

No output from `diff` means the files were identical.

```plaintext
  % Total    % Received % Xferd  Average Speed   Time    Time     Time  Current
                                 Dload  Upload   Total   Spent    Left  Speed
100  293k  100  146k  100  146k  72.0M  72.0M --:--:-- --:--:-- --:--:--  286M
```

#### Make an Outgoing HTTP Request

```plaintext
GET /example_dot_com
```

Make an outgoing HTTP request to [https://example.com](https://example.com), streaming the response back to the client using WASI streams.

**Request**:

```bash
curl localhost:8000/example_dot_com
```

**Response**:

```plaintext
<!doctype html>
<html>
<head>
    <title>Example Domain</title>
...
```

#### Increment a Counter

```plaintext
GET /counter
```

Increment the request counter, then return the current count.

**Request**:

```bash
curl localhost:8000/counter
```

**Response**:

```plaintext
Counter: n
```

### Messaging API

This component also implements a messaging API, which can handle messages and send a message in response if a `reply_to` subject is provided.

If running this component with `wash dev`, you can send a request to the `wasmcloud.dev` subject using the [NATS CLI](https://github.com/nats-io/natscli)

**Request:**

```bash
nats req "wasmcloud.dev" "hello there"
```

**Response:**

```plaintext
14:10:47 Sending request on "wasmcloud.dev"
14:10:47 Received with rtt 868.917µs
hello there
```

### Logging

For each request received by this component, it logs an `Info` level log detailing the request. This uses the [`wasmcloud_component::info!`](https://docs.rs/wasmcloud-component/latest/wasmcloud_component/macro.info.html) macro.

## Adding Custom Capabilities

<!-- TODO: new quickstart -->

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities) section of the wasmCloud documentation.

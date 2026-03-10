# gRPC Hello World

This example demonstrates how to make gRPC calls from a wasmCloud component using the `wasmcloud-grpc-client` crate. The component uses the `wasi:http/outgoing-handler` interface to make HTTP/2 requests to a gRPC server, with automatic protocol handling by the wash-runtime.

## Architecture

```
Component (Wasm)
    ↓ uses wasmcloud-grpc-client crate
    ↓ generates tonic::GreeterClient
    ↓ calls service methods (say_hello)
    ↓
GrpcEndpoint (implements tower::Service)
    ↓ converts gRPC requests to WASI outgoing-http requests
    ↓ adds Content-Type: application/grpc header
    ↓
Host Runtime (wash-runtime)
    ↓ detects gRPC via Content-Type header
    ↓ routes to grpc-client plugin
    ↓ enforces HTTP/2 protocol
    ↓
gRPC Server
```

## Prerequisites

- Rust 1.82+
- [`wash`](https://wasmcloud.com/docs/installation) 0.36.1
- A running gRPC server (see "Running the Test Server" below)
- `wasmcloud-grpc-client` crate (located at `/home/aditya-sal/Desktop/wasmcloud-grpc-client`)

## Project Structure

```
grpc-hello-world/
├── proto/
│   └── helloworld.proto    # Greeter service definition
├── build.rs                # Compiles .proto files with tonic-build
├── src/
│   └── lib.rs             # Component implementation
└── Cargo.toml             # Dependencies: tonic, prost, wasmcloud-grpc-client
```

## Building

The build process will:
1. Compile `helloworld.proto` to Rust code (via `build.rs`)
2. Generate `GreeterClient` with tonic
3. Build the WebAssembly component

```bash
wash build
```

The compiled component will be at `./build/grpc_hello_world_s.wasm`.

## Running the Test Server

You'll need a gRPC server implementing the `Greeter` service. Here's a simple example:

```rust
// server.rs
use tonic::{transport::Server, Request, Response, Status};

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};

#[derive(Debug, Default)]
pub struct MyGreeter {}

#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        let reply = HelloReply {
            message: format!("Hello {}!", request.into_inner().name),
        };
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50051".parse()?;
    let greeter = MyGreeter::default();

    Server::builder()
        .add_service(GreeterServer::new(greeter))
        .serve(addr)
        .await?;

    Ok(())
}
```

Run the server:
```bash
cargo run --bin server
```

## Running with wasmCloud

Start the wasmCloud host with the component:

```bash
wash dev
```

The component will be available at the configured HTTP endpoint. You can test it by making requests:

```bash
curl http://localhost:8080/greet?name=World
```

## How It Works

1. **Proto Definition**: `helloworld.proto` defines the `Greeter` service with a `SayHello` RPC
2. **Code Generation**: `build.rs` uses `tonic_build` to generate Rust client code
3. **Component Code**: `lib.rs` creates a `GrpcEndpoint` pointing to the server URI
4. **Request Flow**:
   - Component calls `client.say_hello(request).await`
   - `GrpcEndpoint` converts it to a WASI `outgoing-request`
   - Adds `Content-Type: application/grpc` header
   - wash-runtime detects gRPC and routes to HTTP/2 client
   - Response is streamed back and decoded by tonic

## Protocol Details

- **HTTP/2 Only**: The gRPC client plugin enforces HTTP/2 (h2c for cleartext, h2 for TLS)
- **ALPN**: TLS connections negotiate HTTP/2 via ALPN with preference for `h2`
- **Connection Pooling**: The underlying hyper client maintains connection pools
- **Streaming**: Both unary and streaming RPCs are supported through the `WasiResponseBody` adapter

## Troubleshooting

**Build errors**: Ensure `tonic-build` is in `[build-dependencies]` and the proto file path is correct

**Runtime errors**: Check that:
- The gRPC server is running and accessible
- The `GRPC_SERVER_URI` environment variable is set correctly
- The wash-runtime has the grpc-client plugin loaded

**Connection failures**: Verify:
- Server address in `GrpcEndpoint::new()`
- Firewall settings allow HTTP/2 connections
- TLS certificates if using HTTPS

```shell
curl http://127.0.0.1:8000
```

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=rust) section of the wasmCloud documentation.

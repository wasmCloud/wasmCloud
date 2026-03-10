# gRPC Hello World

This example demonstrates how to make gRPC calls from a wasmCloud component. The component uses the `wasi:http/outgoing-handler` interface to make HTTP/2 requests to a gRPC server, with automatic protocol handling by the wash-runtime.

## Architecture

```
Component (Wasm)
    ↓ generates tonic::GreeterClient
    ↓ calls service methods (say_hello)
    ↓
WasiGrpcService (implements tower::Service)
    ↓ converts gRPC requests to WASI outgoing-http requests via wstd crate
    ↓ adds Content-Type: application/grpc header
    ↓
Host Runtime (wash-runtime)
    ↓ detects gRPC via Content-Type header
    ↓ enforces HTTP/2 protocol
    ↓
gRPC Server
```

## Running the Test Server

Start the gRPC Server on port 50051:

```bash
cargo run -p bin-server
```

In another shell, start a wash dev session:

```bash
wash dev
```

Navigate to [http://localhost:8000](http://localhost:8000)

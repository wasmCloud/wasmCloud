# gRPC Hello World

This example demonstrates how to make gRPC calls from a wasmCloud component.


## Component as gRPC Client

Start the gRPC Server on port 50051:

```bash
cargo run -p bin-server
```

In another shell, start a wash dev session:

```bash
wash -C component-client dev
```

Navigate to [http://localhost:8000](http://localhost:8000)

## Component as gRPC Server

Start the gRPC Server using wash dev:

```bash
wash -C component-server dev
```

In another shell, run the gRPC Client:

```bash
cargo run -p bin-client
```

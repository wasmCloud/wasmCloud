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

Outbound HTTP from the component to `localhost:50051` is gated by the host runtime and permitted via `workload.allowedHosts` in [`component-client/.wash/config.yaml`](component-client/.wash/config.yaml).

## Component as gRPC Server

Start the gRPC Server using wash dev:

```bash
wash -C component-server dev
```

In another shell, run the gRPC Client:

```bash
cargo run -p bin-client
```

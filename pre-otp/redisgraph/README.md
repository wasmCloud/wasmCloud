[![crates.io](https://img.shields.io/crates/v/wasmcloud-redisgraph.svg)](https://crates.io/crates/wasmcloud-redisgraph)
![Rust](https://github.com/wasmcloud/capability-providers/workflows/REDISGRAPH/badge.svg)
![license](https://img.shields.io/crates/l/wasmcloud-redisgraph.svg)
[![documentation](https://docs.rs/wasmcloud-redisgraph/badge.svg)](https://docs.rs/wasmcloud-redisgraph)

# wasmCloud Graph Database Provider (Redis Graph)

This repository contains a [sample actor](./examples/sample-graph-actor), and the main capability provider [library](./src).

While the actor and common libraries should be usable across different types of graph databases, this provider is build on top of [Redis Graph](https://oss.redislabs.com/redisgraph/).

The following sample shows just how few lines of code are required to build an actor that responds to HTTP requests, reads and writes graph data, and exposes results over HTTP as JSON:

```rust
actor_handlers! { codec::http::OP_HANDLE_REQUEST => handle_http_request,
                  codec::core::OP_HEALTH_REQUEST => health }

fn handle_http_request(req: codec::http::Request) -> HandlerResult<codec::http::Response> {    
    if req.method.to_uppercase() == "POST" {
        create_data()
    } else {
        query_data()
    }
}

// Execute a Cypher query to return data values
fn query_data() -> HandlerResult<codec::http::Response> {
    let (name, birth_year): (String, u32) =
        graph::default().graph("MotoGP")
            .query("MATCH (r:Rider)-[:rides]->(t:Team) WHERE t.name = 'Yamaha' RETURN r.name, r.birth_year")?;

    let result = json!({
        "name": name,
        "birth_year": birth_year
    });
    Ok(codec::http::Response::json(result, 200, "OK"))
}

fn health(_req: codec::core::HealthRequest) -> HandlerResult<()> {
    Ok(())
}
```

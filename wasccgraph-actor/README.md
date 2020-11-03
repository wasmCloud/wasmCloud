# waSCC Graph Actor API

This crate provides [waSCC actors](https://github.com/wascc) with an API they can use to interact with a graph database. The exact implementation of the graph database (Neo4j, RedisGraph, etc) is immaterial to the actor developer using this API.

The following illustrates an example of consuming the graph guest API:

```
// Execute a Cypher query to add data
fn create_data() -> HandlerResult<codec::http::Response> {
    info!("Creating graph data");
    graph::default().graph("MotoGP").mutate("CREATE (:Rider {name: 'Valentino Rossi', birth_year: 1979})-[:rides]->(:Team {name: 'Yamaha'}), \
    (:Rider {name:'Dani Pedrosa', birth_year: 1985, height: 1.58})-[:rides]->(:Team {name: 'Honda'}), \
    (:Rider {name:'Andrea Dovizioso', birth_year: 1986, height: 1.67})-[:rides]->(:Team {name: 'Ducati'})")?;

    Ok(codec::http::Response::ok())
}

// Execute a Cypher query to return data values
fn query_data() -> HandlerResult<codec::http::Response> {
    info!("Querying graph data");
    let (name, birth_year): (String, u32) = graph::default().graph("MotoGP").query(
        "MATCH (r:Rider)-[:rides]->(t:Team) WHERE t.name = 'Yamaha' RETURN r.name, r.birth_year",
    )?;

    let result = json!({
        "name": name,
        "birth_year": birth_year
    });
    Ok(codec::http::Response::json(result, 200, "OK"))
}
```

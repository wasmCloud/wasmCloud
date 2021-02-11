use redisgraph;
use redisgraph::result_set::Column::*;
use redisgraph::result_set::Scalar;
use std::collections::HashMap;
use wasmcloud_actor_graphdb;
use wasmcloud_actor_graphdb::generated::Column;

pub(crate) fn redisgraph_column_to_common(
    rc: redisgraph::result_set::Column,
) -> wasmcloud_actor_graphdb::generated::Column {
    match rc {
        Scalars(s) => {
            let scalars = Some(
                s.into_iter()
                    .map(redisgraph_scalar_to_common)
                    .collect::<Vec<_>>(),
            );
            Column {
                scalars,
                ..Default::default()
            }
        }
        Nodes(n) => {
            let nodes = Some(
                n.into_iter()
                    .map(redisgraph_node_to_common)
                    .collect::<Vec<_>>(),
            );
            Column {
                nodes,
                ..Default::default()
            }
        }
        Relations(r) => {
            let relations = Some(
                r.into_iter()
                    .map(redisgraph_relation_to_common)
                    .collect::<Vec<_>>(),
            );
            Column {
                relations,
                ..Default::default()
            }
        }
        _ => Column::default(),
    }
}

pub(crate) fn redisgraph_scalar_to_common(
    rs: redisgraph::result_set::Scalar,
) -> wasmcloud_actor_graphdb::generated::Scalar {
    let mut scalar = wasmcloud_actor_graphdb::generated::Scalar::default();
    match rs {
        Scalar::Boolean(b) => scalar.bool_value = Some(b),
        Scalar::Double(d) => scalar.double_value = Some(d),
        Scalar::Integer(i) => scalar.int_value = Some(i),
        Scalar::String(s) => scalar.string_value = Some(redisstring_to_string(s)),
        Nil => (),
    };
    scalar
}

pub(crate) fn redisgraph_node_to_common(
    rn: redisgraph::result_set::Node,
) -> wasmcloud_actor_graphdb::generated::Node {
    let labels = rn
        .labels
        .into_iter()
        .map(redisstring_to_string)
        .collect::<Vec<_>>();
    let properties = rn
        .properties
        .into_iter()
        .map(|(k, v)| (redisstring_to_string(k), redisgraph_scalar_to_common(v)))
        .collect::<HashMap<_, _>>();
    wasmcloud_actor_graphdb::generated::Node { labels, properties }
}

pub(crate) fn redisgraph_relation_to_common(
    rr: redisgraph::result_set::Relation,
) -> wasmcloud_actor_graphdb::generated::Relation {
    let type_name = redisstring_to_string(rr.type_name);
    let properties = rr
        .properties
        .into_iter()
        .map(|(k, v)| (redisstring_to_string(k), redisgraph_scalar_to_common(v)))
        .collect::<HashMap<_, _>>();
    wasmcloud_actor_graphdb::generated::Relation {
        type_name,
        properties,
    }
}

fn redisstring_to_string(rs: redisgraph::RedisString) -> String {
    String::from_utf8(rs.into()).unwrap()
}

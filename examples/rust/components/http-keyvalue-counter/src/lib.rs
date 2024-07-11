#![allow(clippy::missing_safety_doc)]
wit_bindgen::generate!({ generate_all });

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::*;

use wasmcloud::bus::lattice;

struct HttpServer;

impl Guest for HttpServer {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        // Get the path & query to the incoming request
        let path_with_query = request
            .path_with_query()
            .expect("failed to get path with query");

        // At first, we can assume the object name will be the path with query
        // (ex. simple paths like '/some-key-here')
        let mut object_name = path_with_query.clone();

        // Let's assume we want to connect to & invoke the default keyvalue provider
        // that is linked with this component
        let mut link_name = "default";

        // If query parameters were supplied, then we need to recalculate the object_name
        // and take special actions if some parameters (like `link_name`) are present (and ignore the rest)
        if let Some((path, query)) = path_with_query.split_once('?') {
            object_name = path.to_string();

            let query_params = query
                .split('&')
                .filter_map(|v| v.split_once('='))
                .collect::<Vec<(&str, &str)>>();

            // If we detect a `link_name` query parameter, use it to change link name
            // and target a different (ex. second) keyvalue provider that is also linked to this
            // component
            if let Some((_, configured_link_name)) = query_params
                .iter()
                .find(|(k, _v)| k.to_lowercase() == "link_name")
            {
                link_name = configured_link_name;
            }
        }

        // Set the link name before performing keyvalue operations
        //
        // 99% of the time, this will be "default", but if the `link_name` parameter
        // is supplied in the path (ex. '/test?link_name=some-kv'), we can invoke other
        // keyvalue providers that are linked, by link name.
        lattice::set_link_name(
            link_name,
            vec![
                wasmcloud::bus::lattice::CallTargetInterface::new("wasi", "keyvalue", "store"),
                wasmcloud::bus::lattice::CallTargetInterface::new("wasi", "keyvalue", "atomics"),
            ],
        );

        // Note that the keyvalue-redis provider used with this example creates a single global bucket.
        // While the wasi-keyvalue interface supports multiple named buckets, the keyvalue-redis provider
        // does not, so we refer to our new bucket in the line below with an empty string.
        let bucket = wasi::keyvalue::store::open("").expect("failed to open empty bucket");
        let count = wasi::keyvalue::atomics::increment(&bucket, &object_name, 1)
            .expect("failed to increment count");

        // Build & send HTTP response
        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let response_body = response.body().unwrap();
        response_body
            .write()
            .unwrap()
            .blocking_write_and_flush(format!("Counter {object_name}: {count}\n").as_bytes())
            .unwrap();
        OutgoingBody::finish(response_body, None).expect("failed to finish response body");
        ResponseOutparam::set(response_out, Ok(response));
    }
}

export!(HttpServer);

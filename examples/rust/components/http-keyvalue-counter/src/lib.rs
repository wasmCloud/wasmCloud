use wasmcloud_component::http;
use wasmcloud_component::http::ErrorCode;
use wasmcloud_component::wasi::keyvalue::*;
use wasmcloud_component::wasmcloud::bus::lattice;

struct Component;

http::export!(Component);

impl http::Server for Component {
    fn handle(
        request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        let (parts, _) = request.into_parts();
        // Get the path of the incoming request
        let Some(path_with_query) = parts.uri.path_and_query() else {
            return http::Response::builder()
                .status(400)
                .body("Bad request, did not contain path and query".into())
                .map_err(|e| {
                    ErrorCode::InternalError(Some(format!("failed to build response {e:?}")))
                });
        };

        // At first, we can assume the object name will be the path with query
        // (ex. simple paths like '/some-key-here')
        let object_name = path_with_query.path();

        // Let's assume we want to connect to & invoke the default keyvalue provider
        // that is linked with this component
        let mut link_name = "default";

        // If query parameters were supplied, then we need to recalculate the object_name
        // and take special actions if some parameters (like `link_name`) are present (and ignore the rest)
        if let Some(query) = path_with_query.query() {
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
                lattice::CallTargetInterface::new("wasi", "keyvalue", "store"),
                lattice::CallTargetInterface::new("wasi", "keyvalue", "atomics"),
            ],
        )
        .map_err(|e| ErrorCode::InternalError(Some(format!("failed to set link name {e:?}"))))?;

        // Increment the counter in the keyvalue store
        let bucket = store::open("default")
            .map_err(|e| ErrorCode::InternalError(Some(format!("failed to open bucket {e:?}"))))?;
        let count = atomics::increment(&bucket, object_name, 1).map_err(|e| {
            ErrorCode::InternalError(Some(format!("failed to increment counter {e:?}")))
        })?;

        // Build & send HTTP response
        Ok(http::Response::new(format!(
            "Counter {object_name}: {count}\n"
        )))
    }
}

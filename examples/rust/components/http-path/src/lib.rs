use wasmcloud_component::http;

struct Component;

http::export!(Component);

impl http::Server for Component {
    fn handle(
        request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        let (parts, _) = request.into_parts();
        Ok(http::Response::new(parts.uri.path().to_string()))
    }
}

use wstd::http::{Body, Client, Request, Response, StatusCode};

const UPSTREAM_URL: &str = "https://httpbin.org/get";

#[wstd::http_server]
async fn main(_req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    let client = Client::new();
    let upstream_req = Request::get(UPSTREAM_URL).body(Body::empty())?;

    match client.send(upstream_req).await {
        Ok(upstream_resp) => {
            let (parts, mut body) = upstream_resp.into_parts();
            let bytes = body.contents().await?.to_vec();
            Ok(Response::builder()
                .status(parts.status)
                .body(Body::from(bytes))?)
        }
        Err(e) => Ok(Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(Body::from(format!("upstream request failed: {e}\n")))?),
    }
}

use wstd::http::{Body, Request, Response, StatusCode};

#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    match req.uri().path_and_query().unwrap().as_str() {
        "/" => home(req).await,
        _ => not_found(req).await,
    }
}

async fn home(_req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    // Return a simple response with a string body
    Ok(Response::new("Hello from wasmCloud!\n".into()))
}

async fn not_found(_req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    Ok(Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body("Not found\n".into())
        .unwrap())
}
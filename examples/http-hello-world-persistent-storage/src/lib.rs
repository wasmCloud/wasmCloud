use wstd::http::{Body, Request, Response, StatusCode};

wit_bindgen::generate!({
    world: "hello",
    path: "wit",
    generate_all,
});

use wasi::keyvalue::{atomics, store};

#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    match req.uri().path() {
        "/" => home(req).await,
        _ => not_found(req).await,
    }
}

async fn home(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    let query = req.uri().query().unwrap_or("");
    let name = match query.split("=").collect::<Vec<&str>>()[..] {
        ["name", name] => name,
        _ => "World",
    };

    let bucket = store::open("")
        .map_err(|e| wstd::http::Error::msg(format!("keyvalue open error: {:?}", e)))?;

    let count = atomics::increment(&bucket, name, 1)
        .map_err(|e| wstd::http::Error::msg(format!("keyvalue increment error: {:?}", e)))?;

    Ok(Response::new(format!("Hello x{count}, {name}!\n").into()))
}

async fn not_found(_req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body("Not found\n".into())
        .map_err(Into::into)
}

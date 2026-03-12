use wstd::http::{Body, Client, Request, Response, StatusCode};

#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    match req.uri().path_and_query().unwrap().as_str() {
        "/example" => fetch_example().await,
        "/wiki" => fetch_wiki().await,
        _ => not_found().await,
    }
}

async fn fetch_example() -> Result<Response<Body>, wstd::http::Error> {
    let client = Client::new();
    let req = Request::get("http://example.com")
        .body(Body::empty())
        .unwrap();
    match client.send(req).await {
        Ok(resp) => Ok(Response::builder()
            .status(resp.status())
            .body(Body::from(format!("example.com: status {}", resp.status())))
            .unwrap()),
        Err(e) => Ok(Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from(format!("example.com request failed: {e}")))
            .unwrap()),
    }
}

async fn fetch_wiki() -> Result<Response<Body>, wstd::http::Error> {
    let client = Client::new();
    let req = Request::get("https://en.wikipedia.org")
        .body(Body::empty())
        .unwrap();
    match client.send(req).await {
        Ok(resp) => Ok(Response::builder()
            .status(resp.status())
            .body(Body::from(format!(
                "en.wikipedia.org: status {}",
                resp.status()
            )))
            .unwrap()),
        Err(e) => Ok(Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from(format!("en.wikipedia.org request blocked: {e}")))
            .unwrap()),
    }
}

async fn not_found() -> Result<Response<Body>, wstd::http::Error> {
    Ok(Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("not found"))
        .unwrap())
}

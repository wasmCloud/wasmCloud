use codec::serialize;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client, RequestBuilder};
use std::collections::HashMap;
use std::convert::TryInto;
use wascc_codec::http::{Request, Response};

pub(crate) async fn request(
    client: &Client,
    req: Request,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let request = build_request(client, req)?;
    let result = request.send().await?;

    let status_code = result.status().as_u16() as u32;
    let reason = match result.status().canonical_reason() {
        Some(r) => r,
        None => "",
    };
    let headers = format_headers(result.headers());
    let resp_body = result.bytes().await?;
    let body_bytes: Vec<u8> = resp_body.into_iter().collect();

    serialize(Response {
        status_code: status_code,
        header: headers,
        status: reason.to_string(),
        body: body_bytes,
    })
}

fn format_headers(h: &HeaderMap) -> HashMap<String, String> {
    let mut headers: HashMap<String, String> = HashMap::new();
    for key in h.keys() {
        let view = h.get_all(key);
        let values: Vec<&HeaderValue> = view.iter().collect();
        let string_vals: Vec<String> = values
            .iter()
            .map(|x| match x.to_str() {
                Ok(v) => v.to_owned(),
                // This isn't great, but since we have to return a HashMap<String, String> for the
                // headers, the easiest way to handle anything that isn't a string is to just
                // base64 encode it.
                // This might be surprising behavior, but we can document it.
                Err(_) => base64::encode(x),
            })
            .collect();
        headers.insert(key.as_str().to_owned(), string_vals.join(","));
    }
    headers
}

fn build_request(
    client: &Client,
    req: Request,
) -> Result<RequestBuilder, Box<dyn std::error::Error>> {
    let path = req.path.as_str();
    let mut r = match req.method.as_str() {
        "GET" => {
            let r = client.get(format!("{}/{}", path, req.query_string).as_str());
            Ok(r)
        }
        "POST" => {
            let r = client.post(path).body(req.body);
            Ok(r)
        }
        "HEAD" => Ok(client.head(path)),
        "PUT" => Ok(client.put(path)),
        "DELETE" => Ok(client.delete(path)),
        "PATCH" => Ok(client.patch(path)),
        "OPTIONS" => Ok(client.request(reqwest::Method::OPTIONS, path)),
        "CONNECT" => Ok(client.request(reqwest::Method::CONNECT, path)),
        "TRACE" => Ok(client.request(reqwest::Method::TRACE, path)),
        m => Err(format!("{} {}", "unknown method: ", m)),
    }?;

    let headers: HeaderMap = (&req.header).try_into().expect("invalid headers");
    r = r.headers(headers);

    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::deserialize;
    use mockito::mock;
    use serde_json::json;

    #[test]
    fn test_format_headers() {
        let mut h = HeaderMap::new();
        h.insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        h.insert(reqwest::header::ETAG, "abc123".parse().unwrap());
        h.insert(
            reqwest::header::HeaderName::from_static("x-some-header"),
            reqwest::header::HeaderValue::from_bytes(b"hello\xfa").unwrap(),
        );

        let expected: HashMap<String, String> = [
            (
                reqwest::header::CONTENT_TYPE.as_str().to_owned(),
                "application/json".to_owned(),
            ),
            (
                reqwest::header::ETAG.as_str().to_owned(),
                "abc123".to_owned(),
            ),
            ("x-some-header".to_owned(), "aGVsbG/6".to_owned()),
        ]
        .iter()
        .cloned()
        .collect();
        assert_eq!(format_headers(&h), expected);
    }

    #[test]
    fn test_get_request_builder() -> Result<(), Box<dyn std::error::Error>> {
        let c = Client::new();
        let req = Request {
            method: "GET".to_string(),
            path: "http://example.com/test".to_string(),
            header: [
                (
                    reqwest::header::ACCEPT.as_str().to_string(),
                    "application/json".to_string(),
                ),
                (
                    reqwest::header::ETAG.as_str().to_string(),
                    "abc123".to_string(),
                ),
            ]
            .iter()
            .cloned()
            .collect(),
            body: vec![],
            query_string: String::new(),
        };

        let request = build_request(&c, req)?.build()?;
        assert_eq!(request.method(), reqwest::Method::GET);
        assert_eq!(request.url().query().is_none(), true);
        assert_eq!(request.headers().keys_len(), 2);
        assert_eq!(request.body().is_none(), true);

        Ok(())
    }

    #[test]
    fn test_post_request_builder() -> Result<(), Box<dyn std::error::Error>> {
        let c = Client::new();
        let body = json!({
            "test": "some_value",
        });
        let req = Request {
            method: "POST".to_string(),
            path: "http://example.com/test".to_string(),
            header: [
                (
                    reqwest::header::CONTENT_TYPE.as_str().to_string(),
                    "application/json".to_string(),
                ),
                (
                    reqwest::header::ETAG.as_str().to_string(),
                    "abc123".to_string(),
                ),
            ]
            .iter()
            .cloned()
            .collect(),
            body: serde_json::to_vec(&body)?,
            query_string: String::new(),
        };

        let request = build_request(&c, req)?.build()?;
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().query().is_none(), true);
        assert_eq!(request.headers().keys_len(), 2);
        assert_eq!(request.body().is_some(), true);
        assert_eq!(
            request.body().unwrap().as_bytes().unwrap(),
            serde_json::to_vec(&body)?.as_slice()
        );

        Ok(())
    }

    #[test]
    fn bad_request() {
        let c = Client::new();
        let req = Request {
            method: "BROKEN".to_string(),
            path: "http://example.com/test".to_string(),
            header: HashMap::new(),
            body: vec![],
            query_string: String::new(),
        };
        assert!(build_request(&c, req).is_err(), true);
    }

    #[tokio::test]
    async fn test_request() {
        let _ = env_logger::try_init();

        let c = Client::new();
        let req = Request {
            method: "GET".to_string(),
            path: mockito::server_url(),
            header: HashMap::new(),
            body: vec![],
            query_string: String::new(),
        };

        let _m = mock("GET", "/")
            .with_header("content-type", "text/plain")
            .with_body("ohai")
            .create();

        let result = request(&c, req).await.unwrap();

        let response: Response = deserialize(result.as_slice()).unwrap();
        assert_eq!(response.status_code, 200);
        assert_eq!(response.status, "OK");
        assert_eq!(response.header.get("content-type").unwrap(), "text/plain");
        assert_eq!(std::str::from_utf8(&response.body).unwrap(), "ohai");
    }
}

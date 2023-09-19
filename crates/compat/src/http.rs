use std::collections::HashMap;

use anyhow::Context;
use http::header::CONTENT_LENGTH;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};

/// Request contains data sent to actor about the http request
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServerRequest {
    /// HTTP method. One of: GET,POST,PUT,DELETE,HEAD,OPTIONS,CONNECT,PATCH,TRACE
    #[serde(default)]
    pub method: String,
    /// Full request path
    #[serde(default)]
    pub path: String,
    /// Query string
    #[serde(rename = "queryString")]
    #[serde(default)]
    pub query_string: String,
    /// Map of request headers (string key, string value)
    pub header: HashMap<String, Vec<String>>,
    /// Request body as a byte array. May be empty.
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
}

impl ServerRequest {
    pub async fn from_http(
        request: http::Request<impl AsyncRead + Unpin>,
    ) -> anyhow::Result<ServerRequest> {
        let (
            http::request::Parts {
                method,
                uri,
                headers,
                ..
            },
            mut body,
        ) = request.into_parts();
        let content_length = headers
            .get(CONTENT_LENGTH)
            .and_then(|content_length| content_length.to_str().ok())
            .and_then(|content_length| content_length.parse().ok());
        let header = headers
            .into_iter()
            .map(|(name, value)| {
                let name = name.context("name missing")?;
                let value = value
                    .to_str()
                    .with_context(|| format!("failed to parse `{name}` header value as string"))?;
                Ok((name.as_str().into(), vec![value.into()]))
            })
            .collect::<anyhow::Result<_>>()
            .context("failed to process request headers")?;
        let body = if let Some(content_length) = content_length {
            let mut buf = Vec::with_capacity(content_length);
            body.take(content_length.try_into().unwrap_or(u64::MAX))
                .read_to_end(&mut buf)
                .await
                .context("failed to read request body")?;
            buf
        } else {
            let mut buf = vec![];
            body.read_to_end(&mut buf)
                .await
                .context("failed to read request body")?;
            buf
        };
        Ok(ServerRequest {
            method: method.as_str().into(),
            path: uri.path().into(),
            query_string: uri.query().map(Into::into).unwrap_or_default(),
            header,
            body,
        })
    }
}

impl TryFrom<ServerRequest> for http::Request<Vec<u8>> {
    type Error = anyhow::Error;

    fn try_from(
        ServerRequest {
            method,
            path,
            query_string,
            header,
            body,
        }: ServerRequest,
    ) -> Result<Self, Self::Error> {
        let req = http::Request::builder().method(method.as_str());
        let req = header
            .into_iter()
            .filter_map(|(name, mut values)| {
                let value = values.pop()?;
                Some((name, value))
            })
            .fold(req, |req, (name, value)| req.header(name, value));
        match (path.as_str(), query_string.as_str()) {
            ("", "") => req,
            (_, "") => req.uri(path),
            ("", _) => req.uri(format!("?{query_string}")),
            _ => req.uri(format!("{path}?{query_string}")),
        }
        .body(body)
        .context("failed to build request")
    }
}

/// Request contains data sent to actor about the http request
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientRequest {
    /// http method, defaults to "GET"
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub url: String,
    /// optional headers. defaults to empty
    pub headers: HashMap<String, Vec<String>>,
    /// request body, defaults to empty
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
}

impl ClientRequest {
    pub async fn from_http(
        request: http::Request<impl AsyncRead + Unpin>,
    ) -> anyhow::Result<ClientRequest> {
        let (
            http::request::Parts {
                method,
                uri,
                headers,
                ..
            },
            mut body,
        ) = request.into_parts();
        let content_length = headers
            .get(CONTENT_LENGTH)
            .and_then(|content_length| content_length.to_str().ok())
            .and_then(|content_length| content_length.parse().ok());
        let headers = headers
            .into_iter()
            .map(|(name, value)| {
                let name = name.context("name missing")?;
                let value = value
                    .to_str()
                    .with_context(|| format!("failed to parse `{name}` header value as string"))?;
                Ok((name.as_str().into(), vec![value.into()]))
            })
            .collect::<anyhow::Result<_>>()
            .context("failed to process request headers")?;
        let body = if let Some(content_length) = content_length {
            let mut buf = Vec::with_capacity(content_length);
            body.take(content_length.try_into().unwrap_or(u64::MAX))
                .read_to_end(&mut buf)
                .await
                .context("failed to read request body")?;
            buf
        } else {
            let mut buf = vec![];
            body.read_to_end(&mut buf)
                .await
                .context("failed to read request body")?;
            buf
        };
        Ok(ClientRequest {
            method: method.as_str().into(),
            url: uri.to_string(),
            headers,
            body,
        })
    }
}

impl TryFrom<ClientRequest> for http::Request<Vec<u8>> {
    type Error = anyhow::Error;

    fn try_from(
        ClientRequest {
            method,
            url,
            headers,
            body,
        }: ClientRequest,
    ) -> Result<Self, Self::Error> {
        let req = http::Request::builder().method(method.as_str());
        headers
            .into_iter()
            .filter_map(|(name, mut values)| {
                let value = values.pop()?;
                Some((name, value))
            })
            .fold(req, |req, (name, value)| req.header(name, value))
            .uri(url)
            .body(body)
            .context("failed to build request")
    }
}

/// Response contains the actor's response to return to the http client
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Response {
    /// Three-digit number, usually in the range 100-599,
    /// A value of 200 indicates success.
    #[serde(rename = "statusCode")]
    #[serde(default)]
    pub status_code: u16,
    /// Map of headers (string keys, list of values)
    pub header: HashMap<String, Vec<String>>,
    /// Body of response as a byte array. May be an empty array.
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
}

impl Default for Response {
    fn default() -> Response {
        Self {
            status_code: 200,
            body: Vec::default(),
            header: HashMap::default(),
        }
    }
}

impl Response {
    pub async fn from_http(
        response: http::Response<impl AsyncRead + Unpin>,
    ) -> anyhow::Result<Response> {
        let (
            http::response::Parts {
                status, headers, ..
            },
            mut body,
        ) = response.into_parts();
        let status_code = status.as_u16();
        let content_length = headers
            .get(CONTENT_LENGTH)
            .and_then(|content_length| content_length.to_str().ok())
            .and_then(|content_length| content_length.parse().ok());
        let header = headers
            .into_iter()
            .map(|(name, value)| {
                let name = name.context("invalid header name")?;
                let value = value
                    .to_str()
                    .with_context(|| format!("failed to parse `{name}` header value as string"))?;
                Ok((name.as_str().into(), vec![value.into()]))
            })
            .collect::<anyhow::Result<_>>()
            .context("failed to process headers")?;
        let body = if let Some(content_length) = content_length {
            let mut buf = Vec::with_capacity(content_length);
            body.take(content_length.try_into().unwrap_or(u64::MAX))
                .read_to_end(&mut buf)
                .await
                .context("failed to read response body")?;
            buf
        } else {
            let mut buf = vec![];
            body.read_to_end(&mut buf)
                .await
                .context("failed to read response body")?;
            buf
        };
        Ok(Response {
            status_code,
            header,
            body,
        })
    }
}

impl TryFrom<Response> for http::Response<Vec<u8>> {
    type Error = anyhow::Error;

    fn try_from(
        Response {
            status_code,
            header,
            body,
        }: Response,
    ) -> Result<Self, Self::Error> {
        let res = http::Response::builder().status(status_code);
        let res = header
            .into_iter()
            .filter_map(|(name, mut values)| {
                let value = values.pop()?;
                Some((name, value))
            })
            .fold(res, |res, (name, value)| res.header(name, value));
        res.body(body).context("failed to construct response")
    }
}

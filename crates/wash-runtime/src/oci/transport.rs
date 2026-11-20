//! Registry transport with bounded metadata collection before OCI parsing.

use super::{Client, ClientConfig, OciDescriptor, Reference, reserve_component_bytes};
use anyhow::Result;
use futures::future::BoxFuture;
use http::{Method, header::ACCEPT};
use oci_client::transport::{Body, BoxError, EffectiveUrl, Request, Response};
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use tower_service::Service;

pub(super) const MAX_METADATA_RESPONSE_SIZE: usize = 4 * 1024 * 1024;

tokio::task_local! {
    /// Only a component-layer download may return a streaming success body.
    static STREAMING_BLOB: ();
}

pub(super) async fn pull_blob<T: AsyncWrite>(
    client: &Client,
    reference: &Reference,
    layer: &OciDescriptor,
    out: T,
) -> Result<()> {
    STREAMING_BLOB
        .scope((), client.pull_blob(reference, layer, out))
        .await?;
    Ok(())
}

#[derive(Clone)]
pub(super) struct RegistryTransport {
    client: reqwest_oci::Client,
}

impl RegistryTransport {
    pub(super) fn new(config: &ClientConfig) -> Result<Self> {
        let mut builder = reqwest_oci::Client::builder()
            .user_agent(config.user_agent)
            .danger_accept_invalid_certs(config.accept_invalid_certificates);
        if !config.tls_certs_only.is_empty() {
            builder = builder.tls_certs_only(certificates(&config.tls_certs_only)?);
        }
        builder = builder.tls_certs_merge(certificates(&config.extra_root_certificates)?);
        if let Some(timeout) = config.read_timeout {
            builder = builder.read_timeout(timeout);
        }
        if let Some(timeout) = config.connect_timeout {
            builder = builder.connect_timeout(timeout);
        }
        let no_proxy = || {
            config
                .no_proxy
                .as_deref()
                .and_then(reqwest_oci::NoProxy::from_string)
        };
        if let Some(address) = &config.https_proxy {
            builder = builder.proxy(reqwest_oci::Proxy::https(address)?.no_proxy(no_proxy()));
        }
        if let Some(address) = &config.http_proxy {
            builder = builder.proxy(reqwest_oci::Proxy::http(address)?.no_proxy(no_proxy()));
        }
        Ok(Self {
            client: builder.build()?,
        })
    }
}

fn certificates(certs: &[super::Certificate]) -> Result<Vec<reqwest_oci::Certificate>> {
    certs
        .iter()
        .map(reqwest_oci::Certificate::try_from)
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}

impl Service<Request> for RegistryTransport {
    type Response = Response;
    type Error = BoxError;
    type Future = BoxFuture<'static, Result<Response, BoxError>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        // The pinned OCI client adds Accept to blob requests, including alternate
        // URLs, but never to authentication requests. Check this inside the blob
        // operation's task-local scope so a token realm cannot masquerade as a
        // blob URL, and concurrent manifest requests remain bounded.
        let streaming_blob = STREAMING_BLOB.try_with(|()| ()).is_ok()
            && request.method() == Method::GET
            && request.headers().contains_key(ACCEPT);
        let client = self.client.clone();
        Box::pin(async move {
            let request = reqwest_oci::Request::try_from(request)?;
            let response = client.execute(request).await?;
            let effective_url = response.url().as_str().parse::<http::Uri>()?;
            let mut response = if streaming_blob && response.status().is_success() {
                http::Response::from(response).map(Body::wrap)
            } else {
                bounded_metadata(response).await?
            };
            response
                .extensions_mut()
                .insert(EffectiveUrl(effective_url));
            Ok(response)
        })
    }
}

async fn bounded_metadata(mut response: reqwest_oci::Response) -> Result<Response, BoxError> {
    let too_large = || {
        std::io::Error::other(format!(
            "registry metadata response exceeds the {MAX_METADATA_RESPONSE_SIZE}-byte limit"
        ))
    };
    if response
        .content_length()
        .is_some_and(|length| length > MAX_METADATA_RESPONSE_SIZE as u64)
    {
        return Err(too_large().into());
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if chunk.len() > MAX_METADATA_RESPONSE_SIZE - data.len() {
            return Err(too_large().into());
        }
        reserve_component_bytes(&mut data, chunk.len(), MAX_METADATA_RESPONSE_SIZE)?;
        data.extend_from_slice(&chunk);
    }
    let (parts, _) = http::Response::from(response).into_parts();
    Ok(http::Response::from_parts(parts, Body::from(data)))
}

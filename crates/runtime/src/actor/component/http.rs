use super::{Instance, InterfaceBindings, InterfaceInstance};

use crate::capability::IncomingHttp;

use std::sync::Arc;

use anyhow::{bail, Context as _};
use async_trait::async_trait;
use tokio::io::AsyncRead;
use tokio::sync::Mutex;
use tracing::instrument;

pub mod incoming_http_bindings {
    wasmtime::component::bindgen!({
        world: "incoming-http",
        async: true,
    });
}

impl Instance {
    /// Set [`IncomingHttp`] handler for this [Instance].
    pub fn incoming_http(
        &mut self,
        incoming_http: Arc<dyn IncomingHttp + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut().replace_incoming_http(incoming_http);
        self
    }

    /// Instantiates and returns a [`InterfaceInstance<incoming_http_bindings::IncomingHttp>`] if exported by the [`Instance`].
    ///
    /// # Errors
    ///
    /// Fails if incoming HTTP bindings are not exported by the [`Instance`]
    pub async fn into_incoming_http(
        mut self,
    ) -> anyhow::Result<InterfaceInstance<incoming_http_bindings::IncomingHttp>> {
        let bindings = if let Ok((bindings, _)) =
            incoming_http_bindings::IncomingHttp::instantiate_async(
                &mut self.store,
                &self.component,
                &self.linker,
            )
            .await
        {
            InterfaceBindings::Interface(bindings)
        } else {
            self.as_guest_bindings()
                .await
                .map(InterfaceBindings::Guest)
                .context("failed to instantiate `wasi:http/incoming-handler` interface")?
        };
        Ok(InterfaceInstance {
            store: Mutex::new(self.store),
            bindings,
        })
    }
}

#[async_trait]
impl IncomingHttp for InterfaceInstance<incoming_http_bindings::IncomingHttp> {
    #[allow(unused)] // TODO: Remove
    #[instrument(skip_all)]
    async fn handle(
        &self,
        request: http::Request<Box<dyn AsyncRead + Sync + Send + Unpin>>,
    ) -> anyhow::Result<http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>> {
        let (
            http::request::Parts {
                method,
                uri,
                headers,
                ..
            },
            body,
        ) = request.into_parts();
        let path_with_query = uri.path_and_query().map(http::uri::PathAndQuery::as_str);
        let scheme = uri.scheme_str();
        let authority = uri.authority().map(http::uri::Authority::as_str);
        let mut store = self.store.lock().await;
        bail!("unsupported"); // TODO: Support
    }
}

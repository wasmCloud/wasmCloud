use core::future::Future;

use async_trait::async_trait;
use tracing::instrument;

use crate::capability::messaging0_2_0::{consumer, types};
use crate::component::{Ctx, Handler};

pub mod bindings {
    wasmtime::component::bindgen!({
        world: "messaging-handler-oh-two",
        async: true,
        with: {
           "wasmcloud:messaging/types": crate::capability::messaging0_2_0::types,
        },
    });
}

/// `wasmcloud:messaging` abstraction
pub trait Messaging {
    /// Handle `wasmcloud:messaging/request`
    fn request(
        &self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> impl Future<Output = anyhow::Result<Result<types::BrokerMessage, String>>> + Send;

    /// Handle `wasmcloud:messaging/publish`
    fn publish(
        &self,
        msg: types::BrokerMessage,
    ) -> impl Future<Output = anyhow::Result<Result<(), String>>> + Send;
}

impl<H> types::Host for Ctx<H> where H: Handler {}

#[async_trait]
impl<H> consumer::Host for Ctx<H>
where
    H: Handler,
{
    #[instrument(level = "debug", skip_all)]
    async fn request(
        &mut self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> anyhow::Result<Result<types::BrokerMessage, String>> {
        self.attach_parent_context();
        Messaging::request(&self.handler, subject, body, timeout_ms).await
    }

    #[instrument(level = "debug", skip_all)]
    async fn publish(&mut self, msg: types::BrokerMessage) -> anyhow::Result<Result<(), String>> {
        self.attach_parent_context();
        self.handler.publish(msg).await
    }
}

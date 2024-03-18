use super::{Ctx, Instance, InterfaceInstance};

use crate::capability::messaging::{consumer, types};
use crate::capability::{Messaging, MessagingHandler};

use core::time::Duration;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::instrument;

pub mod messaging_handler_bindings {
    wasmtime::component::bindgen!({
        world: "messaging-handler",
        async: true,
        with: {
           "wasmcloud:messaging/types": crate::capability::messaging::types,
        },
    });
}

impl Instance {
    /// Set [`Messaging`] handler for this [Instance].
    pub fn messaging(&mut self, messaging: Arc<dyn Messaging + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_messaging(messaging);
        self
    }

    /// Instantiates and returns a [`InterfaceInstance<messaging_handler_bindings::MessagingHandler>`] if exported by the [`Instance`].
    ///
    /// # Errors
    ///
    /// Fails if messaging handler bindings are not exported by the [`Instance`]
    pub async fn into_messaging_handler(
        mut self,
    ) -> anyhow::Result<InterfaceInstance<messaging_handler_bindings::MessagingHandler>> {
        let (bindings, _) = messaging_handler_bindings::MessagingHandler::instantiate_pre(
            &mut self.store,
            &self.instance_pre,
        )
        .await?;
        Ok(InterfaceInstance {
            store: Mutex::new(self.store),
            bindings,
        })
    }
}

#[async_trait]
impl types::Host for Ctx {}

#[async_trait]
impl consumer::Host for Ctx {
    #[instrument]
    async fn request(
        &mut self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> anyhow::Result<Result<types::BrokerMessage, String>> {
        let timeout = Duration::from_millis(timeout_ms.into());
        Ok(self
            .handler
            .request(subject, body, timeout)
            .await
            .map_err(|err| format!("{err:#}")))
    }

    #[instrument(skip(self, msg))]
    async fn publish(&mut self, msg: types::BrokerMessage) -> anyhow::Result<Result<(), String>> {
        Ok(self
            .handler
            .publish(msg)
            .await
            .map_err(|err| format!("{err:#}")))
    }
}

impl MessagingHandler for InterfaceInstance<messaging_handler_bindings::MessagingHandler> {
    async fn handle_message(
        &self,
        msg: &types::BrokerMessage,
    ) -> anyhow::Result<Result<(), String>> {
        let mut store = self.store.lock().await;
        self.bindings
            .wasmcloud_messaging_handler()
            .call_handle_message(&mut *store, msg)
            .await
    }
}

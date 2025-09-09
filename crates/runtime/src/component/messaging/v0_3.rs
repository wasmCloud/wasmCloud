use core::any::Any;
use core::future::Future;

use anyhow::{bail, Context as _};
use async_trait::async_trait;
use tracing::{info_span, instrument};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use wasmtime::component::Resource;
use wasmtime::Store;

use crate::capability::messaging0_3_0::types::{Error, Metadata, Topic};
use crate::capability::messaging0_3_0::{producer, request_reply, types};
use crate::capability::wrpc;
use crate::component::{Ctx, Handler};

pub mod bindings {
    wasmtime::component::bindgen!({
        world: "messaging-handler",
        async: true,
        with: {
           "wasmcloud:messaging/types": crate::capability::messaging0_3_0::types,
        },
    });
}

#[instrument(level = "debug", skip_all)]
pub(crate) async fn handle_message<H>(
    pre: bindings::MessagingHandlerPre<Ctx<H>>,
    mut store: &mut Store<Ctx<H>>,
    msg: wrpc::wasmcloud::messaging0_2_0::types::BrokerMessage,
) -> anyhow::Result<Result<(), String>>
where
    H: Handler,
{
    let call_handle_message = info_span!("call_handle_message");
    store.data_mut().parent_context = Some(call_handle_message.context());
    let bindings = pre.instantiate_async(&mut store).await?;
    let msg = store
        .data_mut()
        .table
        .push(Message::Wrpc(msg))
        .context("failed to push message to table")?;
    bindings
        .wasmcloud_messaging0_3_0_incoming_handler()
        .call_handle(&mut store, msg)
        .await
        .context("failed to call `wasmcloud:messaging/incoming-handler@0.3.0#handle`")
        .map(|err| err.map_err(|err| err.to_string()))
}

/// Options for a request/reply operation.
#[derive(Debug, Default)]
pub struct RequestOptions {
    /// The maximum amount of time to wait for a response. If the timeout value is not set, then
    /// the request/reply operation will block until a message is received in response.
    pub timeout_ms: Option<u32>,

    /// The maximum number of replies to expect before returning.
    pub expected_replies: Option<u32>,
}

#[async_trait]
/// A message with a binary payload and additional information
pub trait HostMessage {
    /// The topic/subject/channel this message was received on
    async fn topic(&self) -> wasmtime::Result<Option<Topic>>;
    /// An optional content-type describing the format of the data in the message. This is
    /// sometimes described as the "format" type
    async fn content_type(&self) -> wasmtime::Result<Option<String>>;
    /// Set the content-type describing the format of the data in the message. This is
    /// sometimes described as the "format" type
    async fn set_content_type(&mut self, content_type: String) -> wasmtime::Result<()>;
    /// An opaque blob of data
    async fn data(&self) -> wasmtime::Result<Vec<u8>>;
    /// Set the opaque blob of data for this message, discarding the old value
    async fn set_data(&mut self, buf: Vec<u8>) -> wasmtime::Result<()>;
    /// Optional metadata (also called headers or attributes in some systems) attached to the
    /// message. This metadata is simply decoration and should not be interpreted by a host
    /// to ensure portability across different implementors (e.g., Kafka -> NATS, etc.).
    async fn metadata(&self) -> wasmtime::Result<Option<Metadata>>;
    /// Add a new key-value pair to the metadata, overwriting any existing value for the same key
    async fn add_metadata(&mut self, key: String, value: String) -> wasmtime::Result<()>;
    /// Set the metadata
    async fn set_metadata(&mut self, meta: Metadata) -> wasmtime::Result<()>;
    /// Remove a key-value pair from the metadata
    async fn remove_metadata(&mut self, key: String) -> wasmtime::Result<()>;

    /// Return [Self] as [Any]
    fn as_any(&self) -> &dyn Any;

    /// Return [Self] as [Any]
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

#[async_trait]
/// A connection to a message-exchange service (e.g., buffer, broker, etc.).
pub trait Client {
    /// Disconnect from a message-exchange service (e.g., buffer, broker, etc.).
    async fn disconnect(&mut self) -> wasmtime::Result<Result<(), Error>>;

    /// Return [Self] as [Any]
    fn as_any(&self) -> &dyn Any;
}

/// `wasmcloud:messaging` abstraction
pub trait Messaging {
    /// Establish connection to a message-exchange service (e.g., buffer, broker, etc.).
    fn connect(
        &self,
        name: String,
    ) -> impl Future<Output = wasmtime::Result<Result<Box<dyn Client + Send + Sync>, Error>>> + Send;

    /// Sends the message using the given client.
    fn send(
        &self,
        client: &(dyn Client + Send + Sync),
        topic: Topic,
        message: Message,
    ) -> impl Future<Output = wasmtime::Result<Result<(), Error>>> + Send;

    /// Performs a blocking request/reply operation with an optional set of request options.
    ///
    /// The behavior of this function is largely dependent on the options given to the function.
    /// If no options are provided, then the request/reply operation will block until a single
    /// message is received in response. If a timeout is provided, then the request/reply operation
    /// will block for the specified amount of time before returning an error if no messages were
    /// received (or the list of messages that were received). If both a timeout and an expected
    /// number of replies are provided, the function should return when either condition is met
    /// (whichever comes first)—e.g., (1) if no replies were received within the timeout return an
    /// error, (2) if the maximum expected number of replies were received before timeout, return
    /// the list of messages, or (3) if the timeout is reached before the expected number of replies,
    /// return the list of messages received up to that point.
    fn request(
        &self,
        client: &(dyn Client + Send + Sync),
        topic: Topic,
        message: &Message,
        options: Option<RequestOptions>,
    ) -> impl Future<Output = wasmtime::Result<Result<Vec<Box<dyn HostMessage + Send + Sync>>, Error>>>
           + Send;

    /// Replies to the given message with the given response message. The details of which topic
    /// the message is sent to is up to the implementation. This allows for reply-to details to be
    /// handled in the best way possible for the underlying messaging system.
    ///
    /// Please note that this reply functionality is different than something like HTTP because there
    /// are several use cases in which a reply might not be required for every message (so this would
    /// be a noop). There are also cases when you might want to reply and then continue processing.
    /// Additionally, you might want to reply to a message several times (such as providing an
    /// update). So this function is allowed to be called multiple times, unlike something like HTTP
    /// where the reply is sent and the connection is closed.
    fn reply(
        &self,
        reply_to: &Message,
        message: Message,
    ) -> impl Future<Output = wasmtime::Result<Result<(), Error>>> + Send;
}

/// A message originating from the guest
#[derive(Debug, Default)]
pub struct GuestMessage {
    /// An optional content-type describing the format of the data in the message. This is
    /// sometimes described as the "format" type
    pub content_type: Option<String>,
    /// An opaque blob of data
    pub data: Vec<u8>,
    /// Optional metadata (also called headers or attributes in some systems) attached to the
    /// message. This metadata is simply decoration and should not be interpreted by a host
    /// to ensure portability across different implementors (e.g., Kafka -> NATS, etc.).
    pub metadata: Option<Vec<(String, String)>>,
}

pub enum Message {
    Host(Box<dyn HostMessage + Send + Sync>),
    Wrpc(wrpc::wasmcloud::messaging0_2_0::types::BrokerMessage),
    Guest(GuestMessage),
}

impl<H> types::Host for Ctx<H> where H: Handler {}

impl<H> types::HostClient for Ctx<H>
where
    H: Handler,
{
    #[instrument(level = "debug", skip_all)]
    async fn connect(
        &mut self,
        name: String,
    ) -> wasmtime::Result<Result<Resource<Box<dyn Client + Send + Sync>>, Error>> {
        self.attach_parent_context();
        match self.handler.connect(name).await? {
            Ok(client) => {
                let client = self
                    .table
                    .push(client)
                    .context("failed to push client to table")?;
                Ok(Ok(client))
            }
            Err(err) => Ok(Err(err)),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn disconnect(
        &mut self,
        client: Resource<Box<dyn Client + Send + Sync>>,
    ) -> wasmtime::Result<Result<(), Error>> {
        self.attach_parent_context();
        let client = self
            .table
            .get_mut(&client)
            .context("failed to get client")?;
        client.disconnect().await
    }

    #[instrument(level = "debug", skip_all)]
    async fn drop(
        &mut self,
        client: Resource<Box<dyn Client + Send + Sync>>,
    ) -> wasmtime::Result<()> {
        self.attach_parent_context();
        self.table
            .delete(client)
            .context("failed to delete client")?;
        Ok(())
    }
}

impl<H> types::HostMessage for Ctx<H>
where
    H: Handler,
{
    #[instrument(level = "debug", skip_all)]
    async fn new(&mut self, data: Vec<u8>) -> wasmtime::Result<Resource<Message>> {
        self.attach_parent_context();
        self.table
            .push(Message::Guest(GuestMessage {
                data,
                ..Default::default()
            }))
            .context("failed to push message to table")
    }

    #[instrument(level = "debug", skip_all)]
    async fn topic(&mut self, msg: Resource<Message>) -> wasmtime::Result<Option<Topic>> {
        self.attach_parent_context();
        let msg = self.table.get(&msg).context("failed to get message")?;
        match msg {
            Message::Host(msg) => msg.topic().await,
            Message::Wrpc(msg) => Ok(Some(msg.subject.clone())),
            Message::Guest(GuestMessage { .. }) => Ok(None),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn content_type(&mut self, msg: Resource<Message>) -> wasmtime::Result<Option<String>> {
        self.attach_parent_context();
        let msg = self.table.get(&msg).context("failed to get message")?;
        match msg {
            Message::Host(msg) => msg.content_type().await,
            Message::Wrpc(..) => Ok(None),
            Message::Guest(GuestMessage { content_type, .. }) => Ok(content_type.clone()),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn set_content_type(
        &mut self,
        msg: Resource<Message>,
        content_type: String,
    ) -> wasmtime::Result<()> {
        self.attach_parent_context();
        let msg = self.table.get_mut(&msg).context("failed to get message")?;
        match msg {
            Message::Host(msg) => msg.set_content_type(content_type).await,
            Message::Wrpc(..) => bail!("content-type not currently supported by wRPC messages"),
            Message::Guest(msg) => {
                msg.content_type = Some(content_type);
                Ok(())
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn data(&mut self, msg: Resource<Message>) -> wasmtime::Result<Vec<u8>> {
        self.attach_parent_context();
        let msg = self.table.get(&msg).context("failed to get message")?;
        match msg {
            Message::Host(msg) => msg.data().await,
            Message::Wrpc(msg) => Ok(msg.body.to_vec()),
            Message::Guest(GuestMessage { data, .. }) => Ok(data.clone()),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn set_data(&mut self, msg: Resource<Message>, buf: Vec<u8>) -> wasmtime::Result<()> {
        self.attach_parent_context();
        let msg = self.table.get_mut(&msg).context("failed to get message")?;
        match msg {
            Message::Host(msg) => msg.set_data(buf).await,
            Message::Wrpc(msg) => {
                msg.body = buf.into();
                Ok(())
            }
            Message::Guest(GuestMessage { data, .. }) => {
                *data = buf;
                Ok(())
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn metadata(&mut self, msg: Resource<Message>) -> wasmtime::Result<Option<Metadata>> {
        self.attach_parent_context();
        let msg = self.table.get(&msg).context("failed to get message")?;
        match msg {
            Message::Host(msg) => msg.metadata().await,
            Message::Wrpc(..) => Ok(None),
            Message::Guest(GuestMessage { metadata, .. }) => Ok(metadata.clone()),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn add_metadata(
        &mut self,
        msg: Resource<Message>,
        key: String,
        value: String,
    ) -> wasmtime::Result<()> {
        self.attach_parent_context();
        let msg = self.table.get_mut(&msg).context("failed to get message")?;
        match msg {
            Message::Host(msg) => msg.add_metadata(key, value).await,
            Message::Wrpc(..) => bail!("metadata not currently supported by wRPC messages"),
            Message::Guest(GuestMessage {
                metadata: Some(metadata),
                ..
            }) => {
                metadata.push((key, value));
                Ok(())
            }
            Message::Guest(GuestMessage { metadata, .. }) => {
                *metadata = Some(vec![(key, value)]);
                Ok(())
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn set_metadata(
        &mut self,
        msg: Resource<Message>,
        meta: Metadata,
    ) -> wasmtime::Result<()> {
        self.attach_parent_context();
        let msg = self.table.get_mut(&msg).context("failed to get message")?;
        match msg {
            Message::Host(msg) => msg.set_metadata(meta).await,
            Message::Wrpc(..) if meta.is_empty() => Ok(()),
            Message::Wrpc(..) => bail!("metadata not currently supported by wRPC messages"),
            Message::Guest(GuestMessage { metadata, .. }) => {
                *metadata = Some(meta);
                Ok(())
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn remove_metadata(
        &mut self,
        msg: Resource<Message>,
        key: String,
    ) -> wasmtime::Result<()> {
        self.attach_parent_context();
        let msg = self.table.get_mut(&msg).context("failed to get message")?;
        match msg {
            Message::Host(msg) => msg.remove_metadata(key).await,
            Message::Guest(GuestMessage {
                metadata: Some(metadata),
                ..
            }) => {
                metadata.retain(|(k, _)| *k != key);
                Ok(())
            }
            Message::Guest(..) | Message::Wrpc(..) => Ok(()),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn drop(&mut self, rep: Resource<Message>) -> wasmtime::Result<()> {
        self.attach_parent_context();
        self.table.delete(rep).context("failed to delete message")?;
        Ok(())
    }
}

impl<H> producer::Host for Ctx<H>
where
    H: Handler,
{
    #[instrument(level = "debug", skip_all)]
    async fn send(
        &mut self,
        client: Resource<Box<dyn Client + Send + Sync>>,
        topic: Topic,
        message: Resource<Message>,
    ) -> wasmtime::Result<Result<(), Error>> {
        self.attach_parent_context();
        let message = self
            .table
            .delete(message)
            .context("failed to delete outgoing message")?;
        let client = self.table.get(&client).context("failed to get client")?;
        self.handler.send(client.as_ref(), topic, message).await
    }
}

impl<H> request_reply::Host for Ctx<H>
where
    H: Handler,
{
    async fn request(
        &mut self,
        client: Resource<Box<dyn Client + Send + Sync>>,
        topic: Topic,
        message: Resource<Message>,
        options: Option<Resource<RequestOptions>>,
    ) -> wasmtime::Result<Result<Vec<Resource<Message>>, Error>> {
        self.attach_parent_context();
        let options = options
            .map(|options| self.table.delete(options))
            .transpose()
            .context("failed to delete request options")?;
        let client = self.table.get(&client).context("failed to get client")?;
        let message = self
            .table
            .get(&message)
            .context("failed to get outgoing message")?;
        match Messaging::request(&self.handler, client.as_ref(), topic, message, options).await? {
            Ok(msgs) => {
                let msgs = msgs
                    .into_iter()
                    .map(|msg| {
                        self.table
                            .push(Message::Host(msg))
                            .context("failed to push message to table")
                    })
                    .collect::<wasmtime::Result<Vec<_>>>()?;
                Ok(Ok(msgs))
            }
            Err(err) => Ok(Err(err)),
        }
    }

    async fn reply(
        &mut self,
        reply_to: Resource<Message>,
        message: Resource<Message>,
    ) -> wasmtime::Result<Result<(), Error>> {
        self.attach_parent_context();
        let message = self
            .table
            .delete(message)
            .context("failed to delete outgoing message")?;
        let reply_to = self
            .table
            .get(&reply_to)
            .context("failed to get incoming message")?;
        self.handler.reply(reply_to, message).await
    }
}

impl<H> request_reply::HostRequestOptions for Ctx<H>
where
    H: Handler,
{
    async fn new(&mut self) -> wasmtime::Result<Resource<RequestOptions>> {
        self.attach_parent_context();
        self.table
            .push(RequestOptions::default())
            .context("failed to push request options to table")
    }

    async fn set_timeout_ms(
        &mut self,
        opts: Resource<RequestOptions>,
        timeout_ms: u32,
    ) -> wasmtime::Result<()> {
        self.attach_parent_context();
        let opts = self
            .table
            .get_mut(&opts)
            .context("failed to get request options")?;
        opts.timeout_ms = Some(timeout_ms);
        Ok(())
    }

    async fn set_expected_replies(
        &mut self,
        opts: Resource<RequestOptions>,
        expected_replies: u32,
    ) -> wasmtime::Result<()> {
        self.attach_parent_context();
        let opts = self
            .table
            .get_mut(&opts)
            .context("failed to get request options")?;
        opts.expected_replies = Some(expected_replies);
        Ok(())
    }

    async fn drop(&mut self, opts: Resource<RequestOptions>) -> wasmtime::Result<()> {
        self.attach_parent_context();
        self.table
            .delete(opts)
            .context("failed to delete request options")?;
        Ok(())
    }
}

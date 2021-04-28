use crate::hlreg::HostLocalSystemService;
use crate::messagebus::{NatsMessage, NatsSubscriber};
use actix::prelude::*;
use std::collections::HashMap;
use wascap::prelude::KeyPair;
use wasmcloud_control_interface::events::ControlEvent;

#[derive(Default)]
pub struct ControlInterface {
    client: Option<nats::asynk::Connection>,
    ns_prefix: String,
    key: Option<KeyPair>,
    options: ControlOptions,
    subscribers: HashMap<String, Addr<NatsSubscriber>>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Initialize {
    pub client: Option<nats::asynk::Connection>,
    pub control_options: ControlOptions,
    pub key: KeyPair,
    pub ns_prefix: String,
}

#[derive(Clone, Debug, Default)]
pub struct ControlOptions {
    pub oci_allow_latest: bool,
    pub oci_allowed_insecure: Vec<String>,
    pub host_labels: HashMap<String, String>,
    pub max_actors: u16,    // Currently unused
    pub max_providers: u16, // Currently unused
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct PublishEvent {
    pub event: ControlEvent,
}

impl Supervised for ControlInterface {}

impl SystemService for ControlInterface {
    fn service_started(&mut self, ctx: &mut Context<Self>) {
        info!("Control Interface started");
        ctx.set_mailbox_capacity(1000);
    }
}

impl HostLocalSystemService for ControlInterface {}

impl Actor for ControlInterface {
    type Context = Context<Self>;
}

impl Handler<PublishEvent> for ControlInterface {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: PublishEvent, _ctx: &mut Context<Self>) -> Self::Result {
        if self.client.is_none() {
            return Box::pin(async move {}.into_actor(self));
        }
        let evt = msg
            .event
            .into_published(&self.key.as_ref().unwrap().public_key());
        let prefix = Some(self.ns_prefix.to_string());
        if let Some(ref nc) = self.client {
            let nc = nc.clone();
            Box::pin(
                async move {
                    let _ = nc
                        .publish(
                            &::wasmcloud_control_interface::broker::control_event(&prefix),
                            serde_json::to_string(&evt).unwrap(),
                        )
                        .await;
                }
                .into_actor(self),
            )
        } else {
            Box::pin(async move {}.into_actor(self))
        }
    }
}

impl Handler<NatsMessage> for ControlInterface {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: NatsMessage, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Handling NATS message with subject:{}", msg.msg.subject);
        use super::handlers::*;
        use ::wasmcloud_control_interface::broker::*;

        let prefix = Some(self.ns_prefix.to_string());
        let host = self.key.as_ref().unwrap().public_key();

        let msg = msg.msg;
        let subject = msg.subject.to_string();
        let allow_latest = self.options.oci_allow_latest;
        let allowed_insecure = self.options.oci_allowed_insecure.clone();
        let nc = self.client.clone();
        Box::pin(
            async move {
                if subject == queries::host_inventory(&prefix, &host) {
                    handle_host_inventory_query(&host, &msg).await
                } else if subject == queries::link_definitions(&prefix) {
                    handle_linkdefs_query(&host, &msg).await
                } else if subject == queries::claims(&prefix) {
                    handle_claims_query(&host, &msg).await
                } else if subject == provider_auction_subject(&prefix) {
                    handle_provider_auction(&host, &msg).await
                } else if subject == actor_auction_subject(&prefix) {
                    handle_actor_auction(&host, &msg).await
                } else if subject == commands::start_actor(&prefix, &host) {
                    handle_start_actor(&host, &msg, allow_latest, &allowed_insecure).await
                } else if subject == commands::update_actor(&prefix, &host) {
                    handle_update_actor(&host, &msg, &allowed_insecure).await
                } else if subject == commands::stop_provider(&prefix, &host) {
                    handle_stop_provider(&host, &msg).await
                } else if subject == commands::start_provider(&prefix, &host) {
                    handle_start_provider(&host, &msg, allow_latest, &allowed_insecure).await
                } else if subject == commands::stop_actor(&prefix, &host) {
                    handle_stop_actor(&host, &msg).await
                } else if subject == queries::hosts(&prefix) {
                    handle_host_probe(&host, &msg).await
                }
                let _ = nc.as_ref().unwrap().flush().await;
            }
            .into_actor(self),
        )
    }
}

impl Handler<Initialize> for ControlInterface {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: Initialize, ctx: &mut Context<Self>) -> Self::Result {
        self.key = Some(msg.key);
        if msg.client.is_some() {
            info!("Initializing control interface - Active");
        } else {
            info!("Initializing control interface - Disabled");
            return Box::pin(async move {}.into_actor(self));
        }
        self.client = msg.client;
        self.options = msg.control_options;
        self.ns_prefix = msg.ns_prefix;

        use ::wasmcloud_control_interface::broker::*;

        let host_id = self.key.as_ref().unwrap().public_key();

        let prefix = Some(self.ns_prefix.to_string());

        self.subscribers.insert(
            queries::link_definitions(&prefix),
            NatsSubscriber::default().start(),
        );
        self.subscribers.insert(
            queries::host_inventory(&prefix, &host_id),
            NatsSubscriber::default().start(),
        );
        self.subscribers
            .insert(queries::claims(&prefix), NatsSubscriber::default().start());
        self.subscribers.insert(
            queries::link_definitions(&prefix),
            NatsSubscriber::default().start(),
        );
        self.subscribers.insert(
            provider_auction_subject(&prefix),
            NatsSubscriber::default().start(),
        );
        self.subscribers.insert(
            actor_auction_subject(&prefix),
            NatsSubscriber::default().start(),
        );
        self.subscribers.insert(
            commands::start_actor(&prefix, &host_id),
            NatsSubscriber::default().start(),
        );
        self.subscribers.insert(
            commands::stop_actor(&prefix, &host_id),
            NatsSubscriber::default().start(),
        );
        self.subscribers.insert(
            commands::start_provider(&prefix, &host_id),
            NatsSubscriber::default().start(),
        );
        self.subscribers.insert(
            commands::stop_provider(&prefix, &host_id),
            NatsSubscriber::default().start(),
        );
        self.subscribers.insert(
            commands::update_actor(&prefix, &host_id),
            NatsSubscriber::default().start(),
        );
        self.subscribers
            .insert(queries::hosts(&prefix), NatsSubscriber::default().start());

        let nc = self.client.as_ref().unwrap().clone();
        let subscribers = self.subscribers.clone();
        let target = ctx.address().recipient();
        Box::pin(
            async move {
                for (subject, subscriber) in subscribers.iter() {
                    let _ = subscriber
                        .send(crate::messagebus::nats_subscriber::Initialize {
                            nc: nc.clone(),
                            subject: subject.to_string(),
                            queue: None,
                            receiver: target.clone(),
                        })
                        .await;
                }
            }
            .into_actor(self),
        )
    }
}

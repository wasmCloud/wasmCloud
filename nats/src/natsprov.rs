use crate::generated::messaging::{BrokerMessage, PublishResponse, RequestArgs};
use std::error::Error;
use std::sync::Arc;
use std::sync::RwLock;
use std::{collections::HashMap, time::Duration};
use wascc_codec::capabilities::Dispatcher;

use crate::OP_DELIVER_MESSAGE;
use nats::Connection;
use wascc_codec::serialize;

const ENV_NATS_SUBSCRIPTION: &str = "SUBSCRIPTION";
const ENV_NATS_URL: &str = "URL";
const ENV_NATS_CLIENT_JWT: &str = "CLIENT_JWT";
const ENV_NATS_CLIENT_SEED: &str = "CLIENT_SEED";
const ENV_NATS_QUEUEGROUP_NAME: &str = "QUEUEGROUP_NAME";
const ENV_NATS_CREDSFILE: &str = "CREDSFILE";

pub(crate) fn publish(
    client: &Connection,
    msg: BrokerMessage,
) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
    trace!(
        "Publishing message on {} ({} bytes)",
        &msg.subject,
        &msg.body.len()
    );

    let res = if msg.reply_to.len() > 0 {
        client.publish_with_reply_or_headers(&msg.subject, Some(&msg.reply_to), None, &msg.body)
    } else {
        client.publish(&msg.subject, &msg.body)
    };

    match res {
        Ok(_) => Ok(serialize(PublishResponse { published: true })?),
        Err(e) => {
            error!("Failed to publish message: {}", e);
            Ok(serialize(PublishResponse { published: false })?)
        }
    }
}

pub(crate) fn request(
    client: &Connection,
    msg: RequestArgs,
) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
    let reply = client.request_timeout(
        &msg.subject,
        &msg.body,
        Duration::from_millis(msg.timeout as u64),
    )?;
    Ok(reply.data)
}

pub(crate) fn initialize_client(
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    actor: &str,
    values: &HashMap<String, String>,
) -> Result<Connection, Box<dyn Error + Sync + Send>> {
    let c = get_connection(values)?;

    match values.get(ENV_NATS_SUBSCRIPTION) {
        Some(ref subs) => {
            let subs: Vec<_> = subs
                .split(',')
                .map(|s| {
                    if s.is_empty() {
                        Err("Empty topic".into())
                    } else {
                        create_subscription(actor, values, dispatcher.clone(), &c, s.to_string())
                    }
                })
                .collect();
            if subs.is_empty() {
                Err("No subscriptions created".into())
            } else {
                Ok(c)
            }
        }
        None => Ok(c),
    }
}

fn create_subscription(
    actor: &str,
    values: &HashMap<String, String>,
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    client: &Connection,
    sub: String,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let actor = actor.to_string();
    let _ = match values.get(ENV_NATS_QUEUEGROUP_NAME) {
        Some(qgroup) => {
            trace!("Queue subscribing '{}' to '{}'", qgroup, sub);
            client
                .queue_subscribe(&sub, qgroup)?
                .with_handler(move |msg| {
                    let dm = delivermessage_for_natsmessage(&msg);
                    let buf = serialize(&dm).unwrap();

                    let d = dispatcher.read().unwrap();
                    if let Err(e) = d.dispatch(&actor, OP_DELIVER_MESSAGE, &buf) {
                        error!("Dispatch failed: {}", e);
                    }
                    Ok(())
                })
        }
        None => {
            trace!("Subscribing to '{}'", sub);

            client.subscribe(&sub)?.with_handler(move |msg| {
                let dm = delivermessage_for_natsmessage(&msg);
                let buf = serialize(&dm).unwrap();
                let d = dispatcher.read().unwrap();
                if let Err(e) = d.dispatch(&actor, OP_DELIVER_MESSAGE, &buf) {
                    error!("Dispatch failed: {}", e);
                }
                Ok(())
            })
        }
    };

    Ok(())
}

fn delivermessage_for_natsmessage(msg: &nats::Message) -> BrokerMessage {
    BrokerMessage {
        subject: msg.subject.clone(),
        reply_to: msg.reply.clone().unwrap_or_else(|| "".to_string()),
        body: msg.data.clone(),
    }
}

fn get_connection(
    values: &HashMap<String, String>,
) -> Result<nats::Connection, Box<dyn std::error::Error + Send + Sync>> {
    let nats_url = match values.get(ENV_NATS_URL) {
        Some(v) => v,
        None => "nats://0.0.0.0:4222",
    }
    .to_string();
    info!("NATS provider host: {}", nats_url);
    let mut opts = if let Some(creds) = get_credsfile(values) {
        nats::Options::with_credentials(creds)
    } else {
        nats::Options::new()
    };
    opts = opts.with_name("waSCC NATS Provider");
    opts.connect(&nats_url)
        .map_err(|e| format!("NATS connection failure:{}", e).into())
}

fn get_credsfile(values: &HashMap<String, String>) -> Option<String> {
    values.get(ENV_NATS_CREDSFILE).cloned()
}

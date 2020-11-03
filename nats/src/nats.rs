use natsclient::AuthenticationStyle;
use natsclient::Client;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::sync::RwLock;
use wascc_codec::capabilities::Dispatcher;
use wascc_codec::messaging::BrokerMessage;
use wascc_codec::messaging::RequestMessage;
use wascc_codec::messaging::OP_DELIVER_MESSAGE;
use wascc_codec::serialize;

const ENV_NATS_SUBSCRIPTION: &str = "SUBSCRIPTION";
const ENV_NATS_URL: &str = "URL";
const ENV_NATS_CLIENT_JWT: &str = "CLIENT_JWT";
const ENV_NATS_CLIENT_SEED: &str = "CLIENT_SEED";
const ENV_NATS_QUEUEGROUP_NAME: &str = "QUEUEGROUP_NAME";

pub(crate) fn publish(
    client: &Client,
    msg: BrokerMessage,
) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
    trace!(
        "Publishing message on {} ({} bytes)",
        &msg.subject,
        &msg.body.len()
    );
    match client.publish(
        &msg.subject,
        &msg.body,
        if !msg.reply_to.is_empty() {
            Some(&msg.reply_to)
        } else {
            None
        },
    ) {
        Ok(_) => Ok(vec![]),
        Err(e) => Err(format!("Failed to publish message: {}", e).into()),
    }
}

pub(crate) fn request(
    client: &Client,
    msg: RequestMessage,
) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
    let reply = client.request(
        &msg.subject,
        &msg.body,
        ::std::time::Duration::from_millis(msg.timeout_ms as _),
    )?;
    Ok(reply.payload)
}

pub(crate) fn initialize_client(
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    actor: &str,
    values: &HashMap<String, String>,
) -> Result<Client, Box<dyn Error + Sync + Send>> {
    let nats_url = match values.get(ENV_NATS_URL) {
        Some(v) => v,
        None => "nats://0.0.0.0:4222",
    }
    .to_string();

    let opts = natsclient::ClientOptions::builder()
        .cluster_uris(vec![nats_url.clone()])
        .authentication(determine_authentication(values)?)
        .build()
        .unwrap();
    let c = natsclient::Client::from_options(opts)?;

    c.connect()?;

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

fn determine_authentication(
    values: &HashMap<String, String>,
) -> Result<AuthenticationStyle, Box<dyn Error + Sync + Send>> {
    match values.get(ENV_NATS_CLIENT_JWT) {
        Some(client_jwt) => match values.get(ENV_NATS_CLIENT_SEED) {
            Some(client_seed) => Ok(AuthenticationStyle::UserCredentials(
                client_jwt.to_string(),
                client_seed.to_string(),
            )),
            None => Err(
                "Missing client seed, required for user credentials (JWT) authentication.".into(),
            ),
        },
        None => Ok(AuthenticationStyle::Anonymous),
    }
}

fn create_subscription(
    actor: &str,
    values: &HashMap<String, String>,
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    client: &Client,
    sub: String,
) -> Result<(), Box<dyn Error + Sync + Send>> {
    let actor = actor.to_string();
    let res = match values.get(ENV_NATS_QUEUEGROUP_NAME) {
        Some(ref qgroup) => {
            trace!("Queue subscribing '{}' to '{}'", qgroup, sub);
            client.queue_subscribe(&sub, qgroup, move |msg| {
                let dm = delivermessage_for_natsmessage(msg);
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

            client.subscribe(&sub, move |msg| {
                let dm = delivermessage_for_natsmessage(msg);
                let buf = serialize(&dm).unwrap();
                let d = dispatcher.read().unwrap();
                if let Err(e) = d.dispatch(&actor, OP_DELIVER_MESSAGE, &buf) {
                    error!("Dispatch failed: {}", e);
                }
                Ok(())
            })
        }
    };

    match res {
        Ok(_) => Ok(()),
        Err(e) => Err(Box::new(e)),
    }
}

fn delivermessage_for_natsmessage(msg: &natsclient::Message) -> BrokerMessage {
    BrokerMessage {
        subject: msg.subject.clone(),
        reply_to: msg.reply_to.clone().unwrap_or_else(|| "".to_string()),
        body: msg.payload.clone(),
    }
}

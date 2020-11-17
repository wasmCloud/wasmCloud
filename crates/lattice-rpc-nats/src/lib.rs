use actix_rt::{spawn, Arbiter};
use crossbeam_channel::{Receiver, Sender};
use futures::StreamExt;
use nats::subscription::Handler;
use rmp_serde::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::error::Error;
use std::io::Cursor;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use wascap::jwt::Claims;
use wascc_host::{
    BusDispatcher, Invocation, InvocationResponse, LatticeProvider, Result, WasccEntity,
};

#[macro_use]
extern crate log;

#[macro_use]
extern crate crossbeam_channel;

#[macro_use]
extern crate async_trait;

pub struct NatsLatticeProvider {
    ns_prefix: Option<String>,
    dispatcher: Option<BusDispatcher>,
    nc: nats::asynk::Connection,
    rpc_timeout: Duration,
    handlers: HashMap<String, Arc<nats::asynk::Subscription>>,
}

struct RpcCall {
    subject: String,
    inv: Invocation,
    timeout: Duration,
}

struct Term;

impl NatsLatticeProvider {
    pub fn new(
        ns_prefix: Option<String>,
        rpc_timeout: Duration,
        nc: nats::asynk::Connection,
    ) -> NatsLatticeProvider {
        NatsLatticeProvider {
            ns_prefix,
            dispatcher: None,
            nc,
            rpc_timeout,
            handlers: HashMap::new(),
        }
    }
}

impl NatsLatticeProvider {
    /// Produce the rpc prefix used by all RPC functions. The lattice namespace
    /// is used as a further tier of separation in the subject space
    fn subject_prefix(&self) -> String {
        format!(
            "wasmbus.rpc.{}",
            self.ns_prefix.as_ref().unwrap_or(&"default".to_string())
        )
    }

    fn invoke_subject(&self, entity: &WasccEntity) -> String {
        let prefix = self.subject_prefix();
        match entity {
            WasccEntity::Actor(s) => format!("{}.{}", prefix, s),
            WasccEntity::Capability { id, binding, .. } => format!("{}.{}.{}", prefix, id, binding),
        }
    }

    fn links_subject(&self) -> String {
        let prefix = self.subject_prefix();
        format!("wasmbus.{}.links", prefix)
    }

    fn claims_subject(&self) -> String {
        let prefix = self.subject_prefix();
        format!("wasmbus.{}.claims", prefix)
    }

    // All hosts should receive link advertisements, so this is not a queue subscribe
    fn handle_links_advertisements(&self) -> Result<()> {
        /*let d = self.dispatcher.clone().unwrap();
        self.nc
            .subscribe(&self.links_subject())?
            .with_handler(move |msg| {
                let ld: LinkDefinition = match deserialize(&msg.data) {
                    Ok(ld) => ld,
                    Err(_) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            "Deserialization failure",
                        ))
                    }
                };
                d.notify_binding_update(
                    &ld.actor,
                    &ld.contract_id,
                    &ld.provider_id,
                    &ld.link_name,
                    ld.values,
                );
                Ok(())
            }); */
        Ok(())
    }

    // All hosts should receive claims advertisements, so this is not a queue subscribe
    fn handle_claims_advertisements(&self) -> Result<()> {
        let d = self.dispatcher.clone().unwrap();
        /* self.nc
        .subscribe(&self.claims_subject())?
        .with_handler(move |msg| {
            let c: Claims<wascap::jwt::Actor> = match deserialize(&msg.data) {
                Ok(c) => c,
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "Deserialization failure",
                    ))
                }
            };
            d.notify_claims_received(c);
            Ok(())
        }); */
        Ok(())
    }
}

#[async_trait]
impl LatticeProvider for NatsLatticeProvider {
    fn init(&mut self, dispatcher: BusDispatcher) {
        self.dispatcher = Some(dispatcher);
        if let Err(e) = self.handle_links_advertisements() {
            error!("Failed to subscribe to link advertisements: {}", e);
        }
        if let Err(e) = self.handle_claims_advertisements() {
            error!("Failed to subscribe to claims advertisements: {}", e);
        }
    }

    fn name(&self) -> String {
        "NATS".to_string()
    }

    async fn rpc(&self, inv: &Invocation) -> Result<InvocationResponse> {
        let bytes = serialize(&inv)?;
        let subject = self.invoke_subject(&inv.target);
        let timeout = self.rpc_timeout.clone();
        //let inv = inv.clone();
        //let res = self.nc.request_timeout(&subject, &bytes, timeout);
        match actix_rt::time::timeout(timeout, self.nc.request(&subject, &bytes)).await? {
            Err(_) => {
                println!("Nats timeout");
                Err("NaTS timeout".into())
            }
            Ok(res) => {
                println!("nc.request(subject, msg) resulted in {:?}", res);
                let ir: Result<InvocationResponse> = deserialize(&res.data);
                ir
            }
        }

        /*        match res {
            Ok(r) => {
                let ir: Result<InvocationResponse> = deserialize(&r.data);
                ir
            }
            Err(e) => {
                println!("Nats timeout");
                Err("NaTS timeout".into())
            }
        } */
    }

    async fn register_rpc_listener(&mut self, subscriber: &WasccEntity) -> Result<()> {
        let subject = self.invoke_subject(&subscriber);
        let s = subject.clone();
        let d = self.dispatcher.clone().unwrap();
        let nc = self.nc.clone();

        /*let mut sub = Arc::new(nc.queue_subscribe(&subject, &subject).await?);
        self.handlers.insert(s.to_string(), sub);
        Arbiter::spawn(async move {
            while let Some(msg) = sub.next().await {
                let inv: Invocation = match deserialize(&msg.data) {
                    Ok(i) => i,
                    Err(_) => return Err("Deserialization failure".into()),
                };
                let res = d.invoke(&inv);
                let _ = msg.respond(&serialize(res).unwrap()).await;
            };
        }); */
        Ok(())

        /* let handler = self
            .nc
            .queue_subscribe(&subject, &subject)?
            .with_handler(move |msg| {
                println!("Received inbound RPC");
                let inv: Invocation = match deserialize(&msg.data) {
                    Ok(i) => i,
                    Err(_) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            "Deserialization failure",
                        ))
                    }
                };
                println!("Deserialized invocation {:?}", inv);
                let res = d.invoke(&inv);
                println!("Got a response: {:?}", res);
                if let Some(ref s) = msg.reply {
                    println!("REPLY: {}", s);
                }
                if let Ok(r) = serialize(res) {
                    println!("RESPONDING");
                    nc.publish(&msg.reply.unwrap(), &r).unwrap();
                    //msg.respond(&r)?;
                    //nc.drain();
                } else {
                    println!("Failed to serialize invocation response");
                }

                nc.flush()?;
                Ok(())
            });

        self.handlers.insert(s, handler);*/
    }

    fn remove_rpc_listener(&mut self, subscriber: &WasccEntity) -> Result<()> {
        let subject = self.invoke_subject(subscriber);
        if let Some(h) = self.handlers.remove(&subject) {
            let _ = h.unsubscribe();
        }
        Ok(())
    }

    // Linking advertisements take place on wasmbus.{prefix}.links
    fn advertise_link(
        &self,
        actor: &str,
        contract_id: &str,
        link_name: &str,
        provider_id: &str,
        values: HashMap<String, String>,
    ) -> Result<()> {
        let ld = LinkDefinition {
            actor: actor.to_string(),
            contract_id: contract_id.to_string(),
            link_name: link_name.to_string(),
            provider_id: provider_id.to_string(),
            values,
        };
        println!("Advertised link!! {:?}", ld);
        /* self.nc
        .publish(&self.links_subject(), &serialize(&ld).unwrap())?; */
        Ok(())
    }

    // Claims advertisements take place on wasmbus.{prefix}.claims
    fn advertise_claims(&self, claims: Claims<wascap::jwt::Actor>) -> Result<()> {
        //self.nc
        //            .publish(&self.claims_subject(), &serialize(&claims).unwrap())?;
        Ok(())
    }
}

/// The standard function for serializing codec structs into a format that can be
/// used for message exchange between actor and host. Use of any other function to
/// serialize could result in breaking incompatibilities.
pub fn serialize<T>(
    item: T,
) -> ::std::result::Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>
where
    T: Serialize,
{
    let mut buf = Vec::new();
    item.serialize(&mut Serializer::new(&mut buf).with_struct_map())?;
    Ok(buf)
}

/// The standard function for de-serializing codec structs from a format suitable
/// for message exchange between actor and host. Use of any other function to
/// deserialize could result in breaking incompatibilities.
pub fn deserialize<'de, T: Deserialize<'de>>(
    buf: &[u8],
) -> ::std::result::Result<T, Box<dyn std::error::Error + Send + Sync>> {
    let mut de = Deserializer::new(Cursor::new(buf));
    match Deserialize::deserialize(&mut de) {
        Ok(t) => Ok(t),
        Err(e) => Err(format!("Failed to de-serialize: {}", e).into()),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct LinkDefinition {
    actor: String,
    contract_id: String,
    link_name: String,
    provider_id: String,
    values: HashMap<String, String>,
}

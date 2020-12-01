use crate::hlreg::HostLocalSystemService;
use crate::host_controller::{
    HostController, QueryActorRunning, QueryHostInventory, QueryProviderRunning, QueryUptime,
    StartActor, StartProvider, StopActor, StopProvider,
};
use crate::messagebus::{GetClaims, MessageBus, QueryAllLinks};
use crate::{Actor, NativeCapability};
use control_interface::{
    deserialize, serialize, ActorDescription, HostInventory, LinkDefinition, ProviderDescription,
    StopActorAck, StopActorCommand, StopProviderAck, StopProviderCommand,
};
use control_interface::{StartActorAck, StartActorCommand, StartProviderAck, StartProviderCommand};
use std::collections::HashMap;
use wascap::jwt::Claims;

// TODO: implement actor update
pub(crate) async fn handle_update_actor(host: &str, msg: &nats::asynk::Message) {}
// TODO: implement provider auction
pub(crate) async fn handle_provider_auction(_host: &str, _msg: &nats::asynk::Message) {}
// TODO: implement actor auction
pub(crate) async fn handle_actor_auction(_host: &str, _msg: &nats::asynk::Message) {}

pub(crate) async fn handle_host_inventory_query(host: &str, msg: &nats::asynk::Message) {
    let hc = HostController::from_hostlocal_registry(host);
    let res = hc.send(QueryHostInventory {}).await;
    let mut inv = HostInventory {
        providers: vec![],
        actors: vec![],
        labels: HashMap::new(),
        host_id: host.to_string(),
    };
    match res {
        Ok(hi) => {
            inv.providers = hi
                .providers
                .iter()
                .map(|ps| ProviderDescription {
                    id: ps.id.to_string(),
                    link_name: ps.link_name.to_string(),
                    image_ref: ps.image_ref.clone(),
                })
                .collect();
            inv.actors = hi
                .actors
                .iter()
                .map(|a| ActorDescription {
                    id: a.id.to_string(),
                    image_ref: a.image_ref.clone(),
                })
                .collect();
            inv.labels = hi.labels.clone();
        }
        Err(_) => {
            error!("Mailbox failure querying host controller for inventory");
        }
    }
    let _ = msg.respond(&serialize(inv).unwrap()).await;
}

pub(crate) async fn handle_linkdefs_query(host: &str, msg: &nats::asynk::Message) {
    let mb = MessageBus::from_hostlocal_registry(host);
    match mb.send(QueryAllLinks {}).await {
        Ok(links) => {
            let linkres = ::control_interface::LinkDefinitionList {
                links: links
                    .links
                    .into_iter()
                    .map(|l| ::control_interface::LinkDefinition {
                        actor_id: l.actor_id,
                        provider_id: l.provider_id,
                        link_name: l.link_name,
                        contract_id: l.contract_id,
                        values: l.values,
                    })
                    .collect(),
            };
            let _ = msg.respond(&serialize(&linkres).unwrap()).await;
        }
        Err(_) => {
            error!("Messagebus mailbox failure querying link definitions");
        }
    }
}

pub(crate) async fn handle_claims_query(host: &str, msg: &nats::asynk::Message) {
    let mb = MessageBus::from_hostlocal_registry(host);
    match mb.send(GetClaims {}).await {
        Ok(claims) => {
            let cs = claims.claims.values().map(|c| claims_to_if(c)).collect();
            let cl = ::control_interface::ClaimsList { claims: cs };
            let _ = msg.respond(&serialize(cl).unwrap()).await;
        }
        Err(_) => {
            error!("Messagebus mailbox failure querying claims");
        }
    }
}

pub(crate) async fn handle_host_probe(host: &str, msg: &nats::asynk::Message) {
    let hc = HostController::from_hostlocal_registry(host);
    let res = hc.send(QueryUptime {}).await;
    let mut probe_ack = ::control_interface::Host {
        id: host.to_string(),
        uptime_seconds: 0,
    };
    match res {
        Ok(up) => {
            probe_ack.uptime_seconds = up;
        }
        Err(_) => {
            error!("Failed to query uptime from host controller");
        }
    }
    let _ = msg.respond(&serialize(probe_ack).unwrap()).await;
}

// TODO: I don't know if this function reads better as a chain of `and_then` futures or
// if this "go" style guard check sequence is easier to read.
pub(crate) async fn handle_start_actor(host: &str, msg: &nats::asynk::Message, allow_latest: bool) {
    let cmd = deserialize::<StartActorCommand>(&msg.data);
    let mut ack = StartActorAck::default();
    ack.host_id = host.to_string();

    if let Err(e) = cmd {
        let f = format!("Bad StartActor command received: {}", e);
        error!("{}", f);
        ack.failure = Some(f);
        let _ = msg.respond(&serialize(ack).unwrap()).await;
        return;
    }
    let cmd = cmd.unwrap();
    ack.actor_ref = cmd.actor_ref.to_string();

    let hc = HostController::from_hostlocal_registry(host);
    let res = hc
        .send(QueryActorRunning {
            actor_ref: cmd.actor_ref.to_string(),
        })
        .await;
    match res {
        Ok(running) => {
            if running {
                let f = format!(
                    "Actor with image ref '{}' is already running on this host",
                    cmd.actor_ref
                );
                error!("{}", f);
                ack.failure = Some(f);
                let _ = msg.respond(&serialize(ack).unwrap()).await;
                return;
            }
        }
        Err(_) => {
            let f = "Failed to query host controller for running actor".to_string();
            error!("{}", f);
            ack.failure = Some(f);
            let _ = msg.respond(&serialize(ack).unwrap()).await;
            return;
        }
    }

    let bytes = crate::oci::fetch_oci_bytes(&cmd.actor_ref, allow_latest).await;
    if let Err(e) = bytes {
        let f = format!("Failed to retrieve actor image from OCI registry: {}", e);
        error!("{}", f);
        ack.failure = Some(f);
        let _ = msg.respond(&serialize(ack).unwrap()).await;
        return;
    }
    let bytes = bytes.unwrap();

    let actor = Actor::from_slice(&bytes);
    if let Err(e) = actor {
        let f = format!(
            "Could not create actor from retrieved OCI image bytes: {}",
            e
        );
        error!("{}", f);
        ack.failure = Some(f);
        let _ = msg.respond(&serialize(ack).unwrap()).await;
        return;
    }
    let actor = actor.unwrap();

    let actor_id = actor.public_key();

    let r = hc
        .send(StartActor {
            actor,
            image_ref: Some(cmd.actor_ref.to_string()),
        })
        .await;
    if let Err(_e) = r {
        let f = "Host controller did not acknowledge start actor message".to_string();
        error!("{}", f);
        ack.failure = Some(f);
        let _ = msg.respond(&serialize(ack).unwrap()).await;
        return;
    }
    // Acknowledge the message
    ack.actor_ref = cmd.actor_ref;
    ack.actor_id = actor_id;
    ack.failure = None;

    let _ = msg.respond(&serialize(ack).unwrap()).await;
}

pub(crate) async fn handle_stop_provider(host: &str, msg: &nats::asynk::Message) {
    let mut ack = StopProviderAck::default();
    let hc = HostController::from_hostlocal_registry(host);

    let cmd = match deserialize::<StopProviderCommand>(&msg.data) {
        Ok(c) => c,
        Err(_) => {
            let f = "Failed to deserialize stop provider command";
            error!("{}", f);
            ack.failure = Some(f.to_string());
            let _ = msg.respond(&serialize(ack).unwrap()).await;
            return;
        }
    };
    match hc
        .send(QueryProviderRunning {
            provider_ref: cmd.provider_ref.to_string(),
            link_name: cmd.link_name.to_string(),
        })
        .await
    {
        Ok(r) if !r => {
            let f = format!(
                "Provider {}/{} is not running on this host",
                cmd.provider_ref, cmd.link_name
            );
            error!("{}", f);
            ack.failure = Some(f);
            let _ = msg.respond(&serialize(ack).unwrap()).await;
            return;
        }
        Ok(_) => {} // Running
        _ => {
            let f = "Host controller unavailable";
            error!("{}", f);
            ack.failure = Some(f.to_string());
            let _ = msg.respond(&serialize(ack).unwrap()).await;
            return;
        }
    }

    if let Err(_) = hc
        .send(StopProvider {
            provider_ref: cmd.provider_ref,
            binding: cmd.link_name,
            contract_id: cmd.contract_id,
        })
        .await
    {
        let f = "Host controller unavailable to stop provider";
        error!("{}", f);
        ack.failure = Some(f.to_string());
        let _ = msg.respond(&serialize(ack).unwrap()).await;
        return;
    }

    let _ = msg.respond(&serialize(ack).unwrap()).await;
}

pub(crate) async fn handle_start_provider(
    host: &str,
    msg: &nats::asynk::Message,
    allow_latest: bool,
) {
    let mut ack = StartProviderAck::default();
    ack.host_id = host.to_string();

    let cmd = deserialize::<StartProviderCommand>(&msg.data);
    if let Err(e) = cmd {
        let f = format!("Bad StartProvider command received: {}", e);
        error!("{}", f);
        ack.failure = Some(f);
        let _ = msg.respond(&serialize(ack).unwrap()).await;
        return;
    }
    let cmd = cmd.unwrap();
    let hc = HostController::from_hostlocal_registry(host);

    let res = hc
        .send(QueryProviderRunning {
            provider_ref: cmd.provider_ref.to_string(),
            link_name: cmd.link_name.to_string(),
        })
        .await;
    match res {
        Ok(running) => {
            if running {
                let f = format!(
                    "Provider with image ref '{}' is already running on this host.",
                    cmd.provider_ref
                );
                error!("{}", f);
                ack.failure = Some(f);
                let _ = msg.respond(&serialize(ack).unwrap()).await;
                return;
            }
        }
        Err(_) => {
            let f = "Failed to query host controller for running providers".to_string();
            error!("{}", f);
            ack.failure = Some(f);
            let _ = msg.respond(&serialize(ack).unwrap()).await;
            return;
        }
    }

    let par = crate::oci::fetch_provider_archive(&cmd.provider_ref, allow_latest).await;
    if let Err(e) = par {
        let f = format!(
            "Failed to retrieve provider archive from OCI registry: {}",
            e
        );
        error!("{}", f);
        ack.failure = Some(f);
        let _ = msg.respond(&serialize(ack).unwrap()).await;
        return;
    }
    let par = par.unwrap();

    let cap = NativeCapability::from_archive(&par, Some(cmd.link_name.to_string()));
    if let Err(e) = cap {
        let f = format!(
            "Failed to create provider archive from OCI image bytes: {}",
            e
        );
        error!("{}", f);
        ack.failure = Some(f);
        let _ = msg.respond(&serialize(ack).unwrap()).await;
        return;
    }
    let cap = cap.unwrap();
    let provider_id = cap.id();

    let r = hc
        .send(StartProvider {
            provider: cap,
            image_ref: Some(cmd.provider_ref.to_string()),
        })
        .await;
    if let Err(_e) = r {
        let f = "Host controller failed to acknowledge start provider command".to_string();
        error!("{}", f);
        let _ = msg.respond(&serialize(ack).unwrap()).await;
        return;
    }

    ack.provider_ref = cmd.provider_ref;
    ack.provider_id = provider_id;

    // Acknowledge the command
    let _ = msg.respond(&serialize(ack).unwrap()).await;
}

pub(crate) async fn handle_stop_actor(host: &str, msg: &nats::asynk::Message) {
    let mut ack = StopActorAck::default();
    let hc = HostController::from_hostlocal_registry(host);

    let cmd = match deserialize::<StopActorCommand>(&msg.data) {
        Ok(c) => c,
        Err(_) => {
            error!("Failed to deserialize stop actor command");
            ack.failure = Some("Failed to deserialize stop actor command".to_string());
            let _ = msg.respond(&serialize(ack).unwrap()).await;
            return;
        }
    };

    match hc
        .send(QueryActorRunning {
            actor_ref: cmd.actor_ref.to_string(),
        })
        .await
    {
        Ok(r) if r => {}
        _ => {
            let f = "Actor is either not running on this host or host controller unresponsive";
            error!("{}", f);
            ack.failure = Some(f.to_string());
            let _ = msg.respond(&serialize(ack).unwrap()).await;
            return;
        }
    };

    if let Err(_) = hc
        .send(StopActor {
            actor_ref: cmd.actor_ref,
        })
        .await
    {
        let f = "Host controller did not acknowledge stop command";
        error!("{}", f);
        ack.failure = Some(f.to_string());
        let _ = msg.respond(&serialize(ack).unwrap()).await;
        return;
    }

    let _ = msg.respond(&serialize(ack).unwrap()).await;
}

fn claims_to_if(c: &Claims<wascap::jwt::Actor>) -> ::control_interface::Claims {
    let mut hm = HashMap::new();
    hm.insert("sub".to_string(), c.subject.to_string());
    hm.insert("iss".to_string(), c.issuer.to_string());
    if let Some(ref md) = c.metadata {
        hm.insert(
            "caps".to_string(),
            md.caps
                .as_ref()
                .map(|v| v.join(","))
                .unwrap_or("".to_string()),
        );
        hm.insert("rev".to_string(), md.rev.unwrap_or(0).to_string());
        hm.insert(
            "version".to_string(),
            md.ver.as_ref().unwrap_or(&"".to_string()).to_string(),
        );
    }
    ::control_interface::Claims { values: hm }
}

use crate::actors::LiveUpdate;

use crate::hlreg::HostLocalSystemService;
use crate::host_controller::{
    AuctionActor, AuctionProvider, GetRunningActor, HostController, QueryActorRunning,
    QueryHostInventory, QueryProviderRunning, QueryUptime, StartActor, StartProvider, StopActor,
    StopProvider,
};
use crate::messagebus::{GetClaims, MessageBus, QueryAllLinks};
use crate::oci::fetch_oci_bytes;
use crate::{Actor, NativeCapability};

use wasmcloud_control_interface::{
    deserialize, serialize, ActorAuctionAck, ActorAuctionRequest, ActorDescription, HostInventory,
    ProviderAuctionAck, ProviderAuctionRequest, ProviderDescription, StopActorAck,
    StopActorCommand, StopProviderAck, StopProviderCommand, UpdateActorAck, UpdateActorCommand,
};
use wasmcloud_control_interface::{
    StartActorAck, StartActorCommand, StartProviderAck, StartProviderCommand,
};

use std::collections::HashMap;
use wascap::jwt::Claims;

// *** NOTE ***
// It is extremely important to note that this function will acknowledge the -acceptance-
// of the actor update as soon as it has verified that the update process can begin.
// In other words, acceptance of this command constitutes the following:
// * the command de-serialized properly
// * the actor mentioned in that command is running within the host that received the command
// Because live updating an actor involves downloading the OCI bytes and then reconstituting a
// low-level wasm runtime host (which could involve a JIT pass depending on the runtime), we
// cannot allow control interface clients to wait that long for acknowledgement.
pub(crate) async fn handle_update_actor(
    host: &str,
    msg: &nats::asynk::Message,
    allowed_insecure: &[String],
) {
    let hc = HostController::from_hostlocal_registry(host);
    let req = deserialize::<UpdateActorCommand>(&msg.data);
    if req.is_err() {
        error!("Failed to deserialize actor start command");
        return;
    }
    let req = req.unwrap();
    let actor = hc
        .send(GetRunningActor {
            actor_id: req.actor_id.to_string(),
        })
        .await;
    let mut ack = UpdateActorAck { accepted: false };
    match actor {
        Ok(a) => {
            if let Some(a) = a {
                ack.accepted = true;
                let _ = msg.respond(&serialize(ack).unwrap()).await;
                let bytes = fetch_oci_bytes(&req.new_actor_ref, false, allowed_insecure).await;
                match bytes {
                    Ok(v) => {
                        if let Err(e) = a
                            .send(LiveUpdate {
                                actor_bytes: v,
                                image_ref: req.new_actor_ref,
                            })
                            .await
                        {
                            error!("Failed to perform actor update: {}", e);
                        }
                    }
                    Err(_e) => {
                        error!(
                            "Failed to obtain actor image '{}' from OCI registry",
                            req.new_actor_ref
                        );
                    }
                }
            } else {
                error!("Target actor for a live update is not running on this host");
                let _ = msg.respond(&serialize(ack).unwrap()).await;
            }
        }
        Err(_) => {
            error!("Failed to obtain running actor from host controller (mailbox error)");
            let _ = msg.respond(&serialize(ack).unwrap()).await;
        }
    }
}

pub(crate) async fn handle_provider_auction(host: &str, msg: &nats::asynk::Message) {
    let hc = HostController::from_hostlocal_registry(host);
    let req = deserialize::<ProviderAuctionRequest>(&msg.data);
    if req.is_err() {
        error!("Failed to deserialize provider auction request");
        return;
    }
    let req = req.unwrap();
    match hc
        .send(AuctionProvider {
            constraints: req.constraints.clone(),
            provider_ref: req.provider_ref.to_string(),
            link_name: req.link_name.to_string(),
        })
        .await
    {
        Ok(r) if r => {
            let ack = ProviderAuctionAck {
                provider_ref: req.provider_ref.to_string(),
                link_name: req.link_name.to_string(),
                host_id: host.to_string(),
            };
            let _ = msg.respond(&serialize(ack).unwrap()).await;
        }
        _ => {
            trace!("Auction provider request denied");
        }
    }
}

pub(crate) async fn handle_actor_auction(host: &str, msg: &nats::asynk::Message) {
    let hc = HostController::from_hostlocal_registry(host);
    let req = deserialize::<ActorAuctionRequest>(&msg.data);
    if req.is_err() {
        error!("Failed to deserialize actor auction request");
        return;
    }
    let req = req.unwrap();
    match hc
        .send(AuctionActor {
            constraints: req.constraints.clone(),
            actor_ref: req.actor_ref.to_string(),
        })
        .await
    {
        Ok(r) if r => {
            let ack = ActorAuctionAck {
                actor_ref: req.actor_ref,
                constraints: req.constraints,
                host_id: host.to_string(),
            };
            let _ = msg.respond(&serialize(ack).unwrap()).await;
        }
        _ => {
            trace!("Auction actor request denied");
        }
    }
}

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
            inv.labels = hi.labels;
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
            let linkres = ::wasmcloud_control_interface::LinkDefinitionList {
                links: links
                    .links
                    .into_iter()
                    .map(|l| ::wasmcloud_control_interface::LinkDefinition {
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
            let cl = ::wasmcloud_control_interface::ClaimsList { claims: cs };
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
    let mut probe_ack = ::wasmcloud_control_interface::Host {
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
pub(crate) async fn handle_start_actor(
    host: &str,
    msg: &nats::asynk::Message,
    allow_latest: bool,
    allowed_insecure: &[String],
) {
    let mut ack = StartActorAck {
        host_id: host.to_string(),
        ..Default::default()
    };

    let cmd = deserialize::<StartActorCommand>(&msg.data);
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
        Ok(running) if running => {
            let f = format!(
                "Actor with image ref '{}' is already running on this host",
                cmd.actor_ref
            );
            warn!("{}", f);
            ack.failure = Some(f);
            let _ = msg.respond(&serialize(ack).unwrap()).await;
            return;
        }
        Ok(_) => {
            let _ = msg.respond(&serialize(ack).unwrap()).await;
        } // all good to start
        Err(_) => {
            let f = "Failed to query host controller for running actor".to_string();
            error!("{}", f);
            ack.failure = Some(f);
            let _ = msg.respond(&serialize(ack).unwrap()).await;
            return;
        }
    }

    let bytes = crate::oci::fetch_oci_bytes(&cmd.actor_ref, allow_latest, allowed_insecure).await;
    if let Err(e) = bytes {
        let f = format!("Failed to retrieve actor image from OCI registry: {}", e);
        error!("{}", f);
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
        return;
    }
    let actor = actor.unwrap();

    let r = hc
        .send(StartActor {
            actor,
            image_ref: Some(cmd.actor_ref.to_string()),
        })
        .await;
    if let Err(_e) = r {
        let f = "Host controller did not acknowledge start actor message".to_string();
        error!("{}", f);
        return;
    }
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
            warn!("{}", f);
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

    if hc
        .send(StopProvider {
            provider_ref: cmd.provider_ref,
            link_name: cmd.link_name,
            contract_id: cmd.contract_id,
        })
        .await
        .is_err()
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
    allowed_insecure: &[String],
) {
    let mut ack = StartProviderAck {
        host_id: host.to_string(),
        ..Default::default()
    };

    let cmd = deserialize::<StartProviderCommand>(&msg.data);
    if let Err(e) = cmd {
        let f = format!("Bad StartProvider command received: {}", e);
        error!("{}", f);
        ack.failure = Some(f);
        let _ = msg.respond(&serialize(ack).unwrap()).await;
        return;
    }
    let cmd = cmd.unwrap();
    ack.provider_ref = cmd.provider_ref.to_string();

    let hc = HostController::from_hostlocal_registry(host);

    let res = hc
        .send(QueryProviderRunning {
            provider_ref: cmd.provider_ref.to_string(),
            link_name: cmd.link_name.to_string(),
        })
        .await;
    match res {
        Ok(running) if running => {
            let f = format!(
                "Provider with image ref '{}' is already running on this host.",
                cmd.provider_ref
            );
            warn!("{}", f);
            ack.failure = Some(f);
            let _ = msg.respond(&serialize(ack).unwrap()).await;
            return;
        }
        Ok(_) => {
            let _ = msg.respond(&serialize(ack).unwrap()).await;
        }
        Err(_) => {
            let f = "Failed to query host controller for running providers".to_string();
            error!("{}", f);
            ack.failure = Some(f);
            let _ = msg.respond(&serialize(ack).unwrap()).await;
            return;
        }
    }

    let par =
        crate::oci::fetch_provider_archive(&cmd.provider_ref, allow_latest, allowed_insecure).await;
    if let Err(e) = par {
        let f = format!(
            "Failed to retrieve provider archive from OCI registry: {}",
            e
        );
        error!("{}", f);
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
        return;
    }
    let cap = cap.unwrap();
    let _provider_id = cap.id();

    let r = hc
        .send(StartProvider {
            provider: cap,
            image_ref: Some(cmd.provider_ref.to_string()),
        })
        .await;
    if let Err(_e) = r {
        let f = "Host controller failed to acknowledge start provider command".to_string();
        error!("{}", f);
        return;
    }
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
            warn!("{}", f);
            ack.failure = Some(f.to_string());
            let _ = msg.respond(&serialize(ack).unwrap()).await;
            return;
        }
    };

    if hc
        .send(StopActor {
            actor_ref: cmd.actor_ref,
        })
        .await
        .is_err()
    {
        let f = "Host controller did not acknowledge stop command";
        error!("{}", f);
        ack.failure = Some(f.to_string());
        let _ = msg.respond(&serialize(ack).unwrap()).await;
        return;
    }

    let _ = msg.respond(&serialize(ack).unwrap()).await;
}

fn claims_to_if(c: &Claims<wascap::jwt::Actor>) -> ::wasmcloud_control_interface::Claims {
    let mut hm = HashMap::new();
    hm.insert("sub".to_string(), c.subject.to_string());
    hm.insert("iss".to_string(), c.issuer.to_string());
    if let Some(ref md) = c.metadata {
        hm.insert(
            "caps".to_string(),
            md.caps
                .as_ref()
                .map(|v| v.join(","))
                .unwrap_or_else(|| "".to_string()),
        );
        hm.insert("rev".to_string(), md.rev.unwrap_or(0).to_string());
        hm.insert(
            "version".to_string(),
            md.ver.as_ref().unwrap_or(&"".to_string()).to_string(),
        );
    }
    ::wasmcloud_control_interface::Claims { values: hm }
}

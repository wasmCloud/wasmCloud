use super::{AdvertiseRemoveLink, LookupAlias, MessageBus, RemoveLink};
use crate::capability::extras::EXTRAS_PUBLIC_KEY;
use crate::dispatch::{gen_config_invocation, Invocation, InvocationResponse, WasmCloudEntity};
use crate::generated::core::CapabilityConfiguration;
use crate::hlreg::HostLocalSystemService;
use crate::messagebus::rpc_client::RpcClient;
use crate::messagebus::rpc_subscription::{CreateSubscription, RpcSubscription};
use crate::messagebus::{
    AdvertiseClaims, AdvertiseLink, CanInvoke, ClaimsResponse, EnforceLocalActorLinks,
    EnforceLocalLink, EnforceLocalProviderLinks, EstablishAllLinks, FindLinks, FindLinksResponse,
    GetClaims, Initialize, LinksResponse, LookupLink, PutClaims, PutLink, QueryActors,
    QueryAllLinks, QueryProviders, QueryResponse, SetCacheClient, Subscribe, Unsubscribe,
};
use crate::{auth, Result};
use actix::prelude::*;
use std::{collections::HashMap, sync::Arc};
use wascap::prelude::KeyPair;
use wasmcloud_provider_core::{
    core::{OP_REMOVE_ACTOR, SYSTEM_ACTOR},
    serialize,
};

pub const OP_HEALTH_REQUEST: &str = "HealthRequest";
pub const OP_BIND_ACTOR: &str = "BindActor";

impl Supervised for MessageBus {}

impl SystemService for MessageBus {
    fn service_started(&mut self, ctx: &mut Context<Self>) {
        info!("Message Bus started");

        // TODO: make this value configurable
        ctx.set_mailbox_capacity(1000);
        self.hb(ctx);
    }
}

impl HostLocalSystemService for MessageBus {}

impl Actor for MessageBus {
    type Context = Context<Self>;
}

impl Handler<FindLinks> for MessageBus {
    type Result = ResponseActFuture<Self, FindLinksResponse>;

    fn handle(&mut self, msg: FindLinks, _ctx: &mut Context<Self>) -> Self::Result {
        let lc = self.latticecache.clone().unwrap();
        Box::pin(
            async move {
                FindLinksResponse {
                    links: lc
                        .collect_links()
                        .await
                        .iter()
                        .filter_map(|ld| {
                            if ld.link_name == msg.link_name && ld.provider_id == msg.provider_id {
                                Some((ld.actor_id.to_string(), ld.values.clone()))
                            } else {
                                None
                            }
                        })
                        .collect(),
                }
            }
            .into_actor(self),
        )
    }
}

impl Handler<EnforceLocalActorLinks> for MessageBus {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: EnforceLocalActorLinks, _ctx: &mut Context<Self>) -> Self::Result {
        let lc = self.latticecache.clone().unwrap();

        Box::pin(
            async move {
                let mut lds = Vec::new();
                let x = lc.collect_links().await;
                for ld in x {
                    if ld.actor_id == msg.actor && lc.has_actor(&msg.actor).await {
                        lds.push(ld);
                    }
                }
                lds
            }
            .into_actor(self)
            .map(move |links, _act, ctx| {
                for link in links {
                    ctx.notify(EnforceLocalLink {
                        actor: link.actor_id,
                        contract_id: link.contract_id,
                        link_name: link.link_name,
                    });
                }
            }),
        )
    }
}

impl Handler<EnforceLocalProviderLinks> for MessageBus {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: EnforceLocalProviderLinks, _ctx: &mut Context<Self>) -> Self::Result {
        if self.latticecache.is_none() {
            return Box::pin(async {}.into_actor(self));
        }

        let lc = self.latticecache.clone().unwrap();

        Box::pin(
            async move {
                let mut lds = Vec::new();
                let x = lc.collect_links().await;
                trace!(
                    "Performing local provider link re-establish check for {}/{} ({} known links)",
                    msg.provider_id,
                    msg.link_name,
                    x.len()
                );
                for ld in x {
                    if ld.link_name == msg.link_name && ld.provider_id == msg.provider_id {
                        lds.push(ld);
                    }
                }
                lds
            }
            .into_actor(self)
            .map(move |links, _act, ctx| {
                for link in links {
                    ctx.notify(EnforceLocalLink {
                        actor: link.actor_id,
                        contract_id: link.contract_id,
                        link_name: link.link_name,
                    });
                }
            }),
        )
    }
}

impl Handler<EnforceLocalLink> for MessageBus {
    type Result = ResponseActFuture<Self, ()>;

    // If the provider responsible for this link is local, and the actor
    // for this link is known to us, then invoke the link binding
    fn handle(&mut self, msg: EnforceLocalLink, _ctx: &mut Context<Self>) -> Self::Result {
        if self.latticecache.is_none() {
            return Box::pin(async {}.into_actor(self));
        }
        let lc = self.latticecache.clone().unwrap();
        let subscribers = self.subscribers.clone();
        let seed = self.key.as_ref().clone().unwrap().seed().unwrap();
        let key = KeyPair::from_seed(&seed).unwrap();
        Box::pin(
            async move {
                let claims = match lc.get_claims(&msg.actor).await {
                    Ok(Some(c)) => c,
                    _ => return,
                };
                if !lc.has_actor(&msg.actor).await {
                    return; // do not send link invocation for actors we don't know about
                }
                if let Ok(Some(ld)) = lc
                    .lookup_link(&msg.actor, &msg.contract_id, &msg.link_name)
                    .await
                {
                    let target = WasmCloudEntity::Capability {
                        id: ld.provider_id.to_string(),
                        contract_id: ld.contract_id.to_string(),
                        link_name: ld.link_name.to_string(),
                    };
                    if let Some(t) = subscribers.get(&target) {
                        let inv = gen_config_invocation(
                            &key,
                            &msg.actor,
                            &msg.contract_id,
                            &ld.provider_id,
                            claims,
                            msg.link_name,
                            ld.values,
                        );
                        let _ = t.send(inv).await;
                    }
                }
            }
            .into_actor(self),
        )
    }
}

impl Handler<EstablishAllLinks> for MessageBus {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, _msg: EstablishAllLinks, _ctx: &mut Context<Self>) -> Self::Result {
        let lc = self.latticecache.clone().unwrap();
        Box::pin(
            async move {
                let mut x = Vec::new();
                for ld in lc.collect_links().await {
                    if lc.has_actor(&ld.actor_id).await {
                        x.push(ld);
                    }
                }
                x
            }
            .into_actor(self)
            .map(|lds, _act, ctx| {
                for ld in lds {
                    ctx.notify(EnforceLocalLink {
                        actor: ld.actor_id,
                        contract_id: ld.contract_id,
                        link_name: ld.link_name,
                    });
                }
            }),
        )
    }
}

impl Handler<QueryActors> for MessageBus {
    type Result = QueryResponse;

    fn handle(&mut self, _msg: QueryActors, _ctx: &mut Context<Self>) -> QueryResponse {
        QueryResponse {
            results: self
                .subscribers
                .keys()
                .filter_map(|k| match k {
                    WasmCloudEntity::Actor(s) => Some(s.to_string()),
                    WasmCloudEntity::Capability { .. } => None,
                })
                .collect(),
        }
    }
}

// Receive a notification of claims
impl Handler<PutClaims> for MessageBus {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: PutClaims, _ctx: &mut Context<Self>) -> Self::Result {
        let subject = msg.claims.subject.to_string();
        let claims = msg.claims.clone();

        let lc = self.latticecache.clone().unwrap();
        Box::pin(
            async move {
                let _ = lc.put_claims(&msg.claims.subject, claims).await;
            }
            .into_actor(self)
            .map(move |_res, _act, ctx| {
                ctx.notify(EnforceLocalActorLinks { actor: subject });
            }),
        )
    }
}

// Receive a request to purge a link definition from the RPC bus
impl Handler<RemoveLink> for MessageBus {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: RemoveLink, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Messagebus received link definition purge notification");
        let lc = self.latticecache.clone().unwrap();
        let key = KeyPair::from_seed(&self.key.as_ref().unwrap().seed().unwrap()).unwrap();
        let subscribers = self.subscribers.clone();
        Box::pin(
            async move {
                let ld = lc
                    .lookup_link(&msg.actor, &msg.contract_id, &msg.link_name)
                    .await?;
                if let Some(l) = ld {
                    let target = WasmCloudEntity::Capability {
                        id: l.provider_id,
                        contract_id: l.contract_id,
                        link_name: l.link_name,
                    };
                    match subscribers.get(&target) {
                        Some(t) => {
                            let inv = Invocation::new(
                                &key,
                                WasmCloudEntity::Actor(SYSTEM_ACTOR.to_string()),
                                target.clone(),
                                OP_REMOVE_ACTOR,
                                serialize(CapabilityConfiguration {
                                    module: msg.actor.to_string(),
                                    values: HashMap::new(),
                                })
                                .unwrap(),
                            );
                            let inv_r = t.send(inv).await?;
                            if let Some(e) = inv_r.error {
                                error!("Provider failed to handle remove actor message: {}", e);
                                return Err(e.into());
                            }
                        }
                        None => {
                            trace!(
                                "Skipping remove actor invocation - target is not on this host."
                            );
                        }
                    }
                    let _ = lc
                        .remove_link(&msg.actor, &msg.contract_id, &msg.link_name)
                        .await;
                    Ok(())
                } else {
                    warn!("Attempted to remove non-existent link.");
                    Ok(())
                }
            }
            .into_actor(self),
        )
    }
}

// Receive a link definition through an advertisement
impl Handler<PutLink> for MessageBus {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: PutLink, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Messagebus received link definition notification");
        let lc = self.latticecache.clone().unwrap();
        Box::pin(
            async move {
                let _ = lc
                    .put_link(
                        &msg.actor,
                        &msg.provider_id,
                        &msg.contract_id,
                        &msg.link_name,
                        msg.values.clone(),
                    )
                    .await;
                msg
            }
            .into_actor(self)
            .map(|msg, _act, ctx| {
                ctx.notify(EnforceLocalLink {
                    actor: msg.actor.to_string(),
                    contract_id: msg.contract_id.to_string(),
                    link_name: msg.link_name,
                });
            }),
        )
    }
}

impl Handler<SetCacheClient> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: SetCacheClient, _ctx: &mut Context<Self>) -> Self::Result {
        self.latticecache = Some(msg.client);
    }
}

impl Handler<CanInvoke> for MessageBus {
    type Result = ResponseActFuture<Self, bool>;

    fn handle(&mut self, msg: CanInvoke, _ctx: &mut Context<Self>) -> Self::Result {
        let lc = self.latticecache.clone().unwrap();
        let auther = self.authorizer.clone();
        let contract_id = msg.contract_id.to_string();

        Box::pin(
            async move {
                if let Ok(Some(c)) = lc.get_claims(&msg.actor).await {
                    let target = WasmCloudEntity::Capability {
                        id: msg.provider_id,
                        contract_id: msg.contract_id.to_string(),
                        link_name: msg.link_name,
                    };
                    if !c
                        .metadata
                        .as_ref()
                        .and_then(|m| m.caps.clone())
                        .map_or(false, |c| c.contains(&contract_id))
                    {
                        return false;
                    }
                    auther
                        .as_ref()
                        .unwrap()
                        .can_invoke(&c, &target, OP_BIND_ACTOR)
                } else {
                    false
                }
            }
            .into_actor(self),
        )
    }
}

impl Handler<QueryAllLinks> for MessageBus {
    type Result = ResponseActFuture<Self, LinksResponse>;

    fn handle(&mut self, _msg: QueryAllLinks, _ctx: &mut Context<Self>) -> Self::Result {
        let lc = self.latticecache.clone().unwrap();
        Box::pin(
            async move {
                LinksResponse {
                    links: lc.collect_links().await,
                }
            }
            .into_actor(self),
        )
    }
}

impl Handler<QueryProviders> for MessageBus {
    type Result = QueryResponse;

    fn handle(&mut self, _msg: QueryProviders, _ctx: &mut Context<Self>) -> QueryResponse {
        QueryResponse {
            results: self
                .subscribers
                .keys()
                .filter_map(|k| match k {
                    WasmCloudEntity::Capability { id, .. } => Some(id.to_string()),
                    _ => None,
                })
                .collect(),
        }
    }
}

impl Handler<Initialize> for MessageBus {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: Initialize, ctx: &mut Context<Self>) -> Self::Result {
        self.key = Some(msg.key);
        self.authorizer = Some(msg.auth);
        self.nc = msg.nc;
        self.namespace = msg.namespace;

        let ns = self.namespace.clone();
        let timeout = msg.rpc_timeout;
        info!("Messagebus initialized");
        if let Some(nc) = self.nc.clone() {
            let rpc_outbound = RpcClient::default().start();
            self.rpc_outbound = Some(rpc_outbound);
            let target = self.rpc_outbound.clone().unwrap();
            let bus = ctx.address();
            let host_id = self.key.as_ref().unwrap().public_key();
            info!("Messagebus initializing with lattice RPC support");
            Box::pin(
                async move {
                    let _ = target
                        .send(super::rpc_client::Initialize {
                            host_id,
                            nc,
                            ns_prefix: ns,
                            bus,
                            rpc_timeout: timeout,
                        })
                        .await;
                }
                .into_actor(self),
            )
        } else {
            Box::pin(async move {}.into_actor(self))
        }
    }
}

impl Handler<AdvertiseLink> for MessageBus {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: AdvertiseLink, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Advertisting link definition");

        let advlink = msg.clone();
        let rpc = self.rpc_outbound.clone();
        let lc = self.latticecache.clone().unwrap();
        Box::pin(
            async move {
                let _ = lc
                    .put_link(
                        &msg.actor,
                        &msg.provider_id,
                        &msg.contract_id,
                        &msg.link_name,
                        msg.values,
                    )
                    .await;

                if let Some(ref rpc) = rpc {
                    let _ = rpc.send(advlink).await;
                }
                EnforceLocalLink {
                    actor: msg.actor.to_string(),
                    contract_id: msg.contract_id.to_string(),
                    link_name: msg.link_name.to_string(),
                }
            }
            .into_actor(self)
            .map(|ell, _act, ctx| {
                ctx.notify(ell);
                Ok(())
            }),
        )
    }
}

impl Handler<AdvertiseRemoveLink> for MessageBus {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: AdvertiseRemoveLink, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Advertising link removal");

        let rpc = self.rpc_outbound.clone();
        Box::pin(
            async move {
                if let Some(ref rpc) = rpc {
                    if let Err(e) = rpc.send(msg).await {
                        error!(
                            "Failed to advertise link definition removal to lattice: {}",
                            e
                        );
                    }
                    None
                } else {
                    Some(RemoveLink {
                        actor: msg.actor,
                        contract_id: msg.contract_id,
                        link_name: msg.link_name,
                    })
                }
            }
            .into_actor(self)
            .map(|rl, _act, ctx| {
                if let Some(rl) = rl {
                    ctx.notify(rl);
                }
                Ok(())
            }),
        )
    }
}

impl Handler<AdvertiseClaims> for MessageBus {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: AdvertiseClaims, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Advertising claims");
        let lc = self.latticecache.clone().unwrap();

        let rpc = self.rpc_outbound.clone();
        Box::pin(
            async move {
                let _ = lc.put_claims(&msg.claims.subject, msg.claims.clone()).await;
                if let Some(ref md) = msg.claims.metadata {
                    if let Some(ref ca) = md.call_alias {
                        match lc.put_call_alias(ca, &msg.claims.subject).await {
                            Ok(_) => {
                                info!(
                                    "Actor {} has claimed call alias '{}'",
                                    &msg.claims.subject, ca
                                );
                            }
                            Err(e) => {
                                warn!(
                                    "Actor {} failed to claim call alias '{}': {}",
                                    &msg.claims.subject, ca, e
                                );
                            }
                        }
                    }
                }
                let el = EnforceLocalActorLinks {
                    actor: msg.claims.subject.to_string(),
                };
                if let Some(rpc) = rpc {
                    let _ = rpc.send(msg).await;
                }
                el
            }
            .into_actor(self)
            .map(|el, _act, ctx| {
                ctx.notify(el);
                Ok(())
            }),
        )
    }
}

impl Handler<Invocation> for MessageBus {
    type Result = ResponseActFuture<Self, InvocationResponse>;

    /// Handle an invocation from any source to any target. If there is a local subscriber
    /// then the invocation will be delivered directly to that subscriber. If the subscriber
    /// is not local, _and_ there is a lattice provider configured, then the bus will attempt
    /// to satisfy that call via RPC over lattice.
    fn handle(&mut self, msg: Invocation, _ctx: &mut Context<Self>) -> Self::Result {
        trace!(
            "{}: Handling invocation from {} to {}",
            self.key.as_ref().unwrap().public_key(),
            msg.origin_url(),
            msg.target_url()
        );
        let lc = self.latticecache.clone().unwrap();
        let auther = self.authorizer.clone();
        let subscribers = self.subscribers.clone();
        let rpc_outbound = self.rpc_outbound.clone();
        Box::pin(
            async move {
                let can_call = if let Ok(claims_map) = lc.get_all_claims().await {
                    auth::authorize_invocation(&msg, auther.as_ref().unwrap().clone(), &claims_map)
                        .is_ok()
                } else {
                    false
                };
                if !can_call {
                    error!(
                        "Invocation from {} to {} - authorization denied.",
                        msg.origin_url(),
                        msg.target_url()
                    );
                    return InvocationResponse::error(&msg, "Invocation authorization denied");
                }
                // Make call either locally within bus or deferred to RPC
                let res = match subscribers.get(&msg.target) {
                    Some(t) => {
                        trace!("Invocation taking place locally within bus");
                        t.send(msg.clone()).await
                    }
                    None => {
                        if let Some(rpc) = rpc_outbound {
                            trace!("Deferring invocation to lattice (no local subscribers)");
                            rpc.send(msg.clone()).await
                        } else {
                            warn!(
                                "No local subscribers and no RPC client enabled - invocation lost"
                            );
                            Ok(InvocationResponse::error(
                            &msg,
                            &"No local bus subscribers found, and no lattice RPC client enabled"))
                        }
                    }
                };
                match res {
                    Ok(ir) => ir,
                    Err(_e) => {
                        InvocationResponse::error(&msg, &"Mailbox error attempting to invoke")
                    }
                }
            }
            .into_actor(self),
        )
    }
}

impl Handler<LookupAlias> for MessageBus {
    type Result = ResponseActFuture<Self, Option<String>>;

    fn handle(&mut self, msg: LookupAlias, _ctx: &mut Self::Context) -> Self::Result {
        let lc = self.latticecache.clone().unwrap();
        Box::pin(
            async move {
                match lc.lookup_call_alias(&msg.alias).await {
                    Ok(Some(alias)) => Some(alias),
                    _ => None,
                }
            }
            .into_actor(self),
        )
    }
}

impl Handler<LookupLink> for MessageBus {
    type Result = ResponseActFuture<Self, Option<String>>;

    fn handle(&mut self, msg: LookupLink, _ctx: &mut Self::Context) -> Self::Result {
        let lc = self.latticecache.clone().unwrap();
        Box::pin(
            async move {
                match lc
                    .lookup_link(&msg.actor, &msg.contract_id, &msg.link_name)
                    .await
                {
                    Ok(Some(ld)) => Some(ld.provider_id),
                    _ => None,
                }
            }
            .into_actor(self),
        )
    }
}

// register interest for an entity that's "on" the bus. if the bus has a
// nats connection, it will register the interest of an RPC subscription proxy. If there is no
// nats connection, it will register the interest of the actual subscriber.
impl Handler<Subscribe> for MessageBus {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: Subscribe, _ctx: &mut Context<Self>) -> Self::Result {
        if self.subscribers.contains_key(&msg.interest) {
            trace!("Skipping bus registration - interested party already registered");
            return Box::pin(async move {}.into_actor(self));
        }

        trace!("Bus registered interest for {}", &msg.interest.url());

        let nc = self.nc.clone();
        let ns = self.namespace.clone();
        Box::pin(
            async move {
                let interest = msg.interest.clone();
                if interest.key() == EXTRAS_PUBLIC_KEY {
                    return (interest, msg.subscriber); // extras are not available over lattice as all hosts have it
                }
                let address = if let Some(ref nc) = nc {
                    let addr = RpcSubscription::default().start();
                    let _ = addr
                        .send(CreateSubscription {
                            entity: msg.interest.clone(),
                            target: msg.subscriber,
                            nc: Arc::new(nc.clone()),
                            namespace: ns,
                        })
                        .await;
                    addr.recipient() // RPC subscriber proxy
                } else {
                    msg.subscriber // Actual subscriber
                };
                (interest, address)
            }
            .into_actor(self)
            .map(|(entity, res), act, _ctx| {
                act.subscribers.insert(entity, res);
            }),
        )
    }
}

impl Handler<Unsubscribe> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: Unsubscribe, _ctx: &mut Context<Self>) {
        trace!("Bus removing interest for {}", msg.interest.url());
        if let Some(subscriber) = self.subscribers.remove(&msg.interest) {
            let _ = subscriber.do_send(Invocation::halt(self.key.as_ref().unwrap()));
        } else {
            warn!("Attempted to remove a non-existent subscriber");
        }
    }
}

impl Handler<GetClaims> for MessageBus {
    type Result = ResponseActFuture<Self, ClaimsResponse>;

    fn handle(&mut self, _msg: GetClaims, _ctx: &mut Context<Self>) -> Self::Result {
        let lc = self.latticecache.clone().unwrap();
        Box::pin(
            async move {
                ClaimsResponse {
                    claims: lc.get_all_claims().await.unwrap_or_default(),
                }
            }
            .into_actor(self),
        )
    }
}

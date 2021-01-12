use super::MessageBus;
use crate::control_interface::ctlactor::{ControlInterface, PublishEvent};
use crate::control_interface::events::RunState;
use crate::generated::core::{deserialize, serialize, HealthRequest, HealthResponse};
use crate::hlreg::HostLocalSystemService;
use crate::messagebus::handlers::OP_HEALTH_REQUEST;
use crate::Result;
use crate::{ControlEvent, Invocation, WasmCloudEntity, SYSTEM_ACTOR};
use actix::prelude::*;
use std::collections::HashMap;
use std::time::Duration;
use wascap::prelude::KeyPair;

const HEARTBEAT_INTERVAL_ENV_VAR: &str = "HEARTBEAT_INTERVAL_S";
const DEFAULT_HEARTBEAT_INTERVAL: u16 = 30;
const PING_TIMEOUT_MS: u64 = 200;

impl MessageBus {
    pub(crate) fn hb(&self, ctx: &mut Context<Self>) {
        trace!("Emitting heartbeat");
        let interval = hb_duration();
        ctx.run_interval(interval, |act, ctx| {
            let subs = act.subscribers.clone();
            let entities: Vec<(_, _)> = subs.into_iter().collect();
            let seed = act.key.as_ref().unwrap().seed().unwrap();
            let host_id = act.key.as_ref().unwrap().public_key();
            let lc = act.latticecache.clone().unwrap();

            ctx.wait(
                async move {
                    let c = lc.get_all_claims().await;
                    let claims = c.unwrap_or(HashMap::new()).values().cloned().collect();
                    let evt = generate_heartbeat_event(entities, claims, seed).await;
                    let cp = ControlInterface::from_hostlocal_registry(&host_id);
                    cp.do_send(PublishEvent { event: evt });
                }
                .into_actor(act),
            );
        });
    }
}

async fn generate_heartbeat_event(
    entities: Vec<(WasmCloudEntity, Recipient<Invocation>)>,
    claims: Vec<wascap::jwt::Claims<wascap::jwt::Actor>>,
    seed: String,
) -> ControlEvent {
    ControlEvent::Heartbeat {
        claims,
        entities: healthping_subscribers(&entities, seed).await,
    }
}

async fn healthping_subscribers(
    subs: &[(WasmCloudEntity, Recipient<Invocation>)],
    seed: String,
) -> HashMap<String, RunState> {
    let key = KeyPair::from_seed(&seed).unwrap();
    let mut hm = HashMap::new();
    for (subscriber, recipient) in subs {
        let ping = generate_ping(subscriber, &key);
        let pong = recipient
            .send(ping)
            .timeout(Duration::from_millis(PING_TIMEOUT_MS))
            .await;
        match pong {
            Ok(ir) => {
                let hr: Result<HealthResponse> = deserialize(&ir.msg);
                match hr {
                    Ok(hr) => {
                        if hr.healthy {
                            hm.insert(subscriber.key(), RunState::Running);
                        } else {
                            hm.insert(subscriber.key(), RunState::Unhealthy(hr.message));
                        }
                    }
                    Err(_e) => {
                        hm.insert(
                            subscriber.key(),
                            RunState::Unhealthy(
                                "Failed to de-serialize health check response from target"
                                    .to_string(),
                            ),
                        );
                    }
                }
            }
            Err(_e) => {
                hm.insert(
                    subscriber.key(),
                    RunState::Unhealthy(
                        "No successful health check response from target".to_string(),
                    ),
                );
            }
        }
    }
    hm
}

fn generate_ping(target: &WasmCloudEntity, key: &KeyPair) -> Invocation {
    Invocation::new(
        key,
        WasmCloudEntity::Actor(SYSTEM_ACTOR.to_string()),
        target.clone(),
        OP_HEALTH_REQUEST,
        serialize(&HealthRequest { placeholder: true }).unwrap(),
    )
}

fn hb_duration() -> Duration {
    match std::env::var(HEARTBEAT_INTERVAL_ENV_VAR) {
        Ok(s) => Duration::from_secs(s.parse().unwrap_or(DEFAULT_HEARTBEAT_INTERVAL as u64)),
        Err(_) => Duration::from_secs(DEFAULT_HEARTBEAT_INTERVAL as u64),
    }
}

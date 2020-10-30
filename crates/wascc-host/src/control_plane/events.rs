use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use crate::generated::core::HealthResponse;
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct EventHeader {
    pub host_origin: String,
    pub timestamp: u64,
}
/// Represents an event that may occur on the lattice control plane. All timestamps
/// are to be considered as Unix timestamps in UTC in seconds since the epoch.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum ControlEvent {
    HostStarted {
        header: EventHeader,
    },
    HostStopped {
        header: EventHeader,
        reason: TerminationReason,
    },
    ActorStarted {
        header: EventHeader,
        actor: String,
        image_ref: Option<String>,
    },
    ActorStopped {
        header: EventHeader,
        actor: String,
        reason: TerminationReason,
    },
    ActorUpdateBegan {
        header: EventHeader,
        actor: String,
        old_revision: u32,
        new_revision: u32,
    },
    ActorUpdateCompleted {
        header: EventHeader,
        actor: String,
        old_revision: u32,
        new_revision: u32,
    },
    ProviderStarted {
        header: EventHeader,
        contract_id: String,
        binding_name: String,
        provider_id: String,
        image_ref: Option<String>,
    },
    ProviderStopped {
        header: EventHeader,
        contract_id: String,
        binding_name: String,
        provider_id: String,
        reason: TerminationReason,
    },
    Heartbeat {
        header: EventHeader,
        claims: Vec<wascap::jwt::Claims<wascap::jwt::Actor>>,
        entities: HashMap<String, RunState>
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum RunState {
    Running,
    Unhealthy(String),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum TerminationReason {
    Requested,
    Unexpected(String),
}

impl ControlEvent {
    pub fn replace_header(self, origin: &str) -> ControlEvent {
        let new_header = EventHeader {
            host_origin: origin.to_string(),
            timestamp: Utc::now().timestamp() as u64,
        };
        use ControlEvent::*;
        match self {
            HostStarted { .. } => HostStarted { header: new_header },
            HostStopped { reason, .. } => HostStopped {
                header: new_header,
                reason,
            },
            ActorStopped { actor, reason, .. } => ActorStopped {
                header: new_header,
                actor,
                reason,
            },
            ActorStarted { actor, image_ref, .. } => ActorStarted {
                header: new_header,
                actor,
                image_ref,
            },
            ActorUpdateBegan {
                actor,
                old_revision,
                new_revision,
                ..
            } => ActorUpdateBegan {
                header: new_header,
                actor,
                old_revision,
                new_revision,
            },
            ActorUpdateCompleted {
                actor,
                old_revision,
                new_revision,
                ..
            } => ActorUpdateCompleted {
                header: new_header,
                actor,
                old_revision,
                new_revision,
            },
            ProviderStarted {
                contract_id,
                binding_name,
                provider_id,
                image_ref,
                ..
            } => ProviderStarted {
                header: new_header,
                contract_id,
                binding_name,
                provider_id,
                image_ref,
            },
            ProviderStopped {
                contract_id,
                binding_name,
                provider_id,
                reason,
                ..
            } => ProviderStopped {
                header: new_header,
                contract_id,
                binding_name,
                provider_id,
                reason,
            },
            Heartbeat { claims, entities, .. } =>
                Heartbeat {
                    header: new_header,
                    claims,
                    entities
                }
        }
    }
}

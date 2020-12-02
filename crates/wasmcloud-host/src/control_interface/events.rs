use chrono::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct EventHeader {
    pub host_origin: String,
    pub timestamp: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PublishedEvent {
    pub event: ControlEvent,
    pub header: EventHeader,
}
/// Represents an event that may occur on the lattice control plane. All timestamps
/// are to be considered as Unix timestamps in UTC in seconds since the epoch.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum ControlEvent {
    HostStarted,
    HostStopped,
    ActorStarted {
        actor: String,
        image_ref: Option<String>,
    },
    ActorStopped {
        actor: String
    },
    ActorUpdateBegan {
        actor: String,
        old_revision: u32,
        new_revision: u32,
    },
    ActorUpdateCompleted {
        actor: String,
        old_revision: u32,
        new_revision: u32,
    },
    ProviderStarted {
        contract_id: String,
        binding_name: String,
        provider_id: String,
        image_ref: Option<String>,
    },
    ProviderStopped {
        contract_id: String,
        binding_name: String,
        provider_id: String,
    },
    Heartbeat {
        claims: Vec<wascap::jwt::Claims<wascap::jwt::Actor>>,
        entities: HashMap<String, RunState>,
    },
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
    pub fn into_published(self, origin: &str) -> PublishedEvent {
        let header = EventHeader {
            host_origin: origin.to_string(),
            timestamp: Utc::now().timestamp() as u64,
        };
        PublishedEvent {
            header,
            event: self,
        }
    }
}

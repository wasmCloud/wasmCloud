use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
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
        reason: Option<String>,
    },
    ActorStarting {
        header: EventHeader,
        actor: String,
    },
    ActorStarted {
        header: EventHeader,
        actor: String,
    },
    ActorStopped {
        header: EventHeader,
        reason: Option<String>,
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
    },
    ProviderStopped {
        header: EventHeader,
        contract_id: String,
        binding_name: String,
        provider_id: String,
    },
}

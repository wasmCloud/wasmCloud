use super::CtlCliCommand;
use crate::ctl::*;
use crate::util::{labels_vec_to_hashmap, OutputKind, Result};
use std::collections::HashMap;
use CtlCliCommand::*;
pub(crate) enum HostCommand {
    Call {
        actor: String,
        operation: String,
        msg: Result<Vec<u8>>,
        output_kind: OutputKind,
    },
    GetHost {
        output_kind: OutputKind,
    },
    GetInventory {
        output_kind: OutputKind,
    },
    GetClaims {
        output_kind: OutputKind,
    },
    Link {
        actor_id: String,
        provider_id: String,
        contract_id: String,
        link_name: Option<String>,
        values: Result<HashMap<String, String>>,
        output_kind: OutputKind,
    },
    StartActor {
        actor_ref: String,
        output_kind: OutputKind,
    },
    StartProvider {
        provider_ref: String,
        link_name: String,
        output_kind: OutputKind,
    },
    StopActor {
        actor_ref: String,
        output_kind: OutputKind,
    },
    StopProvider {
        provider_ref: String,
        contract_id: String,
        link_name: String,
        output_kind: OutputKind,
    },
    UpdateActor {
        actor_id: String,
        new_oci_ref: Option<String>,
        bytes: Vec<u8>,
        output_kind: OutputKind,
    },
}

impl From<CtlCliCommand> for HostCommand {
    /// Transforms a CtlCliCommand to a command to invoke on a standalone host
    fn from(cmd: CtlCliCommand) -> Self {
        match cmd {
            Call(CallCommand {
                actor_id,
                operation,
                data,
                output,
                ..
            }) => HostCommand::Call {
                actor: actor_id,
                operation,
                msg: crate::util::json_str_to_msgpack_bytes(data),
                output_kind: output.kind,
            },
            Get(GetCommand::Hosts(cmd)) => HostCommand::GetHost {
                output_kind: cmd.output.kind,
            },
            Get(GetCommand::HostInventory(cmd)) => HostCommand::GetInventory {
                output_kind: cmd.output.kind,
            },
            Get(GetCommand::Claims(cmd)) => HostCommand::GetClaims {
                output_kind: cmd.output.kind,
            },
            Start(StartCommand::Actor(cmd)) => HostCommand::StartActor {
                actor_ref: cmd.actor_ref,
                output_kind: cmd.output.kind,
            },
            Start(StartCommand::Provider(cmd)) => HostCommand::StartProvider {
                provider_ref: cmd.provider_ref,
                link_name: cmd.link_name,
                output_kind: cmd.output.kind,
            },
            Stop(StopCommand::Actor(cmd)) => HostCommand::StopActor {
                actor_ref: cmd.actor_id,
                output_kind: cmd.output.kind,
            },
            Stop(StopCommand::Provider(cmd)) => HostCommand::StopProvider {
                provider_ref: cmd.provider_id,
                contract_id: cmd.contract_id,
                link_name: cmd.link_name,
                output_kind: cmd.output.kind,
            },
            Link(LinkCommand {
                actor_id,
                provider_id,
                contract_id,
                link_name,
                values,
                output,
                ..
            }) => HostCommand::Link {
                actor_id,
                provider_id,
                contract_id,
                link_name,
                values: labels_vec_to_hashmap(values),
                output_kind: output.kind,
            },
            Update(UpdateCommand::Actor(cmd)) => HostCommand::UpdateActor {
                actor_id: cmd.actor_id,
                new_oci_ref: Some(cmd.new_actor_ref),
                bytes: vec![],
                output_kind: cmd.output.kind,
            },
        }
    }
}

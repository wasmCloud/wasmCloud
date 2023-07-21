use anyhow::Result;
use wash_lib::cli::{
    claims::get_claims,
    get::{get_host_inventory, get_hosts, GetCommand, GetLinksCommand},
    link::{LinkCommand, LinkQueryCommand},
};

use crate::{
    appearance::spinner::Spinner,
    common::link_cmd::handle_command as handle_link_command,
    ctl::{get_claims_output, get_host_inventory_output, get_hosts_output},
    CommandOutput, OutputKind,
};

pub(crate) async fn handle_command(
    command: GetCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out: CommandOutput = match command {
        GetCommand::Links(GetLinksCommand { opts }) => {
            handle_link_command(LinkCommand::Query(LinkQueryCommand { opts }), output_kind).await?
        }
        GetCommand::Claims(cmd) => {
            sp.update_spinner_message("Retrieving claims ... ".to_string());
            let claims = get_claims(cmd).await?;
            get_claims_output(claims)
        }
        GetCommand::Hosts(cmd) => {
            sp.update_spinner_message(" Retrieving Hosts ...".to_string());
            let hosts = get_hosts(cmd).await?;
            get_hosts_output(hosts)
        }
        GetCommand::HostInventory(cmd) => {
            if let Some(id) = cmd.host_id.as_ref() {
                sp.update_spinner_message(format!(" Retrieving inventory for host {} ...", id));
            } else {
                sp.update_spinner_message(" Retrieving hosts for inventory query ...".to_string());
            }
            let inv = get_host_inventory(cmd).await?;
            get_host_inventory_output(inv)
        }
    };

    Ok(out)
}

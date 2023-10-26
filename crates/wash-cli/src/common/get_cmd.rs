use anyhow::Result;
use wash_lib::cli::claims::get_claims;
use wash_lib::cli::get::{get_host_inventories, get_hosts, GetCommand, GetLinksCommand};
use wash_lib::cli::link::{LinkCommand, LinkQueryCommand};
use wash_lib::cli::{CommandOutput, OutputKind};

use crate::appearance::spinner::Spinner;
use crate::common::link_cmd::handle_command as handle_link_command;
use crate::ctl::{get_claims_output, get_host_inventories_output, get_hosts_output};

pub async fn handle_command(command: GetCommand, output_kind: OutputKind) -> Result<CommandOutput> {
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
        GetCommand::HostInventories(cmd) => {
            if let Some(id) = cmd.host_id.as_ref() {
                sp.update_spinner_message(format!(" Retrieving inventory for host {} ...", id));
            } else {
                sp.update_spinner_message(" Retrieving hosts for inventory query ...".to_string());
            }
            let invs = get_host_inventories(cmd).await?;
            get_host_inventories_output(invs)
        }
    };

    Ok(out)
}

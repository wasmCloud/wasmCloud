use anyhow::Result;
use crossterm::{
    cursor, execute,
    terminal::{Clear, ClearType},
};
use std::{collections::HashMap, io::Write, time::Duration};
use tokio::time::sleep;
use crate::lib::cli::claims::get_claims;
use crate::lib::cli::get::{
    get_host_inventories, get_hosts, GetCommand, GetHostInventoriesCommand, GetLinksCommand,
};
use crate::lib::cli::link::{LinkCommand, LinkQueryCommand};
use crate::lib::cli::{CommandOutput, OutputKind};

use crate::appearance::spinner::Spinner;
use crate::cmd::link::invoke as invoke_link_cmd;
use crate::ctl::{
    get_claims_output, get_host_inventories_output, get_hosts_output, host_inventories_table,
};

pub async fn handle_command(command: GetCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    let out: CommandOutput = match command {
        GetCommand::Links(GetLinksCommand { opts }) => {
            invoke_link_cmd(LinkCommand::Query(LinkQueryCommand { opts }), output_kind).await?
        }
        GetCommand::Claims(cmd) => {
            let sp: Spinner = Spinner::new(&output_kind)?;
            sp.update_spinner_message("Retrieving claims ... ".to_string());
            let claims = get_claims(cmd).await?;
            get_claims_output(claims)
        }
        GetCommand::Hosts(cmd) => {
            let sp: Spinner = Spinner::new(&output_kind)?;
            sp.update_spinner_message(" Retrieving Hosts ...".to_string());
            let hosts = get_hosts(cmd).await?;
            get_hosts_output(hosts)
        }
        GetCommand::HostInventories(cmd) => {
            let sp: Spinner = Spinner::new(&output_kind)?;
            if let Some(id) = cmd.host_id.as_ref() {
                sp.update_spinner_message(format!(" Retrieving inventory for host {id} ..."));
            } else {
                sp.update_spinner_message(" Retrieving hosts for inventory query ...".to_string());
            }
            get_inventory_handler(cmd, sp).await?
        }
    };

    Ok(out)
}

async fn get_inventory_handler(
    cmd: GetHostInventoriesCommand,
    sp: Spinner,
) -> Result<CommandOutput> {
    if cmd.watch.is_some() {
        watch_inventory(cmd, sp).await?;
        Ok(CommandOutput::new(
            "Completed Watching Inventory".to_string(),
            HashMap::new(),
        ))
    } else {
        let invs = get_host_inventories(cmd).await?;
        Ok(get_host_inventories_output(invs))
    }
}

async fn watch_inventory(cmd: GetHostInventoriesCommand, sp: Spinner) -> Result<()> {
    let mut stdout = std::io::stdout();
    let invs = get_host_inventories(cmd.clone()).await?;
    sp.finish_and_clear();
    execute!(stdout, Clear(ClearType::FromCursorUp), cursor::MoveTo(0, 0))
        .map_err(|e| anyhow::anyhow!("Failed to clear terminal: {}", e))?;
    let output = host_inventories_table(invs);
    stdout
        .write_all(output.as_bytes())
        .map_err(|e| anyhow::anyhow!("Failed to write inventory to stdout: {}", e))?;

    let mut ctrlc = std::pin::pin!(tokio::signal::ctrl_c());
    let watch_interval = cmd.watch.unwrap_or(Duration::from_millis(5000));

    loop {
        let invs = tokio::select! {
            res = get_host_inventories(cmd.clone()) => res?,
            res = &mut ctrlc => {
                res?;
                execute!(stdout, Clear(ClearType::Purge),Clear(ClearType::FromCursorUp), cursor::MoveTo(0, 0), cursor::Show)
                    .map_err(|e| anyhow::anyhow!("Failed to execute terminal commands: {}", e))?;
                stdout.flush()
                    .map_err(|e| anyhow::anyhow!("Failed to flush stdout: {}", e))?;
                return Ok(());
            }
        };

        execute!(stdout, Clear(ClearType::Purge), cursor::MoveTo(0, 0))
            .map_err(|e| anyhow::anyhow!("Failed to execute terminal commands: {}", e))?;

        let output = host_inventories_table(invs);
        stdout
            .write_all(output.as_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to write inventory to stdout: {}", e))?;

        stdout
            .flush()
            .map_err(|e| anyhow::anyhow!("Failed to flush stdout: {}", e))?;

        execute!(
            stdout,
            Clear(ClearType::CurrentLine),
            Clear(ClearType::FromCursorDown),
        )
        .map_err(|e| anyhow::anyhow!("Failed to clear terminal: {}", e))?;

        tokio::select! {
            () = sleep(watch_interval) => continue,
            res = &mut ctrlc => {
                res?;
                execute!(stdout, Clear(ClearType::Purge),Clear(ClearType::FromCursorUp), cursor::MoveTo(0, 0), cursor::Show)
                    .map_err(|e| anyhow::anyhow!("Failed to execute terminal commands: {}", e))?;
                stdout.flush()
                    .map_err(|e| anyhow::anyhow!("Failed to flush stdout: {}", e))?;
                return Ok(());
            }
        }
    }
}

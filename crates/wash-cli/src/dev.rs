use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{path::PathBuf, sync::Arc};

use anyhow::{bail, Context, Result};
use clap::Parser;
use console::style;
use notify::{event::EventKind, Event as NotifyEvent, RecursiveMode, Watcher};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};
use tokio::{select, sync::mpsc};
use wash_lib::{
    build::{build_project, SignConfig},
    cli::dev::run_dev_loop,
    cli::{sanitize_component_id, CommandOutput},
    component::{scale_component, ScaleComponentArgs},
    config::{downloads_dir, WASMCLOUD_PID_FILE},
    generate::emoji,
    id::{ModuleId, ServerId},
    parser::get_config,
};
use wasmcloud_control_interface::Host;

use crate::{
    down::{handle_down, DownCommand},
    up::{handle_up, NatsOpts, UpCommand, WadmOpts, WasmcloudOpts},
};

#[derive(Debug, Clone, Parser)]
pub struct DevCommand {
    #[clap(flatten)]
    pub nats_opts: NatsOpts,

    #[clap(flatten)]
    pub wasmcloud_opts: WasmcloudOpts,

    #[clap(flatten)]
    pub wadm_opts: WadmOpts,

    /// ID of the host to use for `wash dev`
    /// if one is not selected, `wash dev` will attempt to use the single host in the lattice
    #[clap(long = "host-id", name = "host-id", value_parser)]
    pub host_id: Option<ServerId>,

    /// Path to code directory
    #[clap(name = "code-dir", long = "work-dir", env = "WASH_DEV_CODE_DIR")]
    pub code_dir: Option<PathBuf>,

    /// Whether to leave the host running after dev
    #[clap(
        name = "leave-host-running",
        long = "leave-host-running",
        env = "WASH_DEV_LEAVE_HOST_RUNNING",
        default_value = "false",
        help = "Leave the wasmCloud host running after stopping the devloop"
    )]
    pub leave_host_running: bool,

    /// Run the host in a subprocess (rather than detached mode)
    #[clap(
        name = "use-host-subprocess",
        long = "use-host-subprocess",
        env = "WASH_DEV_USE_HOST_SUBPROCESS",
        default_value = "false",
        help = "Run the wasmCloud host in a subprocess (rather than detached mode)"
    )]
    pub use_host_subprocess: bool,
}

/// Utility struct for holding a wasmCloud host subprocess.
/// This struct ensures that the join handle is aborted once the
/// subprocess is dropped.
struct HostSubprocess(Option<JoinHandle<()>>);

impl HostSubprocess {
    fn into_inner(mut self) -> Option<JoinHandle<()>> {
        self.0.take()
    }
}

impl Drop for HostSubprocess {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// Handle `wash dev`
pub async fn handle_command(
    cmd: DevCommand,
    output_kind: wash_lib::cli::OutputKind,
) -> Result<CommandOutput> {
    // Check if host is running
    let pid_file = downloads_dir()?.join(WASMCLOUD_PID_FILE);
    let existing_instance = tokio::fs::metadata(pid_file).await.is_ok();

    let mut host_subprocess: Option<HostSubprocess> = None;

    // Start host if it's not already running
    if !existing_instance {
        eprintln!(
            "{} {}{}",
            emoji::WARN,
            style("No running wasmcloud host detected (PID file missing), ").bold(),
            style("starting a new host...").bold()
        );
        // Ensure that file loads are allowed
        let mut wasmcloud_opts = cmd.wasmcloud_opts.clone();
        wasmcloud_opts.allow_file_load = Some(true);

        if cmd.use_host_subprocess {
            // Use a subprocess
            eprintln!(
                "{} {}",
                emoji::WRENCH,
                style("starting wasmCloud host subprocess...").bold(),
            );
            let nats_opts = cmd.nats_opts.clone();
            let wadm_opts = cmd.wadm_opts.clone();
            host_subprocess = Some(HostSubprocess(Some(tokio::spawn(async move {
                let _ = handle_up(
                    UpCommand {
                        detached: false,
                        nats_opts,
                        wasmcloud_opts,
                        wadm_opts,
                    },
                    output_kind,
                )
                .await;
                eprintln!(
                    "{} {}",
                    emoji::WRENCH,
                    style("shutting down host subprocess...").bold(),
                );
            }))));

            // Wait a while for wasmcloud to start up
            tokio::time::sleep(Duration::from_secs(5)).await;
        } else {
            // Run a detached process via running the equivalent of `wash up`

            // Run wash up to start the host if not already running
            let _ = handle_up(
                UpCommand {
                    detached: true,
                    nats_opts: cmd.nats_opts,
                    wasmcloud_opts,
                    wadm_opts: cmd.wadm_opts,
                },
                output_kind,
            )
            .await?;
        }

        eprintln!(
            "{} {}",
            emoji::WRENCH,
            style("Successfully started wasmCloud instance").bold(),
        );
    }

    // Connect to the wasmcloud instance
    let ctl_client = Arc::new(
        cmd.wasmcloud_opts
            .into_ctl_client(None)
            .await
            .context("failed to create wasmcloud control client")?,
    );
    let wait_ctl_client = ctl_client.clone();

    // If we started our own instance, wait for one host to be present
    if !existing_instance {
        eprintln!("⏳ ");
        eprintln!(
            "{} {}",
            emoji::HOURGLASS_DRAINING,
            style("Waiting for host to become reachable...").bold(),
        );

        // Wait for up to a minute to find the host
        let _ = timeout(
            Duration::from_secs(60),
            tokio::spawn(async move {
                loop {
                    match wait_ctl_client.get_hosts().await {
                        Ok(hs) => match &hs[..] {
                            [] => {}
                            [h] => {
                                eprintln!(
                                    "{} {}",
                                    emoji::GREEN_CHECK,
                                    style(format!(
                                        "Found single host w/ ID [{}]",
                                        h.response
                                            .as_ref()
                                            .map(|r| r.id.clone())
                                            .unwrap_or_else(|| "N/A".to_string())
                                    ))
                                    .bold(),
                                );
                                break Ok(());
                            }
                            _hs => {
                                bail!("Detected an unexpected number (>1) of hosts present.");
                            }
                        },
                        Err(e) => {
                            eprintln!(
                                "{} {}",
                                emoji::WARN,
                                style(format!("Failed to get hosts (will retry in 5s): {e}"))
                                    .bold(),
                            );
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }),
        )
        .await
        .context("wasmCloud host did not become reachable")?;
    }

    // Refresh host information (used in particular for existing instances)
    let hosts = ctl_client
        .get_hosts()
        .await
        .or_else(|e| bail!("failed to retrieve hosts from lattice: {e}"))?;
    let host: Host = match &hosts[..] {
        [] => bail!("0 hosts detected, is wasmCloud running?"),
        [h] => h
            .response
            .clone()
            .context("received control interface response with empty host")?,
        _ => {
            if let Some(host_id) = cmd.host_id.map(ServerId::into_string) {
                hosts
                    .into_iter()
                    .filter_map(|h| h.response)
                    .find(|h| h.id == host_id)
                    .with_context(|| format!("failed to find host [{host_id}]"))?
            } else {
                bail!(
                    "{} hosts detected, please specify the host on which to deploy with --host-id",
                    hosts.len()
                )
            }
        }
    };

    // Resolve project configuration from the current path
    let current_dir = std::env::current_dir()?;
    let project_path = cmd.code_dir.unwrap_or(current_dir);
    let project_cfg = get_config(Some(project_path.clone()), Some(true))?;

    // Build the project (equivalent to `wash build`)
    let sign_cfg: Option<SignConfig> = Some(SignConfig {
        keys_directory: None,
        issuer: None,
        subject: None,
        disable_keygen: false,
    });
    eprintln!(
        "{} {}",
        emoji::CONSTRUCTION_BARRIER,
        style("Starting project build").bold(),
    );

    // Build the project
    let artifact_path = build_project(&project_cfg, sign_cfg.as_ref())
        .await
        .context("failed to build project")?
        .canonicalize()
        .context("failed to canonicalize path")?;
    eprintln!(
        "✅ successfully built project at [{}]",
        artifact_path.display()
    );

    // Since we're using the component from file on disk, the ref should be the file path (canonicalized) on disk as URI
    let component_ref = format!("file://{}", artifact_path.display());
    // Since the only restriction on component_id is that it must be unique, we can just use the artifact path as the component_id
    // to ensure uniqueness
    let component_id = sanitize_component_id(&artifact_path.display().to_string());

    // Scale the component to one max replica
    scale_component(ScaleComponentArgs {
        client: &ctl_client,
        host_id: &host.id,
        component_id: &component_id,
        component_ref: &component_ref,
        max_instances: 1,
        annotations: Some(HashMap::from_iter(vec![(
            "wash_dev".to_string(),
            "true".to_string(),
        )])),
        config: vec![],
        skip_wait: false,
        timeout_ms: None,
    })
    .await?;

    // Set up a oneshot channel to remove
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    let (reload_tx, mut reload_rx) = mpsc::channel::<()>(1);

    // Handle Ctrl + c with Tokio
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .context("failed to wait for ctrl_c signal")?;
        stop_tx
            .send(())
            .await
            .context("failed to send stop signal after receiving Ctrl + c")?;
        Result::<_, anyhow::Error>::Ok(())
    });

    // Enable/disable watching to prevent having the output artifact trigger a rebuild
    let pause_watch = Arc::new(AtomicBool::new(false));
    let watcher_paused = pause_watch.clone();

    // Spawn a file watcher to listen for changes and send on reload_tx
    let mut watcher = notify::recommended_watcher(move |res: _| match res {
        Ok(event) => match event {
            NotifyEvent {
                kind: EventKind::Create(_),
                ..
            }
            | NotifyEvent {
                kind: EventKind::Modify(_),
                ..
            }
            | NotifyEvent {
                kind: EventKind::Remove(_),
                ..
            } => {
                // If watch has been paused for any reason, skip notifications
                if watcher_paused.load(Ordering::SeqCst) {
                    return;
                }

                let _ = reload_tx.blocking_send(());
            }
            _ => {}
        },
        Err(e) => {
            eprintln!("[error] watch failed: {:?}", e);
        }
    })?;
    watcher.watch(&project_path.clone(), RecursiveMode::Recursive)?;

    // Watch FS for changes and listen for Ctrl + C in tandem
    eprintln!("👀 watching for file changes (press Ctrl+c to stop)...");
    loop {
        select! {
            _ = reload_rx.recv() => {
                pause_watch.store(true, Ordering::SeqCst);
                run_dev_loop(
                    &project_cfg,
                    ModuleId::from_str(&component_id)?,
                    &component_ref,
                    ServerId::from_str(&host.id)?,
                    &ctl_client,
                    sign_cfg.clone(),
                ).await?;
                pause_watch.store(false, Ordering::SeqCst);
                eprintln!("👀 watching for file changes (press Ctrl+c to stop)...");
            },
            _ = stop_rx.recv() => {
                pause_watch.store(true, Ordering::SeqCst);
                eprintln!("🛑 received Ctrl + c, stopping devloop...");

                if !cmd.leave_host_running {
                    eprintln!("⏳ stopping wasmCloud instance...");
                    handle_down(DownCommand::default(), output_kind).await.context("down command failed")?;
                    if let Some(handle) = host_subprocess.and_then(|hs| hs.into_inner())  {
                        handle.await?;
                    }
                }

                break Ok(CommandOutput::default());
            },
        }
    }
}

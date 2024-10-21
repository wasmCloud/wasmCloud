use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use clap::Parser;
use console::style;
use notify::event::ModifyKind;
use notify::{event::EventKind, Event as NotifyEvent, RecursiveMode, Watcher};
use semver::Version;
use session::{SessionMetadata, WashDevSession};
use tokio::{select, sync::mpsc};

use wash_lib::cli::{CommandOutput, CommonPackageArgs};
use wash_lib::generate::emoji;
use wash_lib::id::ServerId;
use wash_lib::parser::load_config;

use crate::cmd::up::{
    nats_client_from_wasmcloud_opts, remove_wadm_pidfile, NatsOpts, WadmOpts, WasmcloudOpts,
};

mod deps;
mod devloop;
mod manifest;
mod session;
mod wit;

const DEFAULT_KEYVALUE_PROVIDER_IMAGE: &str = "ghcr.io/wasmcloud/keyvalue-nats:0.3.1";
const DEFAULT_HTTP_CLIENT_PROVIDER_IMAGE: &str = "ghcr.io/wasmcloud/http-client:0.12.1";
const DEFAULT_HTTP_SERVER_PROVIDER_IMAGE: &str = "ghcr.io/wasmcloud/http-server:0.23.1";
const DEFAULT_BLOBSTORE_FS_PROVIDER_IMAGE: &str = "ghcr.io/wasmcloud/blobstore-fs:0.10.1";
const DEFAULT_MESSAGING_NATS_PROVIDER_IMAGE: &str = "ghcr.io/wasmcloud/messaging-nats:0.23.1";

const DEFAULT_INCOMING_HANDLER_ADDRESS: &str = "127.0.0.1:8000";
const DEFAULT_MESSAGING_HANDLER_SUBSCRIPTION: &str = "wasmcloud.dev";
const DEFAULT_BLOBSTORE_ROOT_DIR: &str = "/tmp";
const DEFAULT_KEYVALUE_BUCKET: &str = "wasmcloud";

const WASH_SESSIONS_FILE_NAME: &str = "wash-dev-sessions.json";

const SESSIONS_FILE_VERSION: Version = Version::new(0, 1, 0);
const SESSION_ID_LEN: usize = 6;

const DEFAULT_PROVIDER_STOP_TIMEOUT_MS: u64 = 3000;

/// The path to the dev directory for wash
async fn dev_dir() -> Result<PathBuf> {
    let dir = wash_lib::config::dev_dir().context("failed to resolve config dir")?;
    if !tokio::fs::try_exists(&dir)
        .await
        .context("failed to check if dev dir exists")?
    {
        tokio::fs::create_dir(&dir)
            .await
            .with_context(|| format!("failed to create dir [{}]", dir.display()))?
    }
    Ok(dir)
}

/// Retrieve the path to the file that stores
async fn sessions_file_path() -> Result<PathBuf> {
    dev_dir()
        .await
        .map(|p| p.join(WASH_SESSIONS_FILE_NAME))
        .context("failed to get dev dir")
}

#[derive(Debug, Clone, Parser)]
pub struct DevCommand {
    #[clap(flatten)]
    pub nats_opts: NatsOpts,

    #[clap(flatten)]
    pub wasmcloud_opts: WasmcloudOpts,

    #[clap(flatten)]
    pub wadm_opts: WadmOpts,

    #[clap(flatten)]
    pub package_args: CommonPackageArgs,

    /// ID of the host to use for `wash dev`
    /// if one is not selected, `wash dev` will attempt to use the single host in the lattice
    #[clap(long = "host-id", name = "host-id", value_parser)]
    pub host_id: Option<ServerId>,

    /// Path to code directory
    #[clap(name = "code-dir", long = "work-dir", env = "WASH_DEV_CODE_DIR")]
    pub code_dir: Option<PathBuf>,

    /// Directories to ignore when watching for changes. This should be set
    /// to directories where generated files are placed, such as `target/` or `dist/`.
    /// Can be specified multiple times.
    #[clap(name = "ignore-dir", long = "ignore-dir")]
    pub ignore_dirs: Vec<PathBuf>,

    /// Whether to leave the host running after dev
    #[clap(
        name = "leave-host-running",
        long = "leave-host-running",
        env = "WASH_DEV_LEAVE_HOST_RUNNING",
        default_value = "false",
        help = "Leave the wasmCloud host running after stopping the devloop"
    )]
    pub leave_host_running: bool,

    /// Write generated WADM manifest(s) to a given folder (every time they are generated)
    #[clap(long = "manifest-output-dir", env = "WASH_DEV_MANIFEST_OUTPUT_DIR")]
    pub manifest_output_dir: Option<PathBuf>,

    /// Skip wit dependency fetching and use only what is currently present in the wit directory
    /// (useful for airgapped or disconnected environments)
    #[clap(long = "skip-fetch")]
    pub skip_wit_fetch: bool,
}

/// Handle `wash dev`
pub async fn handle_command(
    cmd: DevCommand,
    _output_kind: wash_lib::cli::OutputKind,
) -> Result<CommandOutput> {
    let current_dir = std::env::current_dir()?;
    let project_path = cmd.code_dir.unwrap_or(current_dir);
    let project_cfg = load_config(Some(project_path.clone()), Some(true)).await?;

    let mut wash_dev_session = WashDevSession::from_sessions_file(&project_path)
        .await
        .context("failed to build wash dev session")?;
    let session_id = wash_dev_session.id.clone();
    eprintln!(
        "{} Resolved wash session ID [{session_id}]",
        emoji::INFO_SQUARE
    );

    let (mut nats_child, mut wadm_child, mut wasmcloud_child) = (None, None, None);

    // If there is not a running host for this session, then we can start one
    if wash_dev_session.host_data.is_none() {
        (nats_child, wadm_child, wasmcloud_child) = wash_dev_session
            .start_host(
                cmd.wasmcloud_opts.clone(),
                cmd.nats_opts.clone(),
                cmd.wadm_opts.clone(),
            )
            .await
            .with_context(|| format!("failed to start host for session [{session_id}]"))?;
    }
    let host_id = wash_dev_session
        .host_data
        .clone()
        .context("missing host_id, after ensuring host has started")?
        .0;

    // Create NATS and control interface client to use to connect
    let nats_client = nats_client_from_wasmcloud_opts(&cmd.wasmcloud_opts).await?;
    let ctl_client = Arc::new(
        cmd.wasmcloud_opts
            .clone()
            .into_ctl_client(None)
            .await
            .context("failed to create wasmcloud control client")?,
    );
    let lattice = ctl_client.lattice();

    // See if the host is running by retrieving an inventory
    if let Err(_e) = ctl_client.get_host_inventory(&host_id).await {
        eprintln!(
            "{} Failed to retrieve inventory from host [{host_id}]... Is it running?",
            emoji::WARN
        );
        eprintln!(
            "{} {}",
            emoji::CONSTRUCTION_BARRIER,
            style(format!(
                "Starting host for wash dev session [{session_id}]...",
            ))
            .bold(),
        );
        (nats_child, wadm_child, wasmcloud_child) = wash_dev_session
            .start_host(
                cmd.wasmcloud_opts.clone(),
                cmd.nats_opts.clone(),
                cmd.wadm_opts.clone(),
            )
            .await
            .context("failed to start host for session")?;
    }

    // Set up a oneshot channel to perform graceful shutdown, handle Ctrl + c w/ tokio
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    let (reload_tx, mut reload_rx) = mpsc::channel::<()>(1);
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
    let project_path_notify = project_path.clone();
    let mut watcher = notify::recommended_watcher(move |res: _| match res {
        Ok(event) => match event {
            NotifyEvent {
                kind: EventKind::Create(_),
                paths,
                ..
            }
            | NotifyEvent {
                kind: EventKind::Modify(ModifyKind::Data(_)),
                paths,
                ..
            }
            | NotifyEvent {
                kind: EventKind::Remove(_),
                paths,
                ..
            } => {
                // Ensure that paths that take place in ignored directories don't trigger a reload
                if paths.iter().any(|p| {
                    p.strip_prefix(project_path_notify.as_path())
                        .is_ok_and(|p| {
                            // Ignore Rust target directories
                            p.starts_with("target")
                                // Ignore wasmCloud build directories
                                || p.starts_with("build")
                                // Ignore user specifieddirectories
                                || cmd.ignore_dirs.iter().any(|ignore| p.starts_with(ignore))
                        })
                }) {
                    return;
                }
                // If watch has been paused for any reason, skip notifications
                if watcher_paused.load(Ordering::SeqCst) {
                    return;
                }

                let _ = reload_tx.blocking_send(());
            }
            _ => {}
        },
        Err(e) => {
            eprintln!("{} Watch failed: {:?}", emoji::ERROR, e);
        }
    })?;
    watcher.watch(&project_path.clone(), RecursiveMode::Recursive)?;

    // Build sup state for the run loop
    let mut run_loop_state = devloop::RunLoopState {
        dev_session: &mut wash_dev_session,
        nats_client: &nats_client,
        ctl_client: &ctl_client,
        project_cfg: &project_cfg,
        lattice,
        session_id: &session_id,
        manifest_output_dir: cmd.manifest_output_dir.as_ref(),
        previous_deps: None,
        artifact_path: None,
        component_id: None,
        component_ref: None,
        package_args: &cmd.package_args,
        skip_fetch: cmd.skip_wit_fetch,
    };

    // NOTE(brooksmtownsend): Yes, it would make more sense to return here. For some reason unknown to me
    // trying to return any error here will just cause the dev loop to hang infinitely and require a force quit.
    // Even a panic will display a tokio error and then hang. Thankfully, the error will just probably happen
    // again when the dev loop runs and in that case it'll successfully exit out.
    if let Err(e) = devloop::run(&mut run_loop_state).await {
        eprintln!(
            "{} Failed to run first dev loop iteration, will retry: {e}",
            emoji::WARN
        );
    }

    // Watch FS for changes and listen for Ctrl + C in tandem
    eprintln!(
        "{} Watching for file changes (press Ctrl+c to stop)...",
        emoji::EYES
    );
    loop {
        select! {
            // Process a file change/reload
            _ = reload_rx.recv() => {
                pause_watch.store(true, Ordering::SeqCst);
                devloop::run(&mut run_loop_state
                )
                    .await
                    .context("failed to run dev loop iteration")?;
                pause_watch.store(false, Ordering::SeqCst);
                eprintln!("\n{} Watching for file changes (press Ctrl+c to stop)...", emoji::EYES);
            },

            // Process a stop
            _ = stop_rx.recv() => {
                pause_watch.store(true, Ordering::SeqCst);
                eprintln!("\n{} Received Ctrl + c, stopping devloop...", emoji::STOP);

                // Update the sessions file with the fact that this session stopped
                run_loop_state.dev_session.in_use = false;
                SessionMetadata::persist_session(run_loop_state.dev_session).await?;

                // Delete manifests related to the application
                if let Some(dependencies) = run_loop_state.previous_deps {
                    eprintln!("{} Cleaning up deployed WADM application(s)...", emoji::BROOM);
                    dependencies.delete_manifests(&nats_client, lattice).await?;
                }

                // Stop the host, unless explicitly instructed to leave host running
                if !cmd.leave_host_running {
                    eprintln!("{} Stopping wasmCloud instance...", emoji::HOURGLASS_DRAINING);

                    // Stop host via the control interface
                    if let Some((ref host_id, _log_file)) = wash_dev_session.host_data.as_ref() {
                        let receiver = ctl_client.events_receiver(vec!["host_stopped".to_string()]).await;
                        if let Err(e) = ctl_client
                            .stop_host(host_id, Some(2000))
                            .await {
                                eprintln!("{} failed to stop host through control interface: {e}", emoji::WARN);
                            }

                        // Wait for the host_stopped event to be received
                        if let Ok(mut receiver) = receiver {
                            receiver.recv().await;
                        }
                    }

                    // Ensure that the host exited, if not, kill the process forcefully
                    if let Some(mut host) = wasmcloud_child {
                        if tokio::time::timeout(std::time::Duration::from_secs(5), host.wait())
                            .await
                            .context("failed to wait for wasmcloud process to stop, forcefully terminating")
                            .is_err() {
                                eprintln!("{} Terminating host forcefully, this may leave provider processes running", emoji::WARN);
                                host
                                    .kill()
                                    .await
                                    .context("failed to stop wasmcloud process")?;
                            }
                    }

                    // Stop WADM
                    if let Some(mut wadm) = wadm_child {
                        eprintln!("{} Stopping wadm...", emoji::HOURGLASS_DRAINING);
                        wadm
                            .kill()
                            .await
                            .context("failed to stop wadm child process")?;
                        remove_wadm_pidfile(wash_dev_session.base_dir().await?)
                            .await
                            .context("failed to remove wadm pidfile")?;
                    }

                    // Stop NATS
                    if let Some(mut nats) = nats_child {
                        eprintln!("{} Stopping NATS...", emoji::HOURGLASS_DRAINING);
                        nats.kill().await?;
                    }
                }

                eprintln!("{} Dev session exited successfully", emoji::GREEN_CHECK);
                break Ok(CommandOutput::default());
            },
        }
    }
}

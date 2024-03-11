use std::{path::PathBuf, time::Duration};

use anyhow::Result;
use async_nats::jetstream::{
    consumer::{pull::Config as ConsumerConfig, AckPolicy, DeliverPolicy},
    stream::Config,
};
use clap::{Parser, Subcommand};
use futures::TryStreamExt;
use tokio::io::{stdin, stdout, AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;
use wasmcloud_core::Invocation;

use super::{CliConnectionOpts, CommandOutput};
use crate::config::WashConnectionOptions;
use crate::{
    capture::{ReadCapture, WriteCapture},
    id::{ModuleId, ServiceId},
    spier::{ObservedInvocation, ObservedMessage},
};

pub const CAPTURE_STREAM_NAME: &str = "wash-capture";

#[derive(Debug, Parser, Clone)]
pub struct CaptureCommand {
    /// Enable wash capture. This will setup a NATS JetStream stream to capture all invocations
    #[clap(name = "enable", long = "enable", conflicts_with = "disable")]
    pub enable: bool,

    /// Disable wash capture. This will removed the NATS JetStream stream that was setup to capture
    /// all invocations
    #[clap(name = "disable", long = "disable", conflicts_with = "enable")]
    pub disable: bool,

    /// The length of time in minutes to keep messages in the stream.
    #[clap(name = "window_size", long = "window-size", default_value = "60")]
    pub window_size_minutes: u64,

    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Replay through a stream of captured invocations
    #[clap(subcommand)]
    pub replay: Option<CaptureSubcommand>,
}

#[derive(Debug, Subcommand, Clone)]
pub enum CaptureSubcommand {
    Replay(CaptureReplayCommand),
}

#[derive(Debug, Parser, Clone)]
pub struct CaptureReplayCommand {
    /// An actor ID to filter captured invocations by. This will filter anywhere the actor is the
    /// source or the target of the invocation. If provided with an provider ID, it will filter down
    /// to interactions only between the actor and provider
    #[clap(name = "actor_id", long = "actor-id", value_parser)]
    pub actor_id: Option<ModuleId>,

    /// A provider ID to filter captured invocations by. This will filter anywhere the provider is
    /// the source or the target of the invocation. If provided with an actor ID, it will filter
    /// down to interactions only between the actor and provider
    #[clap(name = "provider_id", long = "provider-id", value_parser)]
    pub provider_id: Option<ServiceId>,

    /// Whether or not to step through the replay one message at a time
    #[clap(name = "interactive", long = "interactive")]
    pub interactive: bool,

    /// The file path to the capture file to read from
    #[clap(name = "capturefile")]
    pub capture_file_path: PathBuf,
}

pub async fn handle_replay_command(cmd: CaptureReplayCommand) -> Result<CommandOutput> {
    let capture = ReadCapture::load(cmd.capture_file_path).await?;

    let filtered = capture.messages.into_iter().filter_map(|msg| {
        let mut inv: Invocation = rmp_serde::from_slice(&msg.payload).ok()?;

        if let Some(actor_id) = &cmd.actor_id {
            if (inv.origin.is_actor() && inv.origin.public_key != actor_id.as_ref())
                || (inv.target.is_actor() && inv.target.public_key != actor_id.as_ref())
            {
                return None;
            }
        }

        if let Some(provider_id) = &cmd.provider_id {
            if (inv.origin.is_provider() && inv.origin.public_key != provider_id.as_ref())
                || (inv.target.is_provider() && inv.target.public_key != provider_id.as_ref())
            {
                return None;
            }
        }

        let body = inv.msg;
        inv.msg = Vec::new();
        let from = inv.origin.public_key.clone();
        let to = inv.target.public_key.clone();

        Some((
            ObservedInvocation {
                invocation: inv,
                timestamp: chrono::Local::now(),
                from,
                to,
                message: ObservedMessage::parse(body),
            },
            msg.published,
        ))
    });

    let mut out = stdout();
    for (msg, published) in filtered {
        println!(
            r#"
[{}]
From: {}  To: {}  Host: {}

Operation: {}
Message: {}"#,
            published,
            msg.from,
            msg.to,
            msg.invocation.host_id,
            msg.invocation.operation,
            msg.message
        );
        if cmd.interactive {
            out.write_all(b"Press Enter to continue...").await.unwrap();
            out.flush().await.unwrap();
            stdin().read_exact(&mut [0]).await.unwrap();
        }
    }
    Ok(CommandOutput::default())
}

/// Handles the spy command, printing all output to stdout until the command is interrupted
pub async fn handle_command(cmd: CaptureCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let nats_client = wco.clone().into_nats_client().await?;
    let ctl_client = wco.clone().into_ctl_client(None).await?;
    let js_context = if let Some(domain) = wco.js_domain {
        async_nats::jetstream::with_domain(nats_client, domain)
    } else {
        async_nats::jetstream::new(nats_client)
    };

    if cmd.enable {
        let window_size = Duration::from_secs(cmd.window_size_minutes * 60);
        return enable(
            js_context,
            wco.lattice.as_deref().unwrap_or("default"),
            window_size,
        )
        .await;
    } else if cmd.disable {
        return disable(js_context, wco.lattice.as_deref().unwrap_or("default")).await;
    }

    capture(
        js_context,
        ctl_client,
        wco.lattice.as_deref().unwrap_or("default"),
    )
    .await
}

pub async fn enable(
    ctx: async_nats::jetstream::Context,
    lattice_id: &str,
    window_size: Duration,
) -> Result<CommandOutput> {
    // Until we get concrete errors, we should check for the stream and if it exists return a nice message that we're already enabled
    if ctx.get_stream(CAPTURE_STREAM_NAME).await.is_ok() {
        return Ok(CommandOutput::from_key_and_text(
            "message",
            format!("Capture is already enabled for lattice {lattice_id}"),
        ));
    }
    ctx.create_stream(Config {
        name: stream_name(lattice_id),
        storage: async_nats::jetstream::stream::StorageType::File,
        max_age: window_size,
        // This needs to be set or it breaks invocations
        no_ack: true,
        subjects: vec![format!("wasmbus.rpc.{}.>", lattice_id)],
        ..Default::default()
    })
    .await
    .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    Ok(CommandOutput::from_key_and_text(
        "message",
        "Successfully enabled capture mode for lattice",
    ))
}

pub async fn disable(
    ctx: async_nats::jetstream::Context,
    lattice_id: &str,
) -> Result<CommandOutput> {
    ctx.delete_stream(stream_name(lattice_id))
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    Ok(CommandOutput::from_key_and_text(
        "message",
        "Successfully disabled capture mode for lattice",
    ))
}

pub async fn capture(
    ctx: async_nats::jetstream::Context,
    ctl_client: wasmcloud_control_interface::Client,
    lattice_id: &str,
) -> Result<CommandOutput> {
    let stream = ctx.get_stream(stream_name(lattice_id)).await.map_err(|e| {
        anyhow::anyhow!("Unable to find stream. Have you run `wash capture --enable`? Error: {e:?}")
    })?;

    // Timestamp for cutoff of messages to capture
    let capture_start_time = time::OffsetDateTime::now_utc();

    let inventory = get_all_inventory(&ctl_client).await?;

    let consumer = stream
        .create_consumer(ConsumerConfig {
            description: Some("Wash capture consumer".to_string()),
            deliver_policy: DeliverPolicy::All,
            ack_policy: AckPolicy::None,
            ..Default::default()
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    let mut messages = consumer
        .messages()
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    let max_time_without_message = Duration::from_secs(1);
    let mut expiry = tokio::time::interval_at(
        Instant::now() + max_time_without_message,
        max_time_without_message,
    );

    let filename = format!(
        "{}.{}.washcapture",
        chrono::Local::now().to_rfc3339(),
        lattice_id
    );
    let mut capture = WriteCapture::start(inventory, &filename).await?;

    loop {
        tokio::select! {
            _ = expiry.tick() => {
                println!("No messages received in the last second. Ending capture");
                break
            },
            res = messages.try_next() => {
                // If we get a message, reset the tick
                expiry.reset();
                let msg = match res {
                    Ok(None) => {
                        eprintln!("WARN: Message stream ended early");
                        break;
                    }
                    Ok(Some(m)) => m,
                    Err(_) => {
                        continue;
                    }
                };
                if let Ok(info) = msg.info() {
                    if info.published > capture_start_time {
                        println!("Reached end of capture");
                        break;
                    }
                }
                if let Ok(m) = msg.try_into() {
                    capture.add_message(m).await?;
                }
            }
        }
    }

    capture.finish().await?;

    Ok(CommandOutput::new(
        format!("Completed capture and output to file {filename}"),
        [
            (
                "message".to_string(),
                serde_json::Value::String("Completed capture".to_owned()),
            ),
            (
                "output_path".to_string(),
                serde_json::Value::String(filename),
            ),
        ]
        .into(),
    ))
}

async fn get_all_inventory(
    ctl_client: &wasmcloud_control_interface::Client,
) -> anyhow::Result<Vec<wasmcloud_control_interface::HostInventory>> {
    let futs = ctl_client
        .get_hosts()
        .await
        .map_err(|e| anyhow::anyhow!("{e:?}"))?
        .into_iter()
        .filter_map(|host| host.response.map(|host| (ctl_client.clone(), host.id)))
        .map(|(client, host_id)| async move {
            let inventory = client
                .get_host_inventory(&host_id)
                .await
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            Ok(inventory.response)
        });
    futures::future::join_all(futs)
        .await
        .into_iter()
        .filter_map(Result::transpose)
        .collect()
}

fn stream_name(lattice_id: &str) -> String {
    format!("{}-{lattice_id}", CAPTURE_STREAM_NAME)
}

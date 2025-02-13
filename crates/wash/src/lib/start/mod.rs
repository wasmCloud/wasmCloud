//! The `start` module contains functionality relating to downloading and starting
//! NATS servers and wasmCloud hosts.
//!
//! # Downloading and Starting NATS and wasmCloud
//! ```no_run
//! use anyhow::{anyhow, Result};
//! use crate::lib::common::CommandGroupUsage;
//! use crate::lib::start::{
//!     start_wasmcloud_host,
//!     start_nats_server,
//!     ensure_nats_server,
//!     ensure_wasmcloud,
//!     NatsConfig
//! };
//! use std::path::PathBuf;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let install_dir = PathBuf::from("/tmp");
//!
//!     // Download NATS if not already installed
//!     let nats_binary = ensure_nats_server("v2.10.7", &install_dir).await?;
//!
//!     // Start NATS server, redirecting output to a log file
//!     let nats_log_path = install_dir.join("nats.log");
//!     let nats_log_file = tokio::fs::File::create(&nats_log_path).await?.into_std().await;
//!     let config = NatsConfig::new_standalone("127.0.0.1", 4222, None);
//!     let mut nats_process = start_nats_server(
//!         nats_binary,
//!         nats_log_file,
//!         config,
//!         CommandGroupUsage::UseParent,
//!     ).await?;
//!
//!     // Download wasmCloud if not already installed
//!     let wasmcloud_executable = ensure_wasmcloud("v0.57.1", &install_dir).await?;
//!
//!     // Redirect output (which is on stderr) to a log file
//!     let log_path = install_dir.join("wasmcloud_stderr.log");
//!     let log_file = tokio::fs::File::create(&log_path).await?.into_std().await;
//!
//!     let mut wasmcloud_process = start_wasmcloud_host(
//!         wasmcloud_executable,
//!         std::process::Stdio::null(),
//!         log_file,
//!         std::collections::HashMap::new(),
//!     ).await?;
//!
//!     // Park thread, wasmCloud and NATS are running
//!
//!     // Terminate processes
//!     nats_process.kill().await?;
//!     wasmcloud_process.kill().await?;
//!     Ok(())
//! }
//! ```

use anyhow::Result;
use tracing::debug;

pub async fn wait_for_server(url: &str, service: &str) -> Result<()> {
    let mut wait_count = 1;
    loop {
        // Magic number: 10 + 1, since we are starting at 1 for humans
        if wait_count >= 11 {
            anyhow::bail!("Ran out of retries waiting for {service} to start");
        }
        match tokio::net::TcpStream::connect(url).await {
            Ok(_) => break,
            Err(e) => {
                debug!("Waiting for {service} at {url} to come up, attempt {wait_count}. Will retry in 1 second. Got error {:?}", e);
                wait_count += 1;
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
    Ok(())
}

mod github;
pub use github::*;
mod nats;
pub use nats::*;
mod wadm;
pub use self::wadm::*;
mod wasmcloud;
pub use wasmcloud::*;

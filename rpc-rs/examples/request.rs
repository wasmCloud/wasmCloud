use anyhow::{anyhow, Result};
use clap::Parser;
use nkeys::KeyPairType;
use std::path::PathBuf;
use std::sync::Arc;
use wascap::prelude::KeyPair;
use wasmbus_rpc::rpc_client::RpcClient;

/// RpcClient test CLI for making nats request
#[derive(Parser)]
#[clap(version, about, long_about = None)]
struct Args {
    /// Nats uri. Defaults to 'nats://127.0.0.1:4222'
    #[clap(short, long)]
    nats: Option<String>,

    /// File source for payload
    #[clap(short, long)]
    file: Option<PathBuf>,

    /// Raw data for payload (as string)
    #[clap(short, long)]
    data: Option<String>,

    /// Optional timeout in milliseconds
    #[clap(short, long)]
    timeout_ms: Option<u32>,

    /// Subject (topic)
    #[clap(value_parser)]
    subject: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let data = match (args.data, args.file) {
        (Some(d), None) => d.as_bytes().to_vec(),
        (None, Some(f)) => {
            if !f.is_file() {
                return Err(anyhow!("missing data file {}", f.display()));
            }
            std::fs::read(&f)
                .map_err(|e| anyhow!("error reading data source (path={}): {}", f.display(), e))?
        }
        _ => {
            return Err(anyhow!("please specify --file or --data for data source"));
        }
    };
    if args.subject.is_empty() {
        return Err(anyhow!("subject may not be empty"));
    }

    let timeout = args.timeout_ms.map(|n| std::time::Duration::from_millis(n as u64));
    let kp = Arc::new(KeyPair::new(KeyPairType::User));
    let nats_uri = args.nats.unwrap_or_else(|| "nats://127.0.0.1:4222".to_string());
    let nc = async_nats::connect(&nats_uri).await?;
    let client = RpcClient::new(nc, "HOST".into(), timeout, kp);

    let resp = client.request(args.subject, data).await?;
    println!("{}", String::from_utf8_lossy(&resp));
    Ok(())
}

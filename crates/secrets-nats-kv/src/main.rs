use std::path::PathBuf;

use anyhow::{ensure, Context};
use clap::ValueEnum;
use clap::{Parser, Subcommand};
use nkeys::XKey;
use secrets_nats_kv::client::SECRETS_API_VERSION;
use secrets_nats_kv::Api;

use secrets_nats_kv::client;
use secrets_nats_kv::PutSecretRequest;

#[derive(Parser)]
#[command(about, version, name = "secrets-nats-kv")]
/// A secrets backend for wasmCloud that uses NATS as a key-value store. Included in this CLI
/// are commands to run the secrets backend and to manage secrets in a running backend instance
struct Args {
    #[command(name = "command", subcommand)]
    command: Command,
}

#[derive(Parser, Clone, Debug)]
struct GlobalOpts {
    #[clap(long, env = "NATS_CREDSFILE")]
    nats_creds_file: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the NATS KV secrets backend
    Run(RunCommand),
    /// Put a secret into the NATS KV secrets backend
    Put(PutCommand),
    /// Get a secret from the NATS KV secrets backend
    Get(GetCommand),
    /// Add a secret mapping to the NATS KV secrets backend
    AddMapping(AddSecretMappingCommand),
    /// Remove a secret mapping from the NATS KV secrets backend
    RemoveMapping(RemoveSecretMappingCommand),
}

#[derive(Parser)]
struct RunCommand {
    /// The server's encryption XKey, used to encrypt secrets before storing in NATS.
    #[clap(short, long, env = "ENCRYPTION_XKEY_SEED")]
    encryption_xkey_seed: String,
    /// The server's transit XKey, used to decrypt secrets sent to the server.
    #[clap(short, long, env = "TRANSIT_XKEY_SEED")]
    transit_xkey_seed: String,
    /// The subject prefix to use for all requests to the secrets backend, defaults to `wasmcloud.secrets`
    #[clap(short, long, default_value = "wasmcloud.secrets")]
    subject_base: String,
    /// The name of the secrets backend, defaults to `nats-kv`
    #[clap(short = 'n', long, default_value = "nats-kv")]
    name: String,
    /// The NATS KV bucket to use for storing secrets
    #[clap(short = 'b', long, default_value = "WASMCLOUD_SECRETS")]
    secrets_bucket: String,
    /// The maximum number of versions to keep for each secret
    #[clap(long, default_value = "64")]
    max_secret_history: usize,
    /// The NATS queue group to use for running multiple instances of the secrets backend
    #[clap(long, default_value = "wasmcloud_secrets")]
    nats_queue_base: String,
    /// The NATS address to connect to where the backend is running
    #[clap(long, default_value = "127.0.0.1:4222")]
    nats_address: String,
    /// The API version to use for the secrets backend
    #[clap(long, default_value = SECRETS_API_VERSION)]
    secrets_api_version: String,

    #[command(flatten)]
    global: GlobalOpts,
}

#[derive(Parser, Debug, Clone)]
struct PutCommand {
    /// The server's transit XKey, used to decrypt secrets sent to the server.
    #[clap(short, long, env = "TRANSIT_XKEY_SEED")]
    transit_xkey_seed: String,
    /// The subject prefix to use for all requests to the secrets backend, defaults to `wasmcloud.secrets`
    #[clap(short, long, default_value = "wasmcloud.secrets")]
    subject_base: String,
    /// The NATS address to connect to where the backend is running
    #[clap(long, default_value = "127.0.0.1:4222")]
    nats_address: String,
    /// The name of the secret to put in the backend
    name: String,
    /// The version of the secret to put in the backend
    #[clap(long = "secret-version")]
    version: Option<String>,
    /// The string value of the secret to put in the backend
    #[clap(
        long,
        env = "SECRET_STRING_VALUE",
        required_unless_present = "binary",
        conflicts_with = "binary"
    )]
    string: Option<String>,
    /// The path to a file to read the binary value of the secret to put in the backend
    #[clap(
        long,
        env = "SECRET_BINARY_FILE",
        required_unless_present = "string",
        conflicts_with = "string"
    )]
    binary: Option<PathBuf>,

    #[command(flatten)]
    global: GlobalOpts,
}

#[derive(Parser, Debug, Clone, ValueEnum)]
enum SecretKind {
    String,
    Binary,
}

#[derive(Parser, Debug, Clone)]
struct GetCommand {
    /// The server's encryption XKey to decrypt the secret fetched from the KV store.
    #[clap(short, long, env = "ENCRYPTION_XKEY_SEED")]
    encryption_xkey_seed: String,
    /// The NATS address to connect to where the backend is running
    #[clap(long, default_value = "127.0.0.1:4222")]
    nats_address: String,
    /// The NATS KV bucket to use for storing secrets
    #[clap(short = 'b', long, default_value = "WASMCLOUD_SECRETS")]
    secrets_bucket: String,
    /// The name of the secret to get from the backend
    name: String,
    /// The version of the secret to get from the backend
    #[clap(long = "secret-version")]
    version: Option<String>,
    /// The kind of secret to get from the backend, `string` or `binary`
    #[clap(long, default_value = "string")]
    kind: SecretKind,
    #[command(flatten)]
    global: GlobalOpts,
}

#[derive(Parser, Debug, Clone)]
struct AddSecretMappingCommand {
    /// The NATS address to connect to where the backend is running
    #[clap(long, default_value = "127.0.0.1:4222")]
    nats_address: String,
    /// The subject prefix to use for all requests to the secrets backend, defaults to `wasmcloud.secrets`
    #[clap(short, long, default_value = "wasmcloud.secrets")]
    subject_base: String,
    /// The public key identity of the entity that is allowed to access the secrets
    public_key: String,
    /// The names of the secrets that the public key is allowed to access. Can be specified multiple times.
    #[clap(long = "secret")]
    secrets: Vec<String>,

    #[command(flatten)]
    global: GlobalOpts,
}

#[derive(Parser, Debug, Clone)]
struct RemoveSecretMappingCommand {
    /// The NATS address to connect to where the backend is running
    #[clap(long, default_value = "127.0.0.1:4222")]
    nats_address: String,
    /// The subject prefix to use for all requests to the secrets backend, defaults to `wasmcloud.secrets`
    #[clap(short, long, default_value = "wasmcloud.secrets")]
    subject_base: String,
    /// The public key identity of the entity that is allowed to access the secrets
    public_key: String,
    /// The names of the secrets that the public key should no longer be able to access. Can be specified multiple times.
    #[clap(long = "secret")]
    secrets: Vec<String>,

    #[command(flatten)]
    global: GlobalOpts,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    match args.command {
        Command::Run(args) => run(args).await,
        Command::Put(args) => put(args).await,
        Command::Get(args) => get(args).await,
        Command::AddMapping(args) => add_mapping(args).await,
        Command::RemoveMapping(args) => remove_mapping(args).await,
    }
}

async fn run(args: RunCommand) -> anyhow::Result<()> {
    let server_xkey = XKey::from_seed(&args.transit_xkey_seed)
        .context("failed to create server key from seed")?;
    let encryption_xkey = XKey::from_seed(&args.encryption_xkey_seed)
        .context("failed to create encryption key from seed")?;

    let nats_client = match args.global.nats_creds_file {
        Some(creds_file) => async_nats::ConnectOptions::new()
            .name("secrets-nats-kv")
            .credentials_file(creds_file.clone())
            .await
            .context(format!(
                "failed to read NATS credentials file '{}'",
                &creds_file
            ))?
            .connect(&args.nats_address)
            .await
            .with_context(|| {
                format!(
                    "failed to connect to NATS at {} with credentials file '{}'",
                    args.nats_address, creds_file
                )
            })?,
        None => async_nats::connect(&args.nats_address)
            .await
            .with_context(|| format!("failed to connect to NATS at {}", args.nats_address))?,
    };

    let api = Api::new(
        server_xkey,
        encryption_xkey,
        nats_client,
        args.subject_base,
        args.name.clone(),
        args.secrets_bucket,
        args.max_secret_history,
        args.nats_queue_base,
        args.secrets_api_version,
    );

    println!("Starting secrets backend '{}'", args.name);
    api.run().await
}

async fn put(args: PutCommand) -> anyhow::Result<()> {
    let server_xkey = XKey::from_seed(&args.transit_xkey_seed)
        .context("failed to create server key from seed")?;

    let nats_client = match args.global.nats_creds_file {
        Some(creds_file) => async_nats::ConnectOptions::new()
            .credentials_file(creds_file.clone())
            .await
            .context(format!(
                "failed to read NATS credentials file '{}'",
                &creds_file
            ))?
            .name("secrets-nats-kv")
            .connect(&args.nats_address)
            .await
            .with_context(|| {
                format!(
                    "failed to connect to NATS at {} with credentials file '{}'",
                    args.nats_address, creds_file
                )
            })?,
        None => async_nats::ConnectOptions::new()
            .name("secrets-nats-kv")
            .connect(&args.nats_address)
            .await
            .with_context(|| format!("failed to connect to NATS at {}", args.nats_address))?,
    };

    let binary_secret = args
        .binary
        .map(|path| {
            std::fs::read(&path).with_context(|| {
                format!(
                    "failed to read binary secret from file '{}'",
                    path.display()
                )
            })
        })
        .transpose()?;

    let name = args.name.clone();
    let secret = PutSecretRequest {
        key: name,
        version: args.version.unwrap_or_default(),
        // NOTE: The clap parser will ensure that one and only one of these is present
        string_secret: args.string,
        binary_secret,
    };

    client::put_secret(&nats_client, &args.subject_base, &server_xkey, secret).await?;
    println!("Secret '{}' put successfully", args.name);
    Ok(())
}

async fn get(args: GetCommand) -> anyhow::Result<()> {
    let nats_client = match args.global.nats_creds_file {
        Some(creds_file) => async_nats::ConnectOptions::new()
            .credentials_file(creds_file.clone())
            .await
            .context(format!(
                "failed to read NATS credentials file '{}'",
                &creds_file
            ))?
            .name("secrets-nats-kv")
            .connect(&args.nats_address)
            .await
            .with_context(|| {
                format!(
                    "failed to connect to NATS at {} with credentials file '{creds_file}'",
                    args.nats_address
                )
            })?,
        None => async_nats::ConnectOptions::new()
            .name("secrets-nats-kv")
            .connect(&args.nats_address)
            .await
            .with_context(|| format!("failed to connect to NATS at {}", args.nats_address))?,
    };

    let encryption_xkey = XKey::from_seed(&args.encryption_xkey_seed)
        .context("failed to create encryption key from seed")?;

    let secret = client::get_secret(
        &nats_client,
        &args.secrets_bucket,
        &encryption_xkey,
        &args.name,
        args.version.as_deref(),
    )
    .await?;

    match (args.kind, secret.string_secret, secret.binary_secret) {
        (SecretKind::String, Some(s), None) => println!("{s}"),
        (SecretKind::Binary, None, Some(b)) => {
            println!("{b:?}");
        }
        (SecretKind::String, None, Some(_)) => {
            anyhow::bail!("secret was in binary format, but string format was requested")
        }
        (SecretKind::Binary, Some(_), None) => {
            anyhow::bail!("secret was in string format, but binary format was requested")
        }
        _ => anyhow::bail!("no secret found in KV store"),
    }

    Ok(())
}

async fn add_mapping(args: AddSecretMappingCommand) -> anyhow::Result<()> {
    ensure!(
        !args.secrets.is_empty(),
        "at least one secret must be provided to add a mapping"
    );

    let nats_client = match args.global.nats_creds_file {
        Some(creds_file) => async_nats::ConnectOptions::new()
            .credentials_file(creds_file.clone())
            .await
            .context(format!(
                "failed to read NATS credentials file '{}'",
                &creds_file
            ))?
            .name("secrets-nats-kv")
            .connect(&args.nats_address)
            .await
            .with_context(|| {
                format!(
                    "failed to connect to NATS at {} with credentials file '{}'",
                    args.nats_address, creds_file
                )
            })?,
        None => async_nats::ConnectOptions::new()
            .name("secrets-nats-kv")
            .connect(&args.nats_address)
            .await
            .with_context(|| format!("failed to connect to NATS at {}", args.nats_address))?,
    };

    client::add_mapping(
        &nats_client,
        &args.subject_base,
        &args.public_key,
        args.secrets.clone().into_iter().collect(),
    )
    .await?;
    println!(
        "Public key '{}' can now access secrets: {:?}",
        args.public_key, args.secrets
    );
    Ok(())
}

async fn remove_mapping(args: RemoveSecretMappingCommand) -> anyhow::Result<()> {
    ensure!(
        !args.secrets.is_empty(),
        "at least one secret must be provided to remove a mapping"
    );

    let nats_client = match args.global.nats_creds_file {
        Some(creds_file) => async_nats::ConnectOptions::new()
            .credentials_file(creds_file.clone())
            .await
            .context(format!(
                "failed to read NATS credentials file '{}'",
                &creds_file
            ))?
            .name("secrets-nats-kv")
            .connect(&args.nats_address)
            .await
            .with_context(|| {
                format!(
                    "failed to connect to NATS at {} with credentials file '{}'",
                    args.nats_address, creds_file
                )
            })?,
        None => async_nats::ConnectOptions::new()
            .name("secrets-nats-kv")
            .connect(&args.nats_address)
            .await
            .with_context(|| format!("failed to connect to NATS at {}", args.nats_address))?,
    };

    client::remove_mapping(
        &nats_client,
        &args.subject_base,
        &args.public_key,
        args.secrets.clone().into_iter().collect(),
    )
    .await?;

    println!(
        "Public key '{}' no longer has access to secrets: {:?}",
        args.public_key, args.secrets
    );
    Ok(())
}

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use nkeys::{KeyPair, KeyPairType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
};
use tracing::warn;
use wascap::{
    jwt::{Account, Actor, CapabilityProvider, Claims, Operator},
    wasm::{days_from_now_to_jwt_time, sign_buffer_with_claims},
};

use super::{extract_keypair, get::GetClaimsCommand, CommandOutput, OutputKind};
use crate::{
    cli::inspect,
    common::boxed_err_to_anyhow,
    config::WashConnectionOptions,
    parser::{get_config, ActorConfig, ProjectConfig, ProviderConfig, TypeConfig},
};

#[derive(Debug, Clone, Subcommand)]
pub enum ClaimsCliCommand {
    /// Examine the capabilities of a WebAssembly module
    #[clap(name = "inspect")]
    Inspect(InspectCommand),
    /// Sign a WebAssembly module, specifying capabilities and other claims
    /// including expiration, tags, and additional metadata
    #[clap(name = "sign")]
    Sign(SignCommand),
    /// Generate a signed JWT by supplying basic token information, a signing seed key, and metadata
    #[clap(name = "token", subcommand)]
    Token(TokenCommand),
}

#[derive(Args, Debug, Clone)]
pub struct InspectCommand {
    /// Path to signed actor module or OCI URL of signed actor module
    pub module: String,

    /// Extract the raw JWT from the file and print to stdout
    #[clap(name = "jwt_only", long = "jwt-only")]
    pub(crate) jwt_only: bool,

    /// Extract the WIT world from a component and print to stdout instead of the claims
    #[clap(name = "wit", long = "wit", alias = "world")]
    pub wit: bool,

    /// Digest to verify artifact against (if OCI URL is provided for <module>)
    #[clap(short = 'd', long = "digest")]
    pub(crate) digest: Option<String>,

    /// Allow latest artifact tags (if OCI URL is provided for <module>)
    #[clap(long = "allow-latest")]
    pub(crate) allow_latest: bool,

    /// OCI username, if omitted anonymous authentication will be used
    #[clap(
        short = 'u',
        long = "user",
        env = "WASH_REG_USER",
        hide_env_values = true
    )]
    pub(crate) user: Option<String>,

    /// OCI password, if omitted anonymous authentication will be used
    #[clap(
        short = 'p',
        long = "password",
        env = "WASH_REG_PASSWORD",
        hide_env_values = true
    )]
    pub(crate) password: Option<String>,

    /// Allow insecure (HTTP) registry connections
    #[clap(long = "insecure")]
    pub(crate) insecure: bool,

    /// skip the local OCI cache
    #[clap(long = "no-cache")]
    pub(crate) no_cache: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct SignCommand {
    /// File to read
    pub source: String,

    /// Destination for signed module. If this flag is not provided, the signed module will be placed in the same directory as the source with a "_s" suffix
    #[clap(short = 'd', long = "destination")]
    pub destination: Option<String>,

    #[clap(flatten)]
    pub metadata: ActorMetadata,
}

#[derive(Debug, Clone, Subcommand)]
pub enum TokenCommand {
    /// Generate a signed JWT for an actor module
    #[clap(name = "actor")]
    Actor(ActorMetadata),
    /// Generate a signed JWT for an operator
    #[clap(name = "operator")]
    Operator(OperatorMetadata),
    /// Generate a signed JWT for an account
    #[clap(name = "account")]
    Account(AccountMetadata),
    /// Generate a signed JWT for a service (capability provider)
    #[clap(name = "provider")]
    Provider(ProviderMetadata),
}

#[derive(Debug, Clone, Parser, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct GenerateCommon {
    /// Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
    #[clap(long = "directory", env = "WASH_KEYS", hide_env_values = true)]
    pub directory: Option<PathBuf>,

    /// Indicates the token expires in the given amount of days. If this option is left off, the token will never expire
    #[clap(short = 'x', long = "expires")]
    pub expires_in_days: Option<u64>,

    /// Period in days that must elapse before this token is valid. If this option is left off, the token will be valid immediately
    #[clap(short = 'b', long = "nbf")]
    pub not_before_days: Option<u64>,

    /// Disables autogeneration of keys if seed(s) are not provided
    #[clap(long = "disable-keygen")]
    pub disable_keygen: bool,
}

#[derive(Debug, Clone, Parser)]
pub struct OperatorMetadata {
    /// A descriptive name for the operator
    #[clap(short = 'n', long = "name")]
    name: String,

    /// Path to issuer seed key (self signing operator). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 'i',
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    issuer: Option<String>,

    /// Additional keys to add to valid signers list
    /// Can either be seed value or path to seed file
    #[clap(short = 'a', long = "additional-key", name = "additional-keys")]
    additional_signing_keys: Option<Vec<String>>,

    #[clap(flatten)]
    common: GenerateCommon,
}

#[derive(Debug, Clone, Parser)]
pub struct AccountMetadata {
    /// A descriptive name for the account
    #[clap(short = 'n', long = "name")]
    name: String,

    /// Path to issuer seed key (operator). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 'i',
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    issuer: Option<String>,

    /// Path to subject seed key (account). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 's',
        long = "subject",
        env = "WASH_SUBJECT_KEY",
        hide_env_values = true
    )]
    subject: Option<String>,

    /// Additional keys to add to valid signers list.
    /// Can either be seed value or path to seed file
    #[clap(short = 'a', long = "additional-key", name = "additional-keys")]
    additional_signing_keys: Option<Vec<String>>,

    #[clap(flatten)]
    common: GenerateCommon,
}

#[derive(Debug, Clone, Parser, Default, PartialEq, Eq)]
pub struct ProviderMetadata {
    /// A descriptive name for the provider
    #[clap(short = 'n', long = "name")]
    name: Option<String>,

    /// Capability contract ID that this provider supports
    #[clap(short = 'c', long = "capid")]
    capid: Option<String>,

    /// A human-readable string identifying the vendor of this provider (e.g. Redis or Cassandra or NATS etc)
    #[clap(short = 'v', long = "vendor")]
    vendor: Option<String>,

    /// Monotonically increasing revision number
    #[clap(short = 'r', long = "revision")]
    revision: Option<i32>,

    /// Human-friendly version string
    #[clap(short = 'e', long = "version")]
    version: Option<String>,

    /// Path to issuer seed key (account). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 'i',
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    issuer: Option<String>,

    /// Path to subject seed key (service). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 's',
        long = "subject",
        env = "WASH_SUBJECT_KEY",
        hide_env_values = true
    )]
    subject: Option<String>,

    #[clap(flatten)]
    common: GenerateCommon,
}

impl ProviderMetadata {
    pub fn update_with_project_config(self, project_config: &ProjectConfig) -> Self {
        let provider_config = match project_config.project_type {
            TypeConfig::Provider(ref provider_config) => provider_config.clone(),
            _ => ProviderConfig::default(),
        };

        ProviderMetadata {
            name: self.name.or(Some(project_config.common.name.clone())),
            revision: self.revision.or(Some(project_config.common.revision)),
            version: self
                .version
                .or(Some(project_config.common.version.to_string())),
            capid: self.capid.or(Some(provider_config.capability_id)),
            vendor: self.vendor.or(Some(provider_config.vendor)),
            ..self
        }
    }
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ActorMetadata {
    /// Enable the Key/Value Store standard capability
    #[clap(short = 'k', long = "keyvalue")]
    pub keyvalue: bool,
    /// Enable the Message broker standard capability
    #[clap(short = 'g', long = "msg")]
    pub msg_broker: bool,
    /// Enable the HTTP server standard capability
    #[clap(short = 'q', long = "http_server")]
    pub http_server: bool,
    /// Enable the HTTP client standard capability
    #[clap(long = "http_client")]
    pub http_client: bool,
    /// Enable access to the blob store capability
    #[clap(short = 'f', long = "blob_store")]
    pub blob_store: bool,
    /// Enable access to the extras functionality (random nos, guids, etc)
    #[clap(short = 'z', long = "extras")]
    pub extras: bool,
    /// Enable access to logging capability
    #[clap(short = 'l', long = "logging")]
    pub logging: bool,
    /// Enable access to an append-only event stream provider
    #[clap(short = 'e', long = "events")]
    pub eventstream: bool,
    /// A human-readable, descriptive name for the token
    #[clap(short = 'n', long = "name")]
    pub name: Option<String>,
    /// Add custom capabilities
    #[clap(short = 'c', long = "cap", name = "capabilities")]
    pub custom_caps: Vec<String>,
    /// A list of arbitrary tags to be embedded in the token
    #[clap(short = 't', long = "tag")]
    pub tags: Vec<String>,
    /// Revision number
    #[clap(short = 'r', long = "rev")]
    pub rev: Option<i32>,
    /// Human-readable version string
    #[clap(short = 'v', long = "ver")]
    pub ver: Option<String>,
    /// Developer or human friendly unique alias used for invoking an actor, consisting of lowercase alphanumeric characters, underscores '_' and slashes '/'
    #[clap(short = 'a', long = "call-alias")]
    pub call_alias: Option<String>,

    /// Path to issuer seed key (account). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 'i',
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    pub issuer: Option<String>,

    /// Path to subject seed key (module). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 's',
        long = "subject",
        env = "WASH_SUBJECT_KEY",
        hide_env_values = true
    )]
    pub subject: Option<String>,

    #[clap(flatten)]
    pub common: GenerateCommon,
}

impl ActorMetadata {
    pub fn update_with_project_config(self, project_config: &ProjectConfig) -> Self {
        let actor_config = match project_config.project_type {
            TypeConfig::Actor(ref actor_config) => actor_config.clone(),
            _ => ActorConfig::default(),
        };

        let mut standard_caps = HashMap::from([
            (wascap::caps::KEY_VALUE.to_string(), self.keyvalue),
            (wascap::caps::MESSAGING.to_string(), self.msg_broker),
            (wascap::caps::HTTP_CLIENT.to_string(), self.http_client),
            (wascap::caps::HTTP_SERVER.to_string(), self.http_server),
            (wascap::caps::BLOB.to_string(), self.blob_store),
            (wascap::caps::LOGGING.to_string(), self.logging),
            (wascap::caps::EVENTSTREAMS.to_string(), self.eventstream),
        ]);

        let mut custom_caps = self
            .custom_caps
            .clone()
            .into_iter()
            .collect::<BTreeSet<String>>();

        for cap in actor_config.claims.iter() {
            if let Some(flag) = standard_caps.get_mut(cap) {
                *flag = true;
            } else {
                custom_caps.insert(cap.clone());
            }
        }

        ActorMetadata {
            name: self.name.or(Some(project_config.common.name.clone())),
            rev: self.rev.or(Some(project_config.common.revision)),
            ver: self.ver.or(Some(project_config.common.version.to_string())),
            keyvalue: *(standard_caps
                .get(wascap::caps::KEY_VALUE)
                .unwrap_or(&self.keyvalue)),
            msg_broker: *(standard_caps
                .get(wascap::caps::MESSAGING)
                .unwrap_or(&self.msg_broker)),
            http_server: *(standard_caps
                .get(wascap::caps::HTTP_SERVER)
                .unwrap_or(&self.http_server)),
            http_client: *(standard_caps
                .get(wascap::caps::HTTP_CLIENT)
                .unwrap_or(&self.http_client)),
            blob_store: *(standard_caps
                .get(wascap::caps::BLOB)
                .unwrap_or(&self.blob_store)),
            logging: *(standard_caps
                .get(wascap::caps::LOGGING)
                .unwrap_or(&self.logging)),
            eventstream: *(standard_caps
                .get(wascap::caps::EVENTSTREAMS)
                .unwrap_or(&self.eventstream)),
            custom_caps: custom_caps.into_iter().collect(),
            tags: match actor_config.tags.clone() {
                Some(tags) => tags
                    .clone()
                    .into_iter()
                    .collect::<BTreeSet<String>>()
                    .union(&self.tags.clone().into_iter().collect::<BTreeSet<String>>())
                    .cloned()
                    .collect::<Vec<String>>(),
                None => self.tags,
            },
            call_alias: self.call_alias.or(actor_config.call_alias),
            common: GenerateCommon {
                directory: self.common.directory.or(Some(actor_config.key_directory)),
                ..self.common
            },
            ..self
        }
    }
}

impl From<InspectCommand> for inspect::InspectCliCommand {
    fn from(cmd: InspectCommand) -> Self {
        inspect::InspectCliCommand {
            target: cmd.module,
            jwt_only: cmd.jwt_only,
            wit: cmd.wit,
            digest: cmd.digest,
            allow_latest: cmd.allow_latest,
            user: cmd.user,
            password: cmd.password,
            insecure: cmd.insecure,
            no_cache: cmd.no_cache,
        }
    }
}

pub async fn handle_command(
    command: ClaimsCliCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let project_config = get_config(None, Some(true)).ok();
    match command {
        ClaimsCliCommand::Inspect(inspectcmd) => {
            warn!("claims inspect will be deprecated in future versions. Use inspect instead.");
            inspect::handle_command(inspectcmd, output_kind).await
        }
        ClaimsCliCommand::Sign(signcmd) => sign_file(
            SignCommand {
                metadata: match project_config {
                    Some(ref config) => signcmd.metadata.update_with_project_config(config),
                    None => signcmd.metadata,
                },
                destination: match project_config {
                    Some(ref config) => match config.project_type {
                        TypeConfig::Actor(ref actor_config) => signcmd.destination.or(actor_config
                            .destination
                            .clone()
                            .map(|d| {
                                d.to_str()
                                    .expect("unable to convert destination pathbuf to str")
                                    .to_string()
                            })),
                        _ => signcmd.destination,
                    },
                    None => signcmd.destination,
                },
                ..signcmd
            },
            output_kind,
        ),
        ClaimsCliCommand::Token(gencmd) => {
            generate_token(gencmd, output_kind, project_config.as_ref())
        }
    }
}

fn generate_token(
    cmd: TokenCommand,
    output_kind: OutputKind,
    project_config: Option<&ProjectConfig>,
) -> Result<CommandOutput> {
    match (cmd, project_config) {
        (TokenCommand::Actor(actor), Some(config)) => {
            generate_actor(actor.update_with_project_config(config), output_kind)
        }
        (TokenCommand::Provider(provider), Some(config)) => {
            generate_provider(provider.update_with_project_config(config), output_kind)
        }
        (TokenCommand::Actor(actor), _) => generate_actor(actor, output_kind),
        (TokenCommand::Provider(provider), _) => generate_provider(provider, output_kind),
        (TokenCommand::Operator(operator), _) => generate_operator(operator, output_kind),
        (TokenCommand::Account(account), _) => generate_account(account, output_kind),
    }
}

fn get_keypair_vec(
    keys: &[String],
    keys_dir: Option<PathBuf>,
    keypair_type: KeyPairType,
    disable_keygen: bool,
    output_kind: OutputKind,
) -> Vec<KeyPair> {
    keys.iter()
        .map(|k| {
            extract_keypair(
                Some(k.to_string()),
                None,
                keys_dir.clone(),
                keypair_type.clone(),
                disable_keygen,
                output_kind,
            )
            .unwrap()
        })
        .collect()
}

fn generate_actor(actor: ActorMetadata, output_kind: OutputKind) -> Result<CommandOutput> {
    let issuer = extract_keypair(
        actor.issuer.clone(),
        actor.name.clone(),
        actor.common.directory.clone(),
        KeyPairType::Account,
        actor.common.disable_keygen,
        output_kind,
    )?;
    let subject = extract_keypair(
        actor.subject.clone(),
        actor.name.clone(),
        actor.common.directory.clone(),
        KeyPairType::Module,
        actor.common.disable_keygen,
        output_kind,
    )?;

    let mut caps_list = vec![];
    if actor.keyvalue {
        caps_list.push(wascap::caps::KEY_VALUE.to_string());
    }
    if actor.msg_broker {
        caps_list.push(wascap::caps::MESSAGING.to_string());
    }
    if actor.http_client {
        caps_list.push(wascap::caps::HTTP_CLIENT.to_string());
    }
    if actor.http_server {
        caps_list.push(wascap::caps::HTTP_SERVER.to_string());
    }
    if actor.blob_store {
        caps_list.push(wascap::caps::BLOB.to_string());
    }
    if actor.logging {
        caps_list.push(wascap::caps::LOGGING.to_string());
    }
    if actor.eventstream {
        caps_list.push(wascap::caps::EVENTSTREAMS.to_string());
    }
    caps_list.extend(actor.custom_caps.iter().cloned());

    let claims: Claims<Actor> = Claims::<Actor>::with_dates(
        actor.name.context("actor name is required")?,
        issuer.public_key(),
        subject.public_key(),
        Some(caps_list),
        Some(actor.tags.clone()),
        days_from_now_to_jwt_time(actor.common.expires_in_days),
        days_from_now_to_jwt_time(actor.common.not_before_days),
        false,
        Some(actor.rev.context("actor revision number is required")?),
        Some(actor.ver.context("actor version is required")?),
        sanitize_alias(actor.call_alias)?,
    );

    let jwt = claims.encode(&issuer)?;

    Ok(CommandOutput::from_key_and_text("token", jwt))
}

fn generate_operator(operator: OperatorMetadata, output_kind: OutputKind) -> Result<CommandOutput> {
    let self_sign_key = extract_keypair(
        operator.issuer.clone(),
        Some(operator.name.clone()),
        operator.common.directory.clone(),
        KeyPairType::Operator,
        operator.common.disable_keygen,
        output_kind,
    )?;

    let additional_keys = match operator.additional_signing_keys.clone() {
        Some(keys) => get_keypair_vec(
            &keys,
            operator.common.directory.clone(),
            KeyPairType::Operator,
            true,
            output_kind,
        ),
        None => vec![],
    };

    let claims: Claims<Operator> = Claims::<Operator>::with_dates(
        operator.name.clone(),
        self_sign_key.public_key(),
        self_sign_key.public_key(),
        days_from_now_to_jwt_time(operator.common.not_before_days),
        days_from_now_to_jwt_time(operator.common.expires_in_days),
        if !additional_keys.is_empty() {
            additional_keys.iter().map(|k| k.public_key()).collect()
        } else {
            vec![]
        },
    );

    let jwt = claims.encode(&self_sign_key)?;

    Ok(CommandOutput::from_key_and_text("token", jwt))
}

fn generate_account(account: AccountMetadata, output_kind: OutputKind) -> Result<CommandOutput> {
    let issuer = extract_keypair(
        account.issuer.clone(),
        Some(account.name.clone()),
        account.common.directory.clone(),
        KeyPairType::Operator,
        account.common.disable_keygen,
        output_kind,
    )?;
    let subject = extract_keypair(
        account.subject.clone(),
        Some(account.name.clone()),
        account.common.directory.clone(),
        KeyPairType::Account,
        account.common.disable_keygen,
        output_kind,
    )?;
    let additional_keys = match account.additional_signing_keys.clone() {
        Some(keys) => get_keypair_vec(
            &keys,
            account.common.directory.clone(),
            KeyPairType::Account,
            true,
            output_kind,
        ),
        None => vec![],
    };

    let claims: Claims<Account> = Claims::<Account>::with_dates(
        account.name.clone(),
        issuer.public_key(),
        subject.public_key(),
        days_from_now_to_jwt_time(account.common.not_before_days),
        days_from_now_to_jwt_time(account.common.expires_in_days),
        if !additional_keys.is_empty() {
            additional_keys.iter().map(|k| k.public_key()).collect()
        } else {
            vec![]
        },
    );
    let jwt = claims.encode(&issuer)?;
    Ok(CommandOutput::from_key_and_text("token", jwt))
}

fn generate_provider(provider: ProviderMetadata, output_kind: OutputKind) -> Result<CommandOutput> {
    let issuer = extract_keypair(
        provider.issuer.clone(),
        provider.name.clone(),
        provider.common.directory.clone(),
        KeyPairType::Account,
        provider.common.disable_keygen,
        output_kind,
    )?;
    let subject = extract_keypair(
        provider.subject.clone(),
        provider.name.clone(),
        provider.common.directory.clone(),
        KeyPairType::Service,
        provider.common.disable_keygen,
        output_kind,
    )?;

    let claims: Claims<CapabilityProvider> = Claims::<CapabilityProvider>::with_dates(
        provider.name.context("provider name is required")?,
        issuer.public_key(),
        subject.public_key(),
        provider.capid.context("capability ID is required")?,
        provider.vendor.context("vendor is required")?,
        provider.revision,
        provider.version.clone(),
        HashMap::new(),
        days_from_now_to_jwt_time(provider.common.not_before_days),
        days_from_now_to_jwt_time(provider.common.expires_in_days),
    );
    let jwt = claims.encode(&issuer)?;
    Ok(CommandOutput::from_key_and_text("token", jwt))
}

pub fn sign_file(cmd: SignCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    let mut sfile = File::open(&cmd.source)
        .with_context(|| format!("Failed to open file for signing '{}'", &cmd.source))?;
    let mut buf = Vec::new();
    sfile.read_to_end(&mut buf).unwrap();

    let issuer = extract_keypair(
        cmd.metadata.issuer.clone(),
        Some(cmd.source.clone()),
        cmd.metadata.common.directory.clone(),
        KeyPairType::Account,
        cmd.metadata.common.disable_keygen,
        output_kind,
    )?;
    let subject = extract_keypair(
        cmd.metadata.subject.clone(),
        Some(cmd.source.clone()),
        cmd.metadata.common.directory.clone(),
        KeyPairType::Module,
        cmd.metadata.common.disable_keygen,
        output_kind,
    )?;

    let mut caps_list = vec![];
    if cmd.metadata.keyvalue {
        caps_list.push(wascap::caps::KEY_VALUE.to_string());
    }
    if cmd.metadata.msg_broker {
        caps_list.push(wascap::caps::MESSAGING.to_string());
    }
    if cmd.metadata.http_client {
        caps_list.push(wascap::caps::HTTP_CLIENT.to_string());
    }
    if cmd.metadata.http_server {
        caps_list.push(wascap::caps::HTTP_SERVER.to_string());
    }
    if cmd.metadata.blob_store {
        caps_list.push(wascap::caps::BLOB.to_string());
    }
    if cmd.metadata.logging {
        caps_list.push(wascap::caps::LOGGING.to_string());
    }
    if cmd.metadata.eventstream {
        caps_list.push(wascap::caps::EVENTSTREAMS.to_string());
    }
    caps_list.extend(cmd.metadata.custom_caps.iter().cloned());

    let signed = sign_buffer_with_claims(
        cmd.metadata.name.context("actor name is required")?,
        &buf,
        &subject,
        &issuer,
        cmd.metadata.common.expires_in_days,
        cmd.metadata.common.not_before_days,
        caps_list.clone(),
        cmd.metadata.tags.clone(),
        false,
        Some(
            cmd.metadata
                .rev
                .context("actor revision number is required")?,
        ),
        Some(cmd.metadata.ver.context("actor version is required")?),
        sanitize_alias(cmd.metadata.call_alias)?,
    )?;

    let destination = match cmd.destination.clone() {
        Some(d) => d,
        None => {
            let path_buf = PathBuf::from(cmd.source.clone());

            let path = path_buf.parent().unwrap().to_str().unwrap().to_string();
            let module_name = path_buf.file_stem().unwrap().to_str().unwrap().to_string();
            // If path is empty, user supplied module in current directory
            if path.is_empty() {
                format!("./{}_s.wasm", module_name)
            } else {
                format!("{}/{}_s.wasm", path, module_name)
            }
        }
    };

    let destination_path = PathBuf::from(destination.clone());

    if let Some(p) = destination_path.parent() {
        fs::create_dir_all(p)?;
    }

    let mut outfile = File::create(destination_path).unwrap();

    let output = match outfile.write(&signed) {
        Ok(_) => {
            let mut map = HashMap::new();
            map.insert("destination".to_string(), json!(destination));
            map.insert("capabilities".to_string(), json!(caps_list));
            Ok(CommandOutput::new(
                format!(
                    "Successfully signed {} with capabilities: {}",
                    destination,
                    caps_list.join(",")
                ),
                map,
            ))
        }

        Err(e) => Err(e),
    }?;

    Ok(output)
}

fn sanitize_alias(call_alias: Option<String>) -> Result<Option<String>> {
    if let Some(alias) = call_alias {
        // Alias cannot be a public key to ensure best practices
        if alias.is_empty() {
            bail!("Call alias cannot be empty")
        } else if alias.len() == 56
            && alias
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        {
            bail!("Public key cannot be used as a call alias")
        // Valid aliases contain a combination of lowercase alphanumeric characters, dashes, and slashes
        } else if alias
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '/')
        {
            Ok(Some(alias))
        } else {
            bail!("Call alias contained invalid characters.\nValid aliases are lowercase alphanumeric and can contain underscores and slashes")
        }
    } else {
        Ok(None)
    }
}

/// Retreive claims from a given wasmCloud instance
pub async fn get_claims(
    GetClaimsCommand { opts }: GetClaimsCommand,
) -> Result<Vec<HashMap<String, String>>> {
    let wco: WashConnectionOptions = opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;
    client
        .get_claims()
        .await
        .map_err(boxed_err_to_anyhow)
        .with_context(|| {
            format!(
                "Was able to connect to NATS, but failed to get claims: {:?}",
                client
            )
        })
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use super::*;
    use crate::parser::{
        ActorConfig, CommonConfig, LanguageConfig, RegistryConfig, RustConfig, TypeConfig,
    };
    use claims::assert_ok;
    use clap::Parser;
    use semver::Version;

    #[derive(Parser)]
    struct Cmd {
        #[clap(subcommand)]
        claims: ClaimsCliCommand,
    }

    const SUBSCRIBER_OCI: &str = "wasmcloud.azurecr.io/subscriber:0.2.0";

    #[test]
    fn test_claims_sanitize_alias() {
        const VALID_ALPHANUMERIC: &str = "abc123";
        const VALID_WITHSYMBOLS: &str = "myorganization/subfolder_three";
        const INVALID_SYMBOLS: &str = "hello*^&%@#";
        const INVALID_CAPITAL: &str = "wasmCloud/camelCase_";
        const INVALID_PKEY: &str = "MCUOUQQP3WK4EWO76DPWIEKXMN4JYZ63KEGIEEHZCNBR2GEIXPB4ZFUT";

        assert_eq!(
            sanitize_alias(Some(VALID_ALPHANUMERIC.to_string()))
                .unwrap()
                .unwrap(),
            VALID_ALPHANUMERIC
        );
        assert_eq!(
            sanitize_alias(Some(VALID_WITHSYMBOLS.to_string()))
                .unwrap()
                .unwrap(),
            VALID_WITHSYMBOLS
        );

        let invalid_message = "Call alias contained invalid characters.\nValid aliases are lowercase alphanumeric and can contain underscores and slashes";
        let invalid_symbols = sanitize_alias(Some(INVALID_SYMBOLS.to_string()));
        match invalid_symbols {
            Err(e) => assert_eq!(format!("{}", e), invalid_message),
            _ => panic!("invalid symbols in call alias should not be accepted"),
        };

        let invalid_uppercase = sanitize_alias(Some(INVALID_CAPITAL.to_string()));
        match invalid_uppercase {
            Err(e) => assert_eq!(format!("{}", e), invalid_message),
            _ => panic!("uppercase symbols in call alias should not be accepted"),
        };

        let pkey_message = "Public key cannot be used as a call alias";
        let invalid_pkey = sanitize_alias(Some(INVALID_PKEY.to_string()));
        match invalid_pkey {
            Err(e) => assert_eq!(format!("{}", e), pkey_message),
            _ => panic!("public keys cannot be a call alias"),
        };

        let empty_message = "Call alias cannot be empty";
        let invalid_empty = sanitize_alias(Some("".to_string()));
        match invalid_empty {
            Err(e) => assert_eq!(format!("{}", e), empty_message),
            _ => panic!("call alias cannot be left empty"),
        }

        assert!(sanitize_alias(None).unwrap().is_none());
    }

    #[test]
    /// Enumerates all options and flags of the `claims inspect` command
    /// to ensure command line arguments do not change between versions
    fn test_claims_inspect_comprehensive() {
        let cmd: Cmd = Parser::try_parse_from([
            "claims",
            "inspect",
            SUBSCRIBER_OCI,
            "--digest",
            "sha256:5790f650cff526fcbc1271107a05111a6647002098b74a9a5e2e26e3c0a116b8",
            "--user",
            "name",
            "--password",
            "opensesame",
            "--allow-latest",
            "--insecure",
            "--jwt-only",
            "--no-cache",
        ])
        .unwrap();

        match cmd.claims {
            ClaimsCliCommand::Inspect(InspectCommand {
                module,
                jwt_only,
                digest,
                allow_latest,
                user,
                password,
                insecure,
                no_cache,
                wit,
            }) => {
                assert_eq!(module, SUBSCRIBER_OCI);
                assert_eq!(
                    digest.unwrap(),
                    "sha256:5790f650cff526fcbc1271107a05111a6647002098b74a9a5e2e26e3c0a116b8"
                );
                assert_eq!(user.unwrap(), "name");
                assert_eq!(password.unwrap(), "opensesame");
                assert!(allow_latest);
                assert!(insecure);
                assert!(jwt_only);
                assert!(no_cache);
                assert!(!wit);
            }
            cmd => panic!("claims constructed incorrect command: {:?}", cmd),
        }

        let short_cmd: Cmd = Parser::try_parse_from([
            "claims",
            "inspect",
            SUBSCRIBER_OCI,
            "-d",
            "sha256:5790f650cff526fcbc1271107a05111a6647002098b74a9a5e2e26e3c0a116b8",
            "-u",
            "name",
            "-p",
            "opensesame",
            "--allow-latest",
            "--insecure",
            "--jwt-only",
            "--no-cache",
        ])
        .unwrap();

        match short_cmd.claims {
            ClaimsCliCommand::Inspect(InspectCommand {
                module,
                jwt_only,
                digest,
                allow_latest,
                user,
                password,
                insecure,
                no_cache,
                wit,
            }) => {
                assert_eq!(module, SUBSCRIBER_OCI);
                assert_eq!(
                    digest.unwrap(),
                    "sha256:5790f650cff526fcbc1271107a05111a6647002098b74a9a5e2e26e3c0a116b8"
                );
                assert_eq!(user.unwrap(), "name");
                assert_eq!(password.unwrap(), "opensesame");
                assert!(allow_latest);
                assert!(insecure);
                assert!(jwt_only);
                assert!(no_cache);
                assert!(!wit);
            }
            cmd => panic!("claims constructed incorrect command: {:?}", cmd),
        }
    }

    #[test]
    /// Enumerates all options and flags of the `claims sign` command
    /// to ensure command line arguments do not change between versions
    fn test_claims_sign_comprehensive() {
        const LOCAL_WASM: &str = "./myactor.wasm";
        const ISSUER_KEY: &str = "SAAOBYD6BLELXSNN4S3TXUM7STGPB3A5HYU3D5T7XA4WHGVQBDBD4LJPOM";
        const SUBJECT_KEY: &str = "SMAMA4ABHIJUYQR54BDFHEMXIIGQATUXK6RYU6XLTFHDNCRVWT4KSDDSVE";
        let long_cmd: Cmd = Parser::try_parse_from([
            "claims",
            "sign",
            LOCAL_WASM,
            "--name",
            "MyActor",
            "--cap",
            "test:custom",
            "--destination",
            "./myactor_s.wasm",
            "--directory",
            "./dir",
            "--expires",
            "3",
            "--issuer",
            ISSUER_KEY,
            "--subject",
            SUBJECT_KEY,
            "--nbf",
            "1",
            "--rev",
            "2",
            "--tag",
            "testtag",
            "--ver",
            "0.0.1",
            "--blob_store",
            "--events",
            "--extras",
            "--http_client",
            "--http_server",
            "--keyvalue",
            "--logging",
            "--msg",
            "--disable-keygen",
        ])
        .unwrap();

        match long_cmd.claims {
            ClaimsCliCommand::Sign(SignCommand {
                source,
                destination,
                metadata,
            }) => {
                assert_eq!(source, LOCAL_WASM);
                assert_eq!(destination.unwrap(), "./myactor_s.wasm");
                assert_eq!(metadata.common.directory.unwrap(), PathBuf::from("./dir"));
                assert_eq!(metadata.common.expires_in_days.unwrap(), 3);
                assert_eq!(metadata.common.not_before_days.unwrap(), 1);
                assert!(metadata.common.disable_keygen);
                assert!(metadata.keyvalue);
                assert!(metadata.msg_broker);
                assert!(metadata.http_server);
                assert!(metadata.http_client);
                assert!(metadata.blob_store);
                assert!(metadata.extras);
                assert!(metadata.logging);
                assert!(metadata.eventstream);
                assert_eq!(metadata.name.unwrap(), "MyActor");
                assert!(!metadata.custom_caps.is_empty());
                assert_eq!(metadata.custom_caps[0], "test:custom");
                assert!(!metadata.tags.is_empty());
                assert_eq!(metadata.tags[0], "testtag");
                assert_eq!(metadata.rev.unwrap(), 2);
                assert_eq!(metadata.ver.unwrap(), "0.0.1");
            }
            cmd => panic!("claims constructed incorrect command: {:?}", cmd),
        }
        let short_cmd: Cmd = Parser::try_parse_from([
            "claims",
            "sign",
            LOCAL_WASM,
            "-n",
            "MyActor",
            "-c",
            "test:custom",
            "-d",
            "./myactor_s.wasm",
            "--directory",
            "./dir",
            "-x",
            "3",
            "-i",
            ISSUER_KEY,
            "-s",
            SUBJECT_KEY,
            "-b",
            "1",
            "-r",
            "2",
            "-t",
            "testtag",
            "-v",
            "0.0.1",
            "-f",
            "-e",
            "-z",
            "-q",
            "-k",
            "-l",
            "-g",
            "--disable-keygen",
        ])
        .unwrap();

        match short_cmd.claims {
            ClaimsCliCommand::Sign(SignCommand {
                source,
                destination,
                metadata,
            }) => {
                assert_eq!(source, LOCAL_WASM);
                assert_eq!(destination.unwrap(), "./myactor_s.wasm");
                assert_eq!(metadata.common.directory.unwrap(), PathBuf::from("./dir"));
                assert_eq!(metadata.common.expires_in_days.unwrap(), 3);
                assert_eq!(metadata.common.not_before_days.unwrap(), 1);
                assert!(metadata.common.disable_keygen);
                assert!(metadata.keyvalue);
                assert!(metadata.msg_broker);
                assert!(metadata.http_server);
                assert!(metadata.blob_store);
                assert!(metadata.extras);
                assert!(metadata.logging);
                assert!(metadata.eventstream);
                assert_eq!(metadata.name.unwrap(), "MyActor");
                assert!(!metadata.custom_caps.is_empty());
                assert_eq!(metadata.custom_caps[0], "test:custom");
                assert!(!metadata.tags.is_empty());
                assert_eq!(metadata.tags[0], "testtag");
                assert_eq!(metadata.rev.unwrap(), 2);
                assert_eq!(metadata.ver.unwrap(), "0.0.1");
            }
            cmd => panic!("claims constructed incorrect command: {:?}", cmd),
        }
    }

    #[test]
    /// Enumerates all options and flags of the `claims sign` command
    /// to ensure command line arguments do not change between versions
    fn test_claims_token_comprehensive() {
        const DIR: &str = "./tests/fixtures";
        const EXPR: &str = "10";
        const NBFR: &str = "12";
        const OPERATOR_KEY: &str = "SOALSFXSHRVKCNOP2JSOVOU267XMF2ZMLF627OM6ZPS6WMKVS6HKQGU7QM";
        const OPERATOR_TWO_KEY: &str = "SOAC7EGQIMNPUF3XBSWR2IQIX7ITDNRYZZ4PN3ZZTFEVHPMG7BFOJMGPW4";
        const ACCOUNT_KEY: &str = "SAAH3WW3NDAT7GQOO5IHPHNIGS5JNFQN2F72P6QBSHCOKPBLEEDXQUWI4Q";
        const ACTOR_KEY: &str = "SMAA2XB7UP7FZLPLO27NJB65PKYISNQAH7PZ6PJUHR6CUARVANXZ4OTZOU";
        const PROVIDER_KEY: &str = "SVAKIVYER6D2LZS7QJFOU7LQYLRAMJ5DZE4B7BJHX6QFJIY24KN43JZGN4";

        let account_cmd: Cmd = Parser::try_parse_from([
            "claims",
            "token",
            "account",
            "--name",
            "TokenName",
            "--directory",
            DIR,
            "--expires",
            EXPR,
            "--nbf",
            NBFR,
            "--disable-keygen",
            "--issuer",
            OPERATOR_KEY,
            "--subject",
            ACCOUNT_KEY,
            "-a",
            OPERATOR_TWO_KEY,
        ])
        .unwrap();
        match account_cmd.claims {
            ClaimsCliCommand::Token(TokenCommand::Account(AccountMetadata {
                name,
                issuer,
                subject,
                common,
                additional_signing_keys,
                ..
            })) => {
                assert_eq!(name, "TokenName");
                assert_eq!(common.directory.unwrap(), PathBuf::from(DIR));
                assert_eq!(
                    common.expires_in_days.unwrap(),
                    EXPR.parse::<u64>().unwrap()
                );
                assert_eq!(
                    common.not_before_days.unwrap(),
                    NBFR.parse::<u64>().unwrap()
                );
                assert!(common.disable_keygen);
                assert_eq!(issuer.unwrap(), OPERATOR_KEY);
                assert_eq!(subject.unwrap(), ACCOUNT_KEY);
                let adds = additional_signing_keys.unwrap();
                assert_eq!(adds.len(), 1);
                assert_eq!(adds[0], OPERATOR_TWO_KEY);
            }
            cmd => panic!("claims constructed incorrect command: {:?}", cmd),
        }
        let actor_cmd: Cmd = Parser::try_parse_from([
            "claims",
            "token",
            "actor",
            "--name",
            "TokenName",
            "--directory",
            DIR,
            "--expires",
            EXPR,
            "--nbf",
            NBFR,
            "--disable-keygen",
            "--issuer",
            ACCOUNT_KEY,
            "--subject",
            ACTOR_KEY,
            "-c",
            "test:custom",
            "--rev",
            "2",
            "--tag",
            "testtag",
            "--ver",
            "0.0.1",
            "--blob_store",
            "--events",
            "--extras",
            "--http_client",
            "--http_server",
            "--keyvalue",
            "--logging",
            "--msg",
        ])
        .unwrap();
        match actor_cmd.claims {
            ClaimsCliCommand::Token(TokenCommand::Actor(ActorMetadata {
                name,
                issuer,
                subject,
                common,
                keyvalue,
                msg_broker,
                http_server,
                http_client,
                blob_store,
                extras,
                logging,
                eventstream,
                custom_caps,
                tags,
                rev,
                ver,
                ..
            })) => {
                assert_eq!(name.unwrap(), "TokenName");
                assert_eq!(common.directory.unwrap(), PathBuf::from(DIR));
                assert_eq!(
                    common.expires_in_days.unwrap(),
                    EXPR.parse::<u64>().unwrap()
                );
                assert_eq!(
                    common.not_before_days.unwrap(),
                    NBFR.parse::<u64>().unwrap()
                );
                assert!(common.disable_keygen);
                assert_eq!(issuer.unwrap(), ACCOUNT_KEY);
                assert_eq!(subject.unwrap(), ACTOR_KEY);
                assert!(keyvalue);
                assert!(msg_broker);
                assert!(http_server);
                assert!(http_client);
                assert!(blob_store);
                assert!(extras);
                assert!(logging);
                assert!(eventstream);
                assert_eq!(custom_caps.len(), 1);
                assert_eq!(custom_caps[0], "test:custom");
                assert!(!tags.is_empty());
                assert_eq!(tags[0], "testtag");
                assert_eq!(rev.unwrap(), 2);
                assert_eq!(ver.unwrap(), "0.0.1");
            }
            cmd => panic!("claims constructed incorrect command: {:?}", cmd),
        }
        let operator_cmd: Cmd = Parser::try_parse_from([
            "claims",
            "token",
            "operator",
            "--name",
            "TokenName",
            "--directory",
            DIR,
            "--expires",
            EXPR,
            "--nbf",
            NBFR,
            "--disable-keygen",
            "--issuer",
            OPERATOR_KEY,
            "--additional-key",
            OPERATOR_TWO_KEY,
        ])
        .unwrap();
        match operator_cmd.claims {
            ClaimsCliCommand::Token(TokenCommand::Operator(OperatorMetadata {
                name,
                issuer,
                common,
                additional_signing_keys,
                ..
            })) => {
                assert_eq!(name, "TokenName");
                assert_eq!(common.directory.unwrap(), PathBuf::from(DIR));
                assert_eq!(
                    common.expires_in_days.unwrap(),
                    EXPR.parse::<u64>().unwrap()
                );
                assert_eq!(
                    common.not_before_days.unwrap(),
                    NBFR.parse::<u64>().unwrap()
                );
                assert!(common.disable_keygen);
                assert_eq!(issuer.unwrap(), OPERATOR_KEY);
                let adds = additional_signing_keys.unwrap();
                assert_eq!(adds.len(), 1);
                assert_eq!(adds[0], OPERATOR_TWO_KEY);
            }
            cmd => panic!("claims constructed incorrect command: {:?}", cmd),
        }
        let provider_cmd: Cmd = Parser::try_parse_from([
            "claims",
            "token",
            "provider",
            "--name",
            "TokenName",
            "--directory",
            DIR,
            "--expires",
            EXPR,
            "--nbf",
            NBFR,
            "--disable-keygen",
            "--issuer",
            ACCOUNT_KEY,
            "--subject",
            PROVIDER_KEY,
            "--capid",
            "wasmcloud:test",
            "--vendor",
            "test",
            "--revision",
            "0",
            "--version",
            "1.2.3",
        ])
        .unwrap();
        match provider_cmd.claims {
            ClaimsCliCommand::Token(TokenCommand::Provider(ProviderMetadata {
                name,
                issuer,
                subject,
                common,
                capid,
                vendor,
                revision,
                version,
                ..
            })) => {
                assert_eq!(name.unwrap(), "TokenName");
                assert_eq!(common.directory.unwrap(), PathBuf::from(DIR));
                assert_eq!(
                    common.expires_in_days.unwrap(),
                    EXPR.parse::<u64>().unwrap()
                );
                assert_eq!(
                    common.not_before_days.unwrap(),
                    NBFR.parse::<u64>().unwrap()
                );
                assert!(common.disable_keygen);
                assert_eq!(issuer.unwrap(), ACCOUNT_KEY);
                assert_eq!(subject.unwrap(), PROVIDER_KEY);
                assert_eq!(capid.unwrap(), "wasmcloud:test");
                assert_eq!(vendor.unwrap(), "test");
                assert_eq!(revision.unwrap(), 0);
                assert_eq!(version.unwrap(), "1.2.3");
            }
            cmd => panic!("claims constructed incorrect command: {:?}", cmd),
        }
    }

    #[test]
    fn rust_actor_metadata_with_project_config_overrides() -> anyhow::Result<()> {
        let result = get_config(
            Some(PathBuf::from(
                "./tests/parser/files/rust_actor_claims_metadata.toml",
            )),
            None,
        );

        let project_config = assert_ok!(result);

        assert_eq!(
            project_config.language,
            LanguageConfig::Rust(RustConfig {
                cargo_path: Some("./cargo".into()),
                target_path: Some("./target".into())
            })
        );

        assert_eq!(
            project_config.project_type,
            TypeConfig::Actor(ActorConfig {
                claims: vec![
                    "wasmcloud:httpserver".to_string(),
                    "wasmcloud:httpclient".to_string(),
                    "lexcorp:quantum-simulator".to_string()
                ],
                key_directory: PathBuf::from("./keys"),
                destination: Some(PathBuf::from("./build/testactor.wasm".to_string())),
                call_alias: Some("test-actor".to_string()),
                tags: Some(HashSet::from([
                    "wasmcloud.com/experimental".into(),
                    "test".into(),
                ])),
                ..ActorConfig::default()
            })
        );

        assert_eq!(
            project_config.common,
            CommonConfig {
                name: "testactor".to_string(),
                version: Version::parse("0.1.0").unwrap(),
                revision: 666,
                path: PathBuf::from("./tests/parser/files/")
                    .canonicalize()
                    .unwrap(),
                wasm_bin_name: None,
                registry: RegistryConfig::default(),
            }
        );

        //=== check project config overrides when cli args are NOT specified...
        let actor_metadata = ActorMetadata::default().update_with_project_config(&project_config);
        assert_eq!(
            actor_metadata,
            ActorMetadata {
                name: Some("testactor".to_string()),
                ver: Some(Version::parse("0.1.0")?.to_string()),
                rev: Some(666),
                http_server: true,
                http_client: true,
                custom_caps: vec!["lexcorp:quantum-simulator".to_string()],
                call_alias: Some("test-actor".to_string()),
                tags: vec!["test".to_string(), "wasmcloud.com/experimental".to_string()],
                common: GenerateCommon {
                    directory: Some(PathBuf::from("./keys")),
                    ..GenerateCommon::default()
                },
                ..ActorMetadata::default()
            }
        );

        //=== check project config overrides when some cli args are specified...
        const LOCAL_WASM: &str = "./myactor.wasm";
        let cmd: Cmd = Parser::try_parse_from([
            "claims",
            "sign",
            LOCAL_WASM,
            "--name",
            "MyActor",
            "--cap",
            "test:custom",
            "--destination",
            "./myactor_s.wasm",
            "--directory",
            "./dir",
            "--rev",
            "777",
            "--tag",
            "test-tag",
            "--ver",
            "0.2.0",
            "--blob_store",
            "--keyvalue",
            "--logging",
        ])
        .unwrap();

        match cmd.claims {
            ClaimsCliCommand::Sign(signcmd) => {
                let cmd = SignCommand {
                    metadata: signcmd.metadata.update_with_project_config(&project_config),
                    destination: match &project_config.project_type {
                        TypeConfig::Actor(ref actor_config) => signcmd.destination.or(actor_config
                            .destination
                            .clone()
                            .map(|d| {
                                d.to_str()
                                    .expect("unable to convert destination pathbuf to str")
                                    .to_string()
                            })),
                        _ => signcmd.destination,
                    },
                    ..signcmd
                };

                assert_eq!(cmd.source, LOCAL_WASM);
                assert_eq!(cmd.destination.unwrap(), "./myactor_s.wasm");
                assert_eq!(
                    cmd.metadata.common.directory.unwrap(),
                    PathBuf::from("./dir")
                );
                assert!(cmd.metadata.keyvalue);
                assert!(cmd.metadata.http_server); // from project_config
                assert!(cmd.metadata.http_client); // from project_config
                assert!(cmd.metadata.blob_store);
                assert!(cmd.metadata.logging);
                assert_eq!(cmd.metadata.name.unwrap(), "MyActor");
                assert_eq!(cmd.metadata.custom_caps.len(), 2);
                assert!(cmd
                    .metadata
                    .custom_caps
                    .contains(&"test:custom".to_string()));
                assert!(cmd
                    .metadata
                    .custom_caps
                    .contains(&"lexcorp:quantum-simulator".to_string())); // from project_config
                assert_eq!(cmd.metadata.tags.len(), 3);
                assert!(cmd.metadata.tags.contains(&"test-tag".to_string()));
                assert!(cmd.metadata.tags.contains(&"test".to_string())); // from project_config
                assert!(cmd
                    .metadata
                    .tags
                    .contains(&"wasmcloud.com/experimental".to_string())); // from project_config
                assert_eq!(cmd.metadata.rev.unwrap(), 777);
                assert_eq!(cmd.metadata.ver.unwrap(), "0.2.0");
                assert_eq!(cmd.metadata.call_alias.unwrap(), "test-actor"); // from project_config
            }

            _ => unreachable!("claims constructed incorrect command"),
        }

        Ok(())
    }

    #[test]
    fn rust_provider_metadata_with_project_config_overrides() -> anyhow::Result<()> {
        let result = get_config(
            Some(PathBuf::from(
                "./tests/parser/files/rust_provider_claims_metadata.toml",
            )),
            None,
        );

        let project_config = assert_ok!(result);

        assert_eq!(
            project_config.language,
            LanguageConfig::Rust(RustConfig {
                cargo_path: Some("./cargo".into()),
                target_path: Some("./target".into())
            })
        );

        assert_eq!(
            project_config.project_type,
            TypeConfig::Provider(ProviderConfig {
                capability_id: "wasmcloud:httpserver".into(),
                vendor: "wayne-industries".into()
            })
        );

        assert_eq!(
            project_config.common,
            CommonConfig {
                name: "testprovider".to_string(),
                version: Version::parse("0.1.0").unwrap(),
                revision: 666,
                path: PathBuf::from("./tests/parser/files/")
                    .canonicalize()
                    .unwrap(),
                wasm_bin_name: None,
                registry: RegistryConfig::default(),
            }
        );

        //=== check project config overrides when cli args are NOT specified...
        let provider_metadata =
            ProviderMetadata::default().update_with_project_config(&project_config);
        assert_eq!(
            provider_metadata,
            ProviderMetadata {
                name: Some("testprovider".to_string()),
                version: Some(Version::parse("0.1.0")?.to_string()),
                revision: Some(666),
                capid: Some("wasmcloud:httpserver".into()),
                vendor: Some("wayne-industries".into()),
                ..ProviderMetadata::default()
            }
        );

        //=== check project config overrides when cli args are specified...
        let cmd: Cmd = Parser::try_parse_from([
            "claims",
            "token",
            "provider",
            "--name",
            "TokenName",
            "--capid",
            "wasmcloud:test",
            "--vendor",
            "test",
            "--revision",
            "777",
            "--version",
            "0.2.0",
        ])
        .unwrap();
        match cmd.claims {
            ClaimsCliCommand::Token(TokenCommand::Provider(provider_metadata)) => {
                let metadata = provider_metadata.update_with_project_config(&project_config);
                assert_eq!(metadata.name.unwrap(), "TokenName");
                assert_eq!(metadata.capid.unwrap(), "wasmcloud:test");
                assert_eq!(metadata.vendor.unwrap(), "test");
                assert_eq!(metadata.revision.unwrap(), 777);
                assert_eq!(metadata.version.unwrap(), "0.2.0");
            }
            _ => unreachable!("claims constructed incorrect command"),
        }

        Ok(())
    }
}

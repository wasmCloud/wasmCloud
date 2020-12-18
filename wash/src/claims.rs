// Copyright 2015-2018 Capital One Services, LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::keys::extract_keypair;
use crate::util::{format_output, Output, OutputKind};
use nkeys::{KeyPair, KeyPairType};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::path::PathBuf;
use structopt::clap::AppSettings;
use structopt::StructOpt;
use term_table::{
    row::Row,
    table_cell::{Alignment, TableCell},
    Table, TableStyle,
};
use wascap::caps::*;
use wascap::jwt::{
    Account, Actor, CapabilityProvider, Claims, Operator, TokenValidation, WascapEntity,
};
use wascap::wasm::{days_from_now_to_jwt_time, sign_buffer_with_claims};

#[derive(Debug, StructOpt, Clone)]
#[structopt(
    global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
    name = "claims")]
pub(crate) struct ClaimsCli {
    #[structopt(flatten)]
    command: ClaimsCliCommand,
}

#[derive(Debug, Clone, StructOpt)]
enum ClaimsCliCommand {
    /// Examine the capabilities of a WebAssembly module
    #[structopt(name = "inspect")]
    Inspect(InspectCommand),
    /// Sign a WebAssembly module, specifying capabilities and other claims
    /// including expiration, tags, and additional metadata
    #[structopt(name = "sign")]
    Sign(SignCommand),
    /// Generate a signed JWT by supplying basic token information, a signing seed key, and metadata
    #[structopt(name = "token")]
    Token(TokenCommand),
}

#[derive(StructOpt, Debug, Clone)]
struct InspectCommand {
    /// Path to signed actor module or OCI URL of signed actor module
    module: String,

    /// Extract the raw JWT from the file and print to stdout
    #[structopt(name = "jwt_only", long = "jwt-only")]
    jwt_only: bool,

    /// Digest to verify artifact against (if OCI URL is provided for <module>)
    #[structopt(short = "d", long = "digest")]
    digest: Option<String>,

    /// Allow latest artifact tags (if OCI URL is provided for <module>)
    #[structopt(long = "allow-latest")]
    allow_latest: bool,

    /// OCI username, if omitted anonymous authentication will be used
    #[structopt(
        short = "u",
        long = "user",
        env = "WASH_REG_USER",
        hide_env_values = true
    )]
    user: Option<String>,

    /// OCI password, if omitted anonymous authentication will be used
    #[structopt(
        short = "p",
        long = "password",
        env = "WASH_REG_PASSWORD",
        hide_env_values = true
    )]
    password: Option<String>,

    /// Allow insecure (HTTP) registry connections
    #[structopt(long = "insecure")]
    insecure: bool,

    #[structopt(flatten)]
    pub(crate) output: Output,
}

#[derive(StructOpt, Debug, Clone)]
struct SignCommand {
    /// File to read
    source: String,

    /// Destionation for signed module. If this flag is not provided, the signed module will be placed in the same directory as the source with a "_s" suffix
    #[structopt(short = "d", long = "destination")]
    destination: Option<String>,

    #[structopt(flatten)]
    metadata: ActorMetadata,
}

#[derive(Debug, Clone, StructOpt)]
enum TokenCommand {
    /// Generate a signed JWT for an actor module
    #[structopt(name = "actor")]
    Actor(ActorMetadata),
    /// Generate a signed JWT for an operator
    #[structopt(name = "operator")]
    Operator(OperatorMetadata),
    /// Generate a signed JWT for an account
    #[structopt(name = "account")]
    Account(AccountMetadata),
    /// Generate a signed JWT for a service (capability provider)
    #[structopt(name = "provider")]
    Provider(ProviderMetadata),
}

#[derive(Debug, Clone, StructOpt, Serialize, Deserialize)]
struct GenerateCommon {
    /// Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
    #[structopt(long = "directory", env = "WASH_KEYS", hide_env_values = true)]
    directory: Option<String>,

    /// Indicates the token expires in the given amount of days. If this option is left off, the token will never expire
    #[structopt(short = "x", long = "expires")]
    expires_in_days: Option<u64>,

    /// Period in days that must elapse before this token is valid. If this option is left off, the token will be valid immediately
    #[structopt(short = "b", long = "nbf")]
    not_before_days: Option<u64>,

    /// Disables autogeneration of keys if seed(s) are not provided
    #[structopt(long = "disable-keygen")]
    disable_keygen: bool,

    #[structopt(flatten)]
    pub(crate) output: Output,
}

#[derive(Debug, Clone, StructOpt)]
struct OperatorMetadata {
    /// A descriptive name for the operator
    #[structopt(short = "n", long = "name")]
    name: String,

    /// Path to issuer seed key (self signing operator). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[structopt(
        short = "i",
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    issuer: Option<String>,

    /// Additional keys to add to valid signers list
    /// Can either be seed value or path to seed file
    #[structopt(short = "a", long = "additional-key", name = "additional-keys")]
    additional_signing_keys: Option<Vec<String>>,

    #[structopt(flatten)]
    common: GenerateCommon,
}

#[derive(Debug, Clone, StructOpt)]
struct AccountMetadata {
    /// A descriptive name for the account
    #[structopt(short = "n", long = "name")]
    name: String,

    /// Path to issuer seed key (operator). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[structopt(
        short = "i",
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    issuer: Option<String>,

    /// Path to subject seed key (account). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[structopt(
        short = "s",
        long = "subject",
        env = "WASH_SUBJECT_KEY",
        hide_env_values = true
    )]
    subject: Option<String>,

    /// Additional keys to add to valid signers list.
    /// Can either be seed value or path to seed file
    #[structopt(short = "a", long = "additional-key", name = "additional-keys")]
    additional_signing_keys: Option<Vec<String>>,

    #[structopt(flatten)]
    common: GenerateCommon,
}

#[derive(Debug, Clone, StructOpt)]
struct ProviderMetadata {
    /// A descriptive name for the provider
    #[structopt(short = "n", long = "name")]
    name: String,

    /// Capability contract ID that this provider supports
    #[structopt(short = "c", long = "capid")]
    capid: String,

    /// A human-readable string identifying the vendor of this provider (e.g. Redis or Cassandra or NATS etc)
    #[structopt(short = "v", long = "vendor")]
    vendor: String,

    /// Monotonically increasing revision number
    #[structopt(short = "r", long = "revision")]
    revision: Option<i32>,

    /// Human-friendly version string
    #[structopt(short = "e", long = "version")]
    version: Option<String>,

    /// Path to issuer seed key (account). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[structopt(
        short = "i",
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    issuer: Option<String>,

    /// Path to subject seed key (service). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[structopt(
        short = "s",
        long = "subject",
        env = "WASH_SUBJECT_KEY",
        hide_env_values = true
    )]
    subject: Option<String>,

    #[structopt(flatten)]
    common: GenerateCommon,
}

#[derive(StructOpt, Debug, Clone, Serialize, Deserialize)]
struct ActorMetadata {
    /// Enable the Key/Value Store standard capability
    #[structopt(short = "k", long = "keyvalue")]
    keyvalue: bool,
    /// Enable the Message broker standard capability
    #[structopt(short = "g", long = "msg")]
    msg_broker: bool,
    /// Enable the HTTP server standard capability
    #[structopt(short = "q", long = "http_server")]
    http_server: bool,
    /// Enable the HTTP client standard capability
    #[structopt(short = "h", long = "http_client")]
    http_client: bool,
    /// Enable access to the blob store capability
    #[structopt(short = "f", long = "blob_store")]
    blob_store: bool,
    /// Enable access to the extras functionality (random nos, guids, etc)
    #[structopt(short = "z", long = "extras")]
    extras: bool,
    /// Enable access to logging capability
    #[structopt(short = "l", long = "logging")]
    logging: bool,
    /// Enable access to an append-only event stream provider
    #[structopt(short = "e", long = "events")]
    eventstream: bool,
    /// A human-readable, descriptive name for the token
    #[structopt(short = "n", long = "name")]
    name: String,
    /// Add custom capabilities
    #[structopt(short = "c", long = "cap", name = "capabilities")]
    custom_caps: Vec<String>,
    /// A list of arbitrary tags to be embedded in the token
    #[structopt(short = "t", long = "tag")]
    tags: Vec<String>,
    /// Indicates whether the signed module is a capability provider instead of an actor (the default is actor)
    #[structopt(short = "p", long = "prov")]
    provider: bool,
    /// Revision number
    #[structopt(short = "r", long = "rev")]
    rev: Option<i32>,
    /// Human-readable version string
    #[structopt(short = "v", long = "ver")]
    ver: Option<String>,

    /// Path to issuer seed key (account). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[structopt(
        short = "i",
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    issuer: Option<String>,

    /// Path to subject seed key (module). If this flag is not provided, the will be sourced from $WASH_KEYS ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[structopt(
        short = "s",
        long = "subject",
        env = "WASH_SUBJECT_KEY",
        hide_env_values = true
    )]
    subject: Option<String>,

    #[structopt(flatten)]
    common: GenerateCommon,
}

pub(crate) async fn handle_command(cli: ClaimsCli) -> Result<(), Box<dyn ::std::error::Error>> {
    match cli.command {
        ClaimsCliCommand::Inspect(inspectcmd) => render_caps(inspectcmd).await,
        ClaimsCliCommand::Sign(signcmd) => sign_file(signcmd),
        ClaimsCliCommand::Token(gencmd) => generate_token(gencmd),
    }
}

fn generate_token(cmd: TokenCommand) -> Result<(), Box<dyn ::std::error::Error>> {
    match cmd {
        TokenCommand::Actor(actor) => generate_actor(actor),
        TokenCommand::Operator(operator) => generate_operator(operator),
        TokenCommand::Account(account) => generate_account(account),
        TokenCommand::Provider(provider) => generate_provider(provider),
    }
}

fn get_keypair_vec(
    keys: &[String],
    keys_dir: Option<String>,
    keypair_type: KeyPairType,
    disable_keygen: bool,
) -> Result<Vec<KeyPair>, Box<dyn ::std::error::Error>> {
    Ok(keys
        .iter()
        .map(|k| {
            extract_keypair(
                Some(k.to_string()),
                None,
                keys_dir.clone(),
                keypair_type.clone(),
                disable_keygen,
            )
            .unwrap()
        })
        .collect())
}

fn generate_actor(actor: ActorMetadata) -> Result<(), Box<dyn ::std::error::Error>> {
    let issuer = extract_keypair(
        actor.issuer.clone(),
        None,
        actor.common.directory.clone(),
        KeyPairType::Account,
        true,
    )?;
    let subject = extract_keypair(
        actor.subject.clone(),
        None,
        actor.common.directory.clone(),
        KeyPairType::Module,
        true,
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
    if actor.extras {
        caps_list.push(wascap::caps::EXTRAS.to_string());
    }
    caps_list.extend(actor.custom_caps.iter().cloned());

    if actor.provider && caps_list.len() > 1 {
        return Err("Capability providers cannot provide multiple capabilities at once.".into());
    }
    let claims: Claims<Actor> = Claims::<Actor>::with_dates(
        actor.name.clone(),
        issuer.public_key(),
        subject.public_key(),
        Some(caps_list),
        Some(actor.tags.clone()),
        days_from_now_to_jwt_time(actor.common.expires_in_days),
        days_from_now_to_jwt_time(actor.common.not_before_days),
        actor.provider,
        actor.rev,
        actor.ver.clone(),
    );

    let jwt = claims.encode(&issuer)?;
    println!(
        "{}",
        format_output(
            jwt.clone(),
            json!({ "token": jwt }),
            &actor.common.output.kind
        )
    );

    Ok(())
}

fn generate_operator(operator: OperatorMetadata) -> Result<(), Box<dyn ::std::error::Error>> {
    let self_sign_key = extract_keypair(
        operator.issuer.clone(),
        Some(operator.name.clone()),
        operator.common.directory.clone(),
        KeyPairType::Operator,
        operator.common.disable_keygen,
    )?;

    let additional_keys = match operator.additional_signing_keys.clone() {
        Some(keys) => get_keypair_vec(
            &keys,
            operator.common.directory.clone(),
            KeyPairType::Operator,
            true,
        )?,
        None => vec![],
    };

    let claims: Claims<Operator> = Claims::<Operator>::with_dates(
        operator.name.clone(),
        self_sign_key.public_key(),
        self_sign_key.public_key(),
        days_from_now_to_jwt_time(operator.common.not_before_days),
        days_from_now_to_jwt_time(operator.common.expires_in_days),
        if additional_keys.len() > 0 {
            additional_keys.iter().map(|k| k.public_key()).collect()
        } else {
            vec![]
        },
    );

    let jwt = claims.encode(&self_sign_key)?;
    println!(
        "{}",
        format_output(
            jwt.clone(),
            json!({ "token": jwt }),
            &operator.common.output.kind
        )
    );
    Ok(())
}

fn generate_account(account: AccountMetadata) -> Result<(), Box<dyn ::std::error::Error>> {
    let issuer = extract_keypair(
        account.issuer.clone(),
        Some(account.name.clone()),
        account.common.directory.clone(),
        KeyPairType::Operator,
        account.common.disable_keygen,
    )?;
    let subject = extract_keypair(
        account.subject.clone(),
        Some(account.name.clone()),
        account.common.directory.clone(),
        KeyPairType::Account,
        account.common.disable_keygen,
    )?;
    let additional_keys = match account.additional_signing_keys.clone() {
        Some(keys) => get_keypair_vec(
            &keys,
            account.common.directory.clone(),
            KeyPairType::Account,
            true,
        )?,
        None => vec![],
    };

    let claims: Claims<Account> = Claims::<Account>::with_dates(
        account.name.clone(),
        issuer.public_key(),
        subject.public_key(),
        days_from_now_to_jwt_time(account.common.not_before_days),
        days_from_now_to_jwt_time(account.common.expires_in_days),
        if additional_keys.len() > 0 {
            additional_keys.iter().map(|k| k.public_key()).collect()
        } else {
            vec![]
        },
    );
    let jwt = claims.encode(&issuer)?;
    println!(
        "{}",
        format_output(
            jwt.clone(),
            json!({ "token": jwt }),
            &account.common.output.kind
        )
    );
    Ok(())
}

fn generate_provider(provider: ProviderMetadata) -> Result<(), Box<dyn ::std::error::Error>> {
    let issuer = extract_keypair(
        provider.issuer.clone(),
        None,
        provider.common.directory.clone(),
        KeyPairType::Account,
        true,
    )?;
    let subject = extract_keypair(
        provider.subject.clone(),
        None,
        provider.common.directory.clone(),
        KeyPairType::Module,
        true,
    )?;

    let claims: Claims<CapabilityProvider> = Claims::<CapabilityProvider>::with_dates(
        provider.name.clone(),
        issuer.public_key(),
        subject.public_key(),
        provider.capid.clone(),
        provider.vendor.clone(),
        provider.revision.clone(),
        provider.version.clone(),
        HashMap::new(),
        days_from_now_to_jwt_time(provider.common.not_before_days),
        days_from_now_to_jwt_time(provider.common.expires_in_days),
    );
    let jwt = claims.encode(&issuer)?;
    println!(
        "{}",
        format_output(
            jwt.clone(),
            json!({ "token": jwt }),
            &provider.common.output.kind
        )
    );
    Ok(())
}

fn sign_file(cmd: SignCommand) -> Result<(), Box<dyn ::std::error::Error>> {
    let mut sfile = File::open(&cmd.source).unwrap();
    let mut buf = Vec::new();
    sfile.read_to_end(&mut buf).unwrap();

    let issuer = extract_keypair(
        cmd.metadata.issuer.clone(),
        Some(cmd.source.clone()),
        cmd.metadata.common.directory.clone(),
        KeyPairType::Account,
        cmd.metadata.common.disable_keygen,
    )?;
    let subject = extract_keypair(
        cmd.metadata.subject.clone(),
        Some(cmd.source.clone()),
        cmd.metadata.common.directory.clone(),
        KeyPairType::Module,
        cmd.metadata.common.disable_keygen,
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
    if cmd.metadata.extras {
        caps_list.push(wascap::caps::EXTRAS.to_string());
    }
    if cmd.metadata.eventstream {
        caps_list.push(wascap::caps::EVENTSTREAMS.to_string());
    }
    caps_list.extend(cmd.metadata.custom_caps.iter().cloned());

    if cmd.metadata.provider && caps_list.len() > 1 {
        return Err("Capability providers cannot provide multiple capabilities at once.".into());
    }

    let signed = sign_buffer_with_claims(
        cmd.metadata.name.clone(),
        &buf,
        subject,
        issuer,
        cmd.metadata.common.expires_in_days,
        cmd.metadata.common.not_before_days,
        caps_list.clone(),
        cmd.metadata.tags.clone(),
        cmd.metadata.provider,
        cmd.metadata.rev,
        cmd.metadata.ver.clone(),
    )?;

    let destination = match cmd.destination.clone() {
        Some(d) => d,
        None => {
            let path = PathBuf::from(cmd.source.clone())
                .parent()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let module_name = PathBuf::from(cmd.source.clone())
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            // If path is empty, user supplied module in current directory
            if path.is_empty() {
                format!("./{}_s.wasm", module_name)
            } else {
                format!("{}/{}_s.wasm", path, module_name)
            }
        }
    };

    let mut outfile = File::create(&destination).unwrap();
    let output = match outfile.write(&signed) {
        Ok(_) => Ok(format_output(
            format!(
                "Successfully signed {} with capabilities: {}",
                destination,
                caps_list.join(",")
            ),
            json!({"result": "success", "destination": destination, "capabilities": caps_list}),
            &cmd.metadata.common.output.kind,
        )),
        Err(e) => Err(Box::new(e)),
    }?;

    println!("{}", output);
    Ok(())
}

async fn render_caps(cmd: InspectCommand) -> Result<(), Box<dyn ::std::error::Error>> {
    let module_bytes = match File::open(&cmd.module) {
        Ok(mut f) => {
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).unwrap();
            buf
        }
        Err(_) => {
            crate::reg::pull_artifact(
                cmd.module.to_string(),
                cmd.digest,
                cmd.allow_latest,
                cmd.user,
                cmd.password,
                cmd.insecure,
            )
            .await?
        }
    };

    // Extract will return an error if it encounters an invalid hash in the claims
    let claims = wascap::wasm::extract_claims(&module_bytes);
    let out = match claims {
        Ok(Some(token)) => {
            if cmd.jwt_only {
                Ok(token.jwt)
            } else {
                let validation = wascap::jwt::validate_token::<Actor>(&token.jwt)?;
                Ok(render_actor_claims(token.claims, validation, &cmd.output))
            }
        }
        Err(e) => Err(Box::new(e)),
        Ok(None) => Ok(format!("No capabilities discovered in : {}", &cmd.module)),
    }?;

    println!("{}", out);
    Ok(())
}

/// Renders actor claims into provided output format
fn render_actor_claims(
    claims: Claims<Actor>,
    validation: TokenValidation,
    output: &Output,
) -> String {
    let md = claims.metadata.clone().unwrap();
    let friendly_rev = md.rev.unwrap_or(0);
    let friendly_ver = md.ver.unwrap_or_else(|| "None".to_string());
    let friendly = format!("{} ({})", friendly_ver, friendly_rev);
    let provider = if md.provider {
        "Capability Provider"
    } else {
        "Capabilities"
    };

    let tags = if let Some(tags) = &claims.metadata.as_ref().unwrap().tags {
        if tags.is_empty() {
            "None".to_string()
        } else {
            tags.join(",")
        }
    } else {
        "None".to_string()
    };

    let friendly_caps: Vec<String> = if let Some(caps) = &claims.metadata.as_ref().unwrap().caps {
        caps.iter().map(|c| capability_name(&c)).collect()
    } else {
        vec![]
    };

    match output.kind {
        OutputKind::JSON => {
            let iss_label = token_label(&claims.issuer).to_ascii_lowercase();
            let sub_label = token_label(&claims.subject).to_ascii_lowercase();
            let provider_json = provider.replace(" ", "_").to_ascii_lowercase();
            format!(
                "{}",
                json!({ iss_label: claims.issuer,
                    sub_label: claims.subject,
                    "expires": validation.expires_human,
                    "can_be_used": validation.not_before_human,
                    "version": friendly_ver,
                    provider_json: friendly_caps,
                    "tags": tags })
            )
        }
        OutputKind::Text => {
            let mut table = render_core(&claims, validation);

            table.add_row(Row::new(vec![
                TableCell::new("Version"),
                TableCell::new_with_alignment(friendly, 1, Alignment::Right),
            ]));

            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                provider,
                2,
                Alignment::Center,
            )]));

            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                friendly_caps.join("\n"),
                2,
                Alignment::Left,
            )]));

            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                "Tags",
                2,
                Alignment::Center,
            )]));

            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                tags,
                2,
                Alignment::Left,
            )]));

            table.render()
        }
    }
}

// * - we don't need render impls for Operator or Account because those tokens are never embedded into a module,
// only actors.

fn token_label(pk: &str) -> String {
    match pk.chars().nth(0).unwrap() {
        'A' => "Account".to_string(),
        'M' => "Module".to_string(),
        'O' => "Operator".to_string(),
        'S' => "Server".to_string(),
        'U' => "User".to_string(),
        _ => "<Unknown>".to_string(),
    }
}

fn render_core<T>(claims: &Claims<T>, validation: TokenValidation) -> Table
where
    T: serde::Serialize + DeserializeOwned + WascapEntity,
{
    let mut table = Table::new();
    table.max_column_width = 68;
    table.style = TableStyle::extended();
    let headline = format!("{} - {}", claims.name(), token_label(&claims.subject));

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        headline,
        2,
        Alignment::Center,
    )]));

    table.add_row(Row::new(vec![
        TableCell::new(token_label(&claims.issuer)),
        TableCell::new_with_alignment(&claims.issuer, 1, Alignment::Right),
    ]));
    table.add_row(Row::new(vec![
        TableCell::new(token_label(&claims.subject)),
        TableCell::new_with_alignment(&claims.subject, 1, Alignment::Right),
    ]));

    table.add_row(Row::new(vec![
        TableCell::new("Expires"),
        TableCell::new_with_alignment(validation.expires_human, 1, Alignment::Right),
    ]));

    table.add_row(Row::new(vec![
        TableCell::new("Can Be Used"),
        TableCell::new_with_alignment(validation.not_before_human, 1, Alignment::Right),
    ]));

    table
}

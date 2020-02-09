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

#[macro_use]
extern crate serde_derive;

use nkeys::KeyPair;
use std::fs::{read_to_string, File};
use std::io::Read;
use std::io::Write;
use structopt::clap::AppSettings;
use structopt::StructOpt;
use wascap::jwt::{Account, Actor, Claims, Operator};
use wascap::wasm::{days_from_now_to_jwt_time, sign_buffer_with_claims};

#[derive(Debug, StructOpt, Clone)]
#[structopt(
    global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
    name = "wascap", 
    about = "A command line utility for viewing, manipulating, and verifying capability claims in WebAssembly modules")]
struct Cli {
    #[structopt(flatten)]
    command: CliCommand,
}

#[derive(Debug, Clone, StructOpt)]
enum CliCommand {
    /// Examine the capabilities of a WebAssembly module
    #[structopt(name = "caps")]
    Caps {
        /// The file to read
        file: String,
        /// Extract the raw JWT from the file and print to stdout
        #[structopt(name = "raw", short = "r", long = "raw")]
        raw: bool,
    },
    /// Sign a WebAssembly module, specifying capabilities and other claims
    /// including expiration, tags, and additional metadata
    #[structopt(name = "sign")]
    Sign(SignCommand),
    /// Generate a signed JWT by supplying basic token information, a signing seed key, and metadata
    #[structopt(name = "gen")]
    Generate(GenerateCommand),
}

#[derive(Debug, Clone, StructOpt)]
enum GenerateCommand {
    /// Generate a signed JWT for an actor module
    #[structopt(name = "actor")]
    Actor(ActorMetadata),
    /// Generate a signed JWT for an operator
    #[structopt(name = "operator")]
    Operator(OperatorMetadata),
    /// Generate a signed JWT for an account
    #[structopt(name = "account")]
    Account(AccountMetadata),
}

#[derive(Debug, Clone, StructOpt, Serialize, Deserialize)]
struct GenerateCommon {
    /// Issuer seed key path (usually a .nk file)
    #[structopt(short = "i", long = "issuer")]
    issuer_key_path: String,

    /// Subject seed key path (usually a .nk file)
    #[structopt(short = "u", long = "subject")]
    subject_key_path: String,

    /// Indicates the token expires in the given amount of days. If this option is left off, the token will never expire
    #[structopt(short = "x", long = "expires")]
    expires_in_days: Option<u64>,
    /// Period in days that must elapse before this token is valid. If this option is left off, the token will be valid immediately
    #[structopt(short = "b", long = "nbf")]
    not_before_days: Option<u64>,
}

#[derive(Debug, Clone, StructOpt)]
struct OperatorMetadata {
    /// A descriptive name for the operator
    #[structopt(short = "n", long = "name")]
    name: String,

    #[structopt(flatten)]
    common: GenerateCommon,
}

#[derive(Debug, Clone, StructOpt)]
struct AccountMetadata {
    /// A descriptive name for the account
    #[structopt(short = "n", long = "name")]
    name: String,

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
    #[structopt(short = "s", long = "http_server")]
    http_server: bool,
    /// Enable the HTTP client standard capability
    #[structopt(short = "h", long = "http_client")]
    http_client: bool,
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

    #[structopt(flatten)]
    common: GenerateCommon,
}

#[derive(StructOpt, Debug, Clone)]
struct SignCommand {
    /// File to read
    source: String,
    /// Target output file
    output: String,

    #[structopt(flatten)]
    metadata: ActorMetadata,
}

fn main() -> Result<(), Box<dyn ::std::error::Error>> {
    let args = Cli::from_args();
    let cmd = args.command;
    env_logger::init();

    match handle_command(cmd) {
        Ok(_) => {},
        Err(e) => {
            println!("Command line failure: {}", e);
        }
    }
    Ok(())
}

fn handle_command(cmd: CliCommand) -> Result<(), Box<dyn ::std::error::Error>> {
    match cmd {
        CliCommand::Caps { file, raw } => render_caps(&file, raw),
        CliCommand::Sign(signcmd) => sign_file(&signcmd),
        CliCommand::Generate(gencmd) => generate_token(&gencmd),
    }
}

fn generate_token(cmd: &GenerateCommand) -> Result<(), Box<dyn ::std::error::Error>> {
    match cmd {
        GenerateCommand::Actor(actor) => generate_actor(actor),
        GenerateCommand::Operator(operator) => generate_operator(operator),
        GenerateCommand::Account(account) => generate_account(account),
    }
}

fn get_keypairs(
    common: &GenerateCommon,
) -> Result<(KeyPair, KeyPair), Box<dyn ::std::error::Error>> {
    if common.issuer_key_path.is_empty() {
        return Err("Must specify an issuer key path".into());
    }
    if common.subject_key_path.is_empty() {
        return Err("Must specify a subject key path".into());
    }
    let iss_key = read_to_string(&common.issuer_key_path)?;
    let sub_key = read_to_string(&common.subject_key_path)?;
    let issuer = KeyPair::from_seed(iss_key.trim_end())?;
    let subject = KeyPair::from_seed(sub_key.trim_end())?;

    Ok((issuer, subject))
}

fn generate_actor(actor: &ActorMetadata) -> Result<(), Box<dyn ::std::error::Error>> {
    let (issuer, subject) = get_keypairs(&actor.common)?;
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
    println!("{}", claims.encode(&issuer)?);
    Ok(())
}

fn generate_operator(operator: &OperatorMetadata) -> Result<(), Box<dyn ::std::error::Error>> {
    let (issuer, subject) = get_keypairs(&operator.common)?;
    let claims: Claims<Operator> = Claims::<Operator>::with_dates(
        operator.name.clone(),
        issuer.public_key(),
        subject.public_key(),
        days_from_now_to_jwt_time(operator.common.not_before_days),
        days_from_now_to_jwt_time(operator.common.expires_in_days),
    );
    println!("{}", claims.encode(&issuer)?);
    Ok(())
}

fn generate_account(account: &AccountMetadata) -> Result<(), Box<dyn ::std::error::Error>> {
    let (issuer, subject) = get_keypairs(&account.common)?;
    let claims: Claims<Account> = Claims::<Account>::with_dates(
        account.name.clone(),
        issuer.public_key(),
        subject.public_key(),
        days_from_now_to_jwt_time(account.common.not_before_days),
        days_from_now_to_jwt_time(account.common.expires_in_days),
    );
    println!("{}", claims.encode(&issuer)?);
    Ok(())
}

fn sign_file(cmd: &SignCommand) -> Result<(), Box<dyn ::std::error::Error>> {
    let mut sfile = File::open(&cmd.source).unwrap();
    let mut buf = Vec::new();
    sfile.read_to_end(&mut buf).unwrap();

    let mod_kp = if !cmd.metadata.common.subject_key_path.is_empty() {
        KeyPair::from_seed(&read_to_string(&cmd.metadata.common.subject_key_path)?.trim_end())?
    } else {
        let m = KeyPair::new_module();
        println!("New module key created. SAVE this seed key: {}", m.seed()?);
        m
    };

    let acct_kp = if !cmd.metadata.common.issuer_key_path.is_empty() {
        KeyPair::from_seed(&read_to_string(&cmd.metadata.common.issuer_key_path)?.trim_end())? 
    } else {
        let a = KeyPair::new_account();
        println!("New account key created. SAVE this seed key: {}", a.seed()?);
        a
    };

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
    caps_list.extend(cmd.metadata.custom_caps.iter().cloned());

    if cmd.metadata.provider && caps_list.len() > 1 {
        return Err("Capability providers cannot provide multiple capabilities at once.".into());
    }

    let signed = sign_buffer_with_claims(
        cmd.metadata.name.clone(),
        &buf,
        mod_kp,
        acct_kp,
        cmd.metadata.common.expires_in_days,
        cmd.metadata.common.not_before_days,
        caps_list.clone(),
        cmd.metadata.tags.clone(),
        cmd.metadata.provider,
        cmd.metadata.rev,
        cmd.metadata.ver.clone(),
    )?;

    let mut outfile = File::create(&cmd.output).unwrap();
    match outfile.write(&signed) {
        Ok(_) => {
            println!(
                "Successfully signed {} with capabilities: {}",
                cmd.output,
                caps_list.join(",")
            );
            Ok(())
        }
        Err(e) => Err(Box::new(e)),
    }
}

fn render_caps(file: &str, raw: bool) -> Result<(), Box<dyn ::std::error::Error>> {
    let mut wfile = File::open(&file).unwrap();
    let mut buf = Vec::new();
    wfile.read_to_end(&mut buf).unwrap();

    // Extract will return an error if it encounters an invalid hash in the claims
    let claims = wascap::wasm::extract_claims(&buf);
    match claims {
        Ok(Some(token)) => {
            if raw {
                println!("{}", &token.jwt);
            } else {
                let validation = wascap::jwt::validate_token::<Actor>(&token.jwt)?;
                println!("{}", token.claims.render(validation));
            }
            Ok(())
        }
        Err(e) => {
            println!("Error reading capabilities: {}", e);
            Ok(())
        }
        Ok(None) => {
            println!("No capabilities discovered in : {}", &file);
            Ok(())
        }
    }
}

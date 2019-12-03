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

use nkeys::KeyPair;
use std::fs::{read_to_string, File};
use std::io::Read;
use std::io::Write;
use structopt::clap::AppSettings;
use structopt::StructOpt;
use wascap::cli::emit_claims;
use wascap::wasm::sign_buffer_with_claims;

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
}

#[derive(StructOpt, Debug, Clone)]
struct SignCommand {
    /// File to read
    source: String,
    /// Target output file
    output: String,
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

    /// Add custom capabilities
    #[structopt(short = "c", long = "cap", name = "capabilities")]
    custom_caps: Vec<String>,
    /// Indicates the token expires in the given amount of days. If this option is left off, the token will never expire
    #[structopt(short = "x", long = "expires")]
    expires_in_days: Option<u64>,
    /// Period in days that must elapse before this token is valid. If this option is left off, the token will be valid immediately
    #[structopt(short = "b", long = "nbf")]
    not_before_days: Option<u64>,
    /// Path to the account (signer)'s nkey file. If one is not present, one will be created
    #[structopt(short = "a", long = "acct")]
    acct_signer_path: Option<String>,
    /// Path to the module's nkey file. If one is not present, one will be created
    #[structopt(short = "m", long = "mod")]
    mod_key_path: Option<String>,
    /// A list of arbitrary tags to be embedded in the token
    #[structopt(short = "t", long = "tag")]
    tags: Vec<String>,
}

fn main() -> Result<(), Box<dyn ::std::error::Error>> {
    let args = Cli::from_args();
    let cmd = args.command;
    env_logger::init();

    handle_command(cmd)
}

fn handle_command(cmd: CliCommand) -> Result<(), Box<dyn ::std::error::Error>> {
    match cmd {
        CliCommand::Caps { file, raw } => render_caps(&file, raw),
        CliCommand::Sign(signcmd) => sign_file(&signcmd),
    }
}

fn sign_file(cmd: &SignCommand) -> Result<(), Box<dyn ::std::error::Error>> {
    let mut sfile = File::open(&cmd.source).unwrap();
    let mut buf = Vec::new();
    sfile.read_to_end(&mut buf).unwrap();

    let mod_kp = if let Some(p) = &cmd.mod_key_path {
        let kp = KeyPair::from_seed(&read_to_string(p)?.trim_end());
        match kp {
            Ok(pair) => pair,
            Err(e) => panic!("Failed to read module seed key: {}", e),
        }
    } else {
        let m = KeyPair::new_module();
        println!("New module key created. SAVE this seed key: {}", m.seed()?);
        m
    };

    let acct_kp = if let Some(p) = &cmd.acct_signer_path {
        let kp = KeyPair::from_seed(&read_to_string(p)?.trim_end());
        match kp {
            Ok(pair) => pair,
            Err(e) => panic!("Failed to read account seed key: {}", e),
        }
    } else {
        let a = KeyPair::new_account();
        println!("New account key created. SAVE this seed key: {}", a.seed()?);
        a
    };

    let mut caps_list = vec![];
    if cmd.keyvalue {
        caps_list.push(wascap::caps::KEY_VALUE.to_string());
    }
    if cmd.msg_broker {
        caps_list.push(wascap::caps::MESSAGING.to_string());
    }
    if cmd.http_client {
        caps_list.push(wascap::caps::HTTP_CLIENT.to_string());
    }
    if cmd.http_server {
        caps_list.push(wascap::caps::HTTP_SERVER.to_string());
    }
    caps_list.extend(cmd.custom_caps.iter().cloned());

    let signed = sign_buffer_with_claims(
        &buf,
        mod_kp,
        acct_kp,
        cmd.expires_in_days,
        cmd.not_before_days,
        caps_list,
        cmd.tags.clone(),
    )?;

    let mut outfile = File::create(&cmd.output).unwrap();
    match outfile.write(&signed) {
        Ok(_) => {
            println!("Successfully signed {}.", cmd.output);
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
                emit_claims(&token.claims, &token.jwt);
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

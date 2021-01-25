extern crate provider_archive;
use crate::keys::extract_keypair;
use crate::util::{convert_error, format_output, Output, OutputKind, Result};
use nkeys::KeyPairType;
use provider_archive::*;
use serde_json::json;
use std::fs::File;
use std::io::prelude::*;
use std::path::PathBuf;
use structopt::clap::AppSettings;
use structopt::StructOpt;

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

#[derive(Debug, StructOpt, Clone)]
#[structopt(
    global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
    name = "par")]
pub(crate) struct ParCli {
    #[structopt(flatten)]
    command: ParCliCommand,
}

impl ParCli {
    pub(crate) fn command(self) -> ParCliCommand {
        self.command
    }
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) enum ParCliCommand {
    /// Build a provider archive file
    #[structopt(name = "create")]
    Create(CreateCommand),
    /// Inspect a provider archive file
    #[structopt(name = "inspect")]
    Inspect(InspectCommand),
    /// Insert a provider into a provider archive file
    #[structopt(name = "insert")]
    Insert(InsertCommand),
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct CreateCommand {
    /// Capability contract ID (e.g. wascc:messaging or wascc:keyvalue).
    #[structopt(short = "c", long = "capid")]
    capid: String,

    /// Vendor string to help identify the publisher of the provider (e.g. Redis, Cassandra, waSCC, etc). Not unique.
    #[structopt(short = "v", long = "vendor")]
    vendor: String,

    /// Monotonically increasing revision number
    #[structopt(short = "r", long = "revision")]
    revision: Option<i32>,

    /// Human friendly version string
    #[structopt(long = "version")]
    version: Option<String>,

    /// Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
    #[structopt(
        short = "d",
        long = "directory",
        env = "WASH_KEYS",
        hide_env_values = true
    )]
    directory: Option<String>,

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

    /// Name of the capability provider
    #[structopt(short = "n", long = "name")]
    name: String,

    /// Architecture of provider binary in format ARCH-OS (e.g. x86_64-linux)
    #[structopt(short = "a", long = "arch")]
    arch: String,

    /// Path to provider binary for populating the archive
    #[structopt(short = "b", long = "binary")]
    binary: String,

    /// File output destination path
    #[structopt(long = "destination")]
    destination: Option<String>,

    /// Include a compressed provider archive
    #[structopt(long = "compress")]
    compress: bool,

    /// Disables autogeneration of signing keys
    #[structopt(long = "disable-keygen")]
    disable_keygen: bool,

    #[structopt(flatten)]
    pub(crate) output: Output,
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct InspectCommand {
    /// Path to provider archive or OCI URL of provider archive
    #[structopt(name = "archive")]
    archive: String,

    /// Digest to verify artifact against (if OCI URL is provided for <archive>)
    #[structopt(short = "d", long = "digest")]
    digest: Option<String>,

    /// Allow latest artifact tags (if OCI URL is provided for <archive>)
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
pub(crate) struct InsertCommand {
    /// Path to provider archive
    #[structopt(name = "archive")]
    archive: String,

    /// Architecture of binary in format ARCH-OS (e.g. x86_64-linux)
    #[structopt(short = "a", long = "arch")]
    arch: String,

    /// Path to provider binary to insert into archive
    #[structopt(short = "b", long = "binary")]
    binary: String,

    /// Location of key files for signing. Defaults to $WASH_KEYS ($HOME/.wash/keys)
    #[structopt(
        short = "d",
        long = "directory",
        env = "WASH_KEYS",
        hide_env_values = true
    )]
    directory: Option<String>,

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

    /// Disables autogeneration of signing keys
    #[structopt(long = "disable-keygen")]
    disable_keygen: bool,

    #[structopt(flatten)]
    pub(crate) output: Output,
}

pub(crate) async fn handle_command(command: ParCliCommand) -> Result<String> {
    match command {
        ParCliCommand::Create(cmd) => handle_create(cmd),
        ParCliCommand::Inspect(cmd) => handle_inspect(cmd).await,
        ParCliCommand::Insert(cmd) => handle_insert(cmd),
    }
}

/// Creates a provider archive using an initial architecture target, provider, and signing keys
pub(crate) fn handle_create(cmd: CreateCommand) -> Result<String> {
    let mut par = ProviderArchive::new(
        &cmd.capid,
        &cmd.name,
        &cmd.vendor,
        cmd.revision,
        cmd.version,
    );

    let mut f = File::open(cmd.binary.clone())?;
    let mut lib = Vec::new();
    f.read_to_end(&mut lib)?;

    let issuer = extract_keypair(
        cmd.issuer,
        Some(cmd.binary.clone()),
        cmd.directory.clone(),
        KeyPairType::Account,
        cmd.disable_keygen,
    )?;
    let subject = extract_keypair(
        cmd.subject,
        Some(cmd.binary.clone()),
        cmd.directory,
        KeyPairType::Service,
        cmd.disable_keygen,
    )?;

    par.add_library(&cmd.arch, &lib).map_err(convert_error)?;

    let extension = if cmd.compress { ".par.gz" } else { ".par" };
    let outfile = match cmd.destination {
        Some(path) => path,
        None => format!(
            "{}{}",
            PathBuf::from(cmd.binary.clone())
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .to_string(),
            extension
        ),
    };

    Ok(
        if par
            .write(&outfile, &issuer, &subject, cmd.compress)
            .is_err()
        {
            format!(
                "Error writing PAR. Please ensure directory {:?} exists",
                PathBuf::from(outfile).parent().unwrap(),
            )
        } else {
            format_output(
                format!("Successfully created archive {}", outfile),
                json!({"result": "success", "file": outfile}),
                &cmd.output,
            )
        },
    )
}

/// Loads a provider archive and outputs the contents of the claims
pub(crate) async fn handle_inspect(cmd: InspectCommand) -> Result<String> {
    let archive = match File::open(&cmd.archive) {
        Ok(mut f) => {
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            ProviderArchive::try_load(&buf).map_err(|e| format!("{}", e))?
        }
        Err(_) => {
            let artifact = crate::reg::pull_artifact(
                cmd.archive,
                cmd.digest,
                cmd.allow_latest,
                cmd.user,
                cmd.password,
                cmd.insecure,
            )
            .await?;
            ProviderArchive::try_load(&artifact).map_err(|e| format!("{}", e))?
        }
    };
    let claims = archive.claims().unwrap();
    let metadata = claims.metadata.unwrap();

    let output = match cmd.output.kind {
        OutputKind::JSON => {
            let friendly_rev = if metadata.rev.is_some() {
                format!("{}", metadata.rev.unwrap())
            } else {
                "None".to_string()
            };
            let friendly_ver = metadata.ver.unwrap_or_else(|| "None".to_string());
            format!(
                "{}",
                json!({"name": metadata.name.unwrap(),
                    "public_key": claims.subject,
                    "capability_contract_id": metadata.capid,
                    "vendor": metadata.vendor,
                    "ver": friendly_ver,
                    "rev": friendly_rev,
                    "targets": archive.targets()})
            )
        }
        OutputKind::Text => {
            use term_table::row::Row;
            use term_table::table_cell::*;
            use term_table::{Table, TableStyle};

            let mut table = Table::new();
            table.max_column_width = 68;
            table.style = TableStyle::extended();

            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                format!("{} - Provider Archive", metadata.name.unwrap()),
                2,
                Alignment::Center,
            )]));

            table.add_row(Row::new(vec![
                TableCell::new("Public Key"),
                TableCell::new_with_alignment(claims.subject, 1, Alignment::Right),
            ]));
            table.add_row(Row::new(vec![
                TableCell::new("Capability Contract ID"),
                TableCell::new_with_alignment(metadata.capid, 1, Alignment::Right),
            ]));
            table.add_row(Row::new(vec![
                TableCell::new("Vendor"),
                TableCell::new_with_alignment(metadata.vendor, 1, Alignment::Right),
            ]));

            if let Some(ver) = metadata.ver {
                table.add_row(Row::new(vec![
                    TableCell::new("Version"),
                    TableCell::new_with_alignment(ver, 1, Alignment::Right),
                ]));
            }

            if let Some(rev) = metadata.rev {
                table.add_row(Row::new(vec![
                    TableCell::new("Revision"),
                    TableCell::new_with_alignment(rev, 1, Alignment::Right),
                ]));
            }

            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                "Supported Architecture Targets",
                2,
                Alignment::Center,
            )]));

            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                archive.targets().join("\n"),
                2,
                Alignment::Left,
            )]));

            table.render()
        }
    };

    Ok(output)
}

/// Loads a provider archive and attempts to insert an additional provider into it
pub(crate) fn handle_insert(cmd: InsertCommand) -> Result<String> {
    let mut buf = Vec::new();
    let mut f = File::open(cmd.archive.clone())?;
    f.read_to_end(&mut buf)?;

    let mut par = ProviderArchive::try_load(&buf).map_err(convert_error)?;

    let issuer = extract_keypair(
        cmd.issuer,
        Some(cmd.binary.clone()),
        cmd.directory.clone(),
        KeyPairType::Account,
        cmd.disable_keygen,
    )?;
    let subject = extract_keypair(
        cmd.subject,
        Some(cmd.binary.clone()),
        cmd.directory,
        KeyPairType::Service,
        cmd.disable_keygen,
    )?;

    let mut f = File::open(cmd.binary.clone())?;
    let mut lib = Vec::new();
    f.read_to_end(&mut lib)?;

    par.add_library(&cmd.arch, &lib).map_err(convert_error)?;

    par.write(&cmd.archive, &issuer, &subject, is_compressed(&buf)?)
        .map_err(convert_error)?;

    Ok(format_output(
        format!(
            "Successfully inserted {} into archive {}",
            cmd.binary, cmd.archive
        ),
        json!({"result": "success", "file": cmd.archive}),
        &cmd.output,
    ))
}

/// Inspects the byte slice for a GZIP header, and returns true if the file is compressed
fn is_compressed(input: &[u8]) -> Result<bool> {
    if input.len() < 2 {
        return Err("Not enough bytes to be a valid PAR file".into());
    }
    Ok(input[0..2] == GZIP_MAGIC)
}

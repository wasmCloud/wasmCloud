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
    /// Capability contract ID (e.g. wasmcloud:messaging or wasmcloud:keyvalue).
    #[structopt(short = "c", long = "capid")]
    capid: String,

    /// Vendor string to help identify the publisher of the provider (e.g. Redis, Cassandra, wasmcloud, etc). Not unique.
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
    directory: Option<PathBuf>,

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

    /// skip the local OCI cache
    #[structopt(long = "no-cache")]
    no_cache: bool,

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
    directory: Option<PathBuf>,

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
                &cmd.output.kind,
            )
        },
    )
}

/// Loads a provider archive and outputs the contents of the claims
pub(crate) async fn handle_inspect(cmd: InspectCommand) -> Result<String> {
    let artifact_bytes = crate::reg::get_artifact(
        cmd.archive,
        cmd.digest,
        cmd.allow_latest,
        cmd.user,
        cmd.password,
        cmd.insecure,
        cmd.no_cache,
    )
    .await?;
    let artifact = ProviderArchive::try_load(&artifact_bytes).map_err(|e| format!("{}", e))?;
    let claims = artifact.claims().unwrap();
    let metadata = claims.metadata.unwrap();

    let output = match cmd.output.kind {
        OutputKind::Json => {
            let friendly_rev = if metadata.rev.is_some() {
                format!("{}", metadata.rev.unwrap())
            } else {
                "None".to_string()
            };
            let friendly_ver = metadata.ver.unwrap_or_else(|| "None".to_string());
            format!(
                "{}",
                json!({"name": metadata.name.unwrap(),
                    "issuer": claims.issuer,
                    "service": claims.subject,
                    "capability_contract_id": metadata.capid,
                    "vendor": metadata.vendor,
                    "ver": friendly_ver,
                    "rev": friendly_rev,
                    "targets": artifact.targets()})
            )
        }
        OutputKind::Text => {
            use term_table::row::Row;
            use term_table::table_cell::*;
            use term_table::Table;

            let mut table = Table::new();
            crate::util::configure_table_style(&mut table);

            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                format!("{} - Provider Archive", metadata.name.unwrap()),
                2,
                Alignment::Center,
            )]));

            table.add_row(Row::new(vec![
                TableCell::new("Account"),
                TableCell::new_with_alignment(claims.issuer, 1, Alignment::Right),
            ]));
            table.add_row(Row::new(vec![
                TableCell::new("Service"),
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
                artifact.targets().join("\n"),
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
        &cmd.output.kind,
    ))
}

/// Inspects the byte slice for a GZIP header, and returns true if the file is compressed
fn is_compressed(input: &[u8]) -> Result<bool> {
    if input.len() < 2 {
        return Err("Not enough bytes to be a valid PAR file".into());
    }
    Ok(input[0..2] == GZIP_MAGIC)
}

#[cfg(test)]
mod test {
    use super::*;

    // Uses all flags and options of the `par create` command
    // to ensure API does not change between versions
    #[test]
    fn test_par_create_comprehensive() {
        const ISSUER: &str = "SAAJLQZDZO57THPTIIEELEY7FJYOJZQWQD7FF4J67TUYTSCOXTF7R4Y3VY";
        const SUBJECT: &str = "SVAH7IN6QE6XODCGIIWZQDZ5LNSSS4FNEO6SNHZSSASW4BBBKSZ6KWTKWY";
        let create_long = ParCli::from_iter_safe(&[
            "par",
            "create",
            "--arch",
            "x86_64-testrunner",
            "--binary",
            "./testrunner.so",
            "--capid",
            "wasmcloud:test",
            "--name",
            "CreateTest",
            "--vendor",
            "TestRunner",
            "--destination",
            "./test.par.gz",
            "--revision",
            "1",
            "--version",
            "1.11.111",
            "--directory",
            "./tests/fixtures",
            "--issuer",
            ISSUER,
            "--subject",
            SUBJECT,
            "--output",
            "text",
            "--disable-keygen",
            "--compress",
        ])
        .unwrap();
        match create_long.command {
            ParCliCommand::Create(CreateCommand {
                capid,
                vendor,
                revision,
                version,
                directory,
                issuer,
                subject,
                name,
                arch,
                binary,
                destination,
                compress,
                disable_keygen,
                output,
            }) => {
                assert_eq!(capid, "wasmcloud:test");
                assert_eq!(arch, "x86_64-testrunner");
                assert_eq!(binary, "./testrunner.so");
                assert_eq!(directory.unwrap(), PathBuf::from("./tests/fixtures"));
                assert_eq!(issuer.unwrap(), ISSUER);
                assert_eq!(subject.unwrap(), SUBJECT);
                assert_eq!(output.kind, OutputKind::Text);
                assert_eq!(name, "CreateTest");
                assert_eq!(vendor, "TestRunner");
                assert_eq!(destination.unwrap(), "./test.par.gz");
                assert_eq!(revision.unwrap(), 1);
                assert_eq!(version.unwrap(), "1.11.111");
                assert!(disable_keygen);
                assert!(compress);
            }
            cmd => panic!("par insert constructed incorrect command {:?}", cmd),
        }
        let create_short = ParCli::from_iter_safe(&[
            "par",
            "create",
            "-a",
            "x86_64-testrunner",
            "-b",
            "./testrunner.so",
            "-c",
            "wasmcloud:test",
            "-n",
            "CreateTest",
            "-v",
            "TestRunner",
            "--destination",
            "./test.par.gz",
            "-r",
            "1",
            "--version",
            "1.11.111",
            "-d",
            "./tests/fixtures",
            "-i",
            ISSUER,
            "-s",
            SUBJECT,
            "-o",
            "json",
        ])
        .unwrap();
        match create_short.command {
            ParCliCommand::Create(CreateCommand {
                capid,
                vendor,
                revision,
                version,
                directory,
                issuer,
                subject,
                name,
                arch,
                binary,
                destination,
                compress,
                disable_keygen,
                output,
            }) => {
                assert_eq!(capid, "wasmcloud:test");
                assert_eq!(arch, "x86_64-testrunner");
                assert_eq!(binary, "./testrunner.so");
                assert_eq!(directory.unwrap(), PathBuf::from("./tests/fixtures"));
                assert_eq!(issuer.unwrap(), ISSUER);
                assert_eq!(subject.unwrap(), SUBJECT);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(name, "CreateTest");
                assert_eq!(vendor, "TestRunner");
                assert_eq!(destination.unwrap(), "./test.par.gz");
                assert_eq!(revision.unwrap(), 1);
                assert_eq!(version.unwrap(), "1.11.111");
                assert!(!disable_keygen);
                assert!(!compress);
            }
            cmd => panic!("par insert constructed incorrect command {:?}", cmd),
        }
    }

    // Uses all flags and options of the `par insert` command
    // to ensure API does not change between versions
    #[test]
    fn test_par_insert_comprehensive() {
        const ISSUER: &str = "SAAJLQZDZO57THPTQLEELEY7FJYOJZQWQD7FF4J67TUYTSCOXTF7R4Y3VY";
        const SUBJECT: &str = "SVAH7IN6QE6XODCGQAWZQDZ5LNSSS4FNEO6SNHZSSASW4BBBKSZ6KWTKWY";
        let insert_short = ParCli::from_iter_safe(&[
            "par",
            "insert",
            "libtest.par.gz",
            "-a",
            "x86_64-testrunner",
            "-b",
            "./testrunner.so",
            "-d",
            "./tests/fixtures",
            "-i",
            ISSUER,
            "-s",
            SUBJECT,
            "-o",
            "text",
            "--disable-keygen",
        ])
        .unwrap();
        match insert_short.command {
            ParCliCommand::Insert(InsertCommand {
                archive,
                arch,
                binary,
                directory,
                issuer,
                subject,
                output,
                disable_keygen,
            }) => {
                assert_eq!(archive, "libtest.par.gz");
                assert_eq!(arch, "x86_64-testrunner");
                assert_eq!(binary, "./testrunner.so");
                assert_eq!(directory.unwrap(), PathBuf::from("./tests/fixtures"));
                assert_eq!(issuer.unwrap(), ISSUER);
                assert_eq!(subject.unwrap(), SUBJECT);
                assert_eq!(output.kind, OutputKind::Text);
                assert!(disable_keygen);
            }
            cmd => panic!("par insert constructed incorrect command {:?}", cmd),
        }
        let insert_long = ParCli::from_iter_safe(&[
            "par",
            "insert",
            "libtest.par.gz",
            "--arch",
            "x86_64-testrunner",
            "--binary",
            "./testrunner.so",
            "--directory",
            "./tests/fixtures",
            "--issuer",
            ISSUER,
            "--subject",
            SUBJECT,
            "--output",
            "text",
        ])
        .unwrap();
        match insert_long.command {
            ParCliCommand::Insert(InsertCommand {
                archive,
                arch,
                binary,
                directory,
                issuer,
                subject,
                output,
                disable_keygen,
            }) => {
                assert_eq!(archive, "libtest.par.gz");
                assert_eq!(arch, "x86_64-testrunner");
                assert_eq!(binary, "./testrunner.so");
                assert_eq!(directory.unwrap(), PathBuf::from("./tests/fixtures"));
                assert_eq!(issuer.unwrap(), ISSUER);
                assert_eq!(subject.unwrap(), SUBJECT);
                assert_eq!(output.kind, OutputKind::Text);
                assert!(!disable_keygen);
            }
            cmd => panic!("par insert constructed incorrect command {:?}", cmd),
        }
    }

    // Uses all flags and options of the `par inspect` command
    // to ensure API does not change between versions
    #[test]
    fn test_par_inspect_comprehensive() {
        const LOCAL: &str = "./coolthing.par.gz";
        const REMOTE: &str = "wasmcloud.azurecr.io/coolthing.par.gz";

        let inspect_long = ParCli::from_iter_safe(&[
            "par",
            "inspect",
            LOCAL,
            "--digest",
            "sha256:blah",
            "--output",
            "json",
            "--password",
            "secret",
            "--user",
            "name",
            "--no-cache",
        ])
        .unwrap();
        match inspect_long.command {
            ParCliCommand::Inspect(InspectCommand {
                archive,
                digest,
                allow_latest,
                user,
                password,
                insecure,
                output,
                no_cache,
            }) => {
                assert_eq!(archive, LOCAL);
                assert_eq!(digest.unwrap(), "sha256:blah");
                assert!(!allow_latest);
                assert!(!insecure);
                assert_eq!(user.unwrap(), "name");
                assert_eq!(password.unwrap(), "secret");
                assert_eq!(output.kind, OutputKind::Json);
                assert!(no_cache);
            }
            cmd => panic!("par inspect constructed incorrect command {:?}", cmd),
        }
        let inspect_short = ParCli::from_iter_safe(&[
            "par",
            "inspect",
            REMOTE,
            "-d",
            "sha256:blah",
            "-o",
            "json",
            "-p",
            "secret",
            "-u",
            "name",
            "--allow-latest",
            "--insecure",
            "--no-cache",
        ])
        .unwrap();
        match inspect_short.command {
            ParCliCommand::Inspect(InspectCommand {
                archive,
                digest,
                allow_latest,
                user,
                password,
                insecure,
                output,
                no_cache,
            }) => {
                assert_eq!(archive, REMOTE);
                assert_eq!(digest.unwrap(), "sha256:blah");
                assert!(allow_latest);
                assert!(insecure);
                assert_eq!(user.unwrap(), "name");
                assert_eq!(password.unwrap(), "secret");
                assert_eq!(output.kind, OutputKind::Json);
                assert!(no_cache);
            }
            cmd => panic!("par inspect constructed incorrect command {:?}", cmd),
        }
    }
}

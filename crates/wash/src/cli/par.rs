use std::fs::File;
use std::io::Read;
use std::{collections::HashMap, path::PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use nkeys::KeyPairType;
use provider_archive::ProviderArchive;
use serde_json::json;
use tracing::warn;
use crate::lib::cli::par::{
    convert_error, create_provider_archive, detect_arch, insert_provider_binary,
};
use crate::lib::cli::{extract_keypair, inspect, par, CommandOutput, OutputKind};

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

#[derive(Debug, Clone, Subcommand)]
pub enum ParCliCommand {
    /// Build a provider archive file
    #[clap(name = "create")]
    Create(CreateCommand),
    /// Inspect a provider archive file
    #[clap(name = "inspect")]
    Inspect(InspectCommand),
    /// Insert a provider into a provider archive file
    #[clap(name = "insert")]
    Insert(InsertCommand),
}

#[derive(Parser, Debug, Clone)]
pub struct CreateCommand {
    /// Vendor string to help identify the publisher of the provider (e.g. Redis, Cassandra, wasmcloud, etc). Not unique.
    #[clap(short = 'v', long = "vendor")]
    vendor: String,

    /// Monotonically increasing revision number
    #[clap(short = 'r', long = "revision")]
    revision: Option<i32>,

    /// Human friendly version string
    #[clap(long = "version")]
    version: Option<String>,

    /// Optional path to a JSON schema describing the link definition specification for this provider.
    #[clap(
        short = 'j',
        long = "schema",
        env = "WASH_JSON_SCHEMA",
        hide_env_values = true
    )]
    schema: Option<PathBuf>,

    /// Location of key files for signing. Defaults to $`WASH_KEYS` ($HOME/.wash/keys)
    #[clap(
        short = 'd',
        long = "directory",
        env = "WASH_KEYS",
        hide_env_values = true
    )]
    directory: Option<PathBuf>,

    /// Path to issuer seed key (account). If this flag is not provided, the will be sourced from $`WASH_KEYS` ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 'i',
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    issuer: Option<String>,

    /// Path to subject seed key (service). If this flag is not provided, the will be sourced from $`WASH_KEYS` ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 's',
        long = "subject",
        env = "WASH_SUBJECT_KEY",
        hide_env_values = true
    )]
    subject: Option<String>,

    /// Name of the capability provider
    #[clap(short = 'n', long = "name")]
    name: String,

    /// Architecture of provider binary in format ARCH-OS (e.g. x86_64-linux)
    #[clap(short = 'a', long = "arch", default_value_t = detect_arch())]
    arch: String,

    /// Path to provider binary for populating the archive
    #[clap(short = 'b', long = "binary")]
    binary: String,

    /// File output destination path
    #[clap(long = "destination")]
    destination: Option<String>,

    /// Include a compressed provider archive
    #[clap(long = "compress")]
    compress: bool,

    /// Disables autogeneration of signing keys
    #[clap(long = "disable-keygen")]
    disable_keygen: bool,

    /// Location of project directory containing WIT
    #[clap(long = "wit-directory", env = "WIT_DIR")]
    wit_dir: Option<PathBuf>,
}

#[derive(Parser, Debug, Clone)]
pub struct InspectCommand {
    /// Path to provider archive or OCI URL of provider archive
    #[clap(name = "archive")]
    archive: String,

    /// Digest to verify artifact against (if OCI URL is provided for `<archive>`)
    #[clap(short = 'd', long = "digest")]
    digest: Option<String>,

    /// Allow latest artifact tags (if OCI URL is provided for `<archive>`)
    #[clap(long = "allow-latest")]
    allow_latest: bool,

    /// OCI username, if omitted anonymous authentication will be used
    #[clap(
        short = 'u',
        long = "user",
        env = "WASH_REG_USER",
        hide_env_values = true
    )]
    user: Option<String>,

    /// OCI password, if omitted anonymous authentication will be used
    #[clap(
        short = 'p',
        long = "password",
        env = "WASH_REG_PASSWORD",
        hide_env_values = true
    )]
    password: Option<String>,

    /// Allow insecure (HTTP) registry connections
    #[clap(long = "insecure")]
    insecure: bool,

    /// Skip checking OCI registry's certificate for validity
    #[clap(long = "insecure-skip-tls-verify")]
    pub insecure_skip_tls_verify: bool,

    /// skip the local OCI cache
    #[clap(long = "no-cache")]
    no_cache: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct InsertCommand {
    /// Path to provider archive
    #[clap(name = "archive")]
    archive: String,

    /// Architecture of binary in format ARCH-OS (e.g. x86_64-linux)
    #[clap(short = 'a', long = "arch", default_value_t = detect_arch())]
    arch: String,

    /// Path to provider binary to insert into archive
    #[clap(short = 'b', long = "binary")]
    binary: String,

    /// Location of key files for signing. Defaults to $`WASH_KEYS` ($HOME/.wash/keys)
    #[clap(
        short = 'd',
        long = "directory",
        env = "WASH_KEYS",
        hide_env_values = true
    )]
    directory: Option<PathBuf>,

    /// Path to issuer seed key (account). If this flag is not provided, the will be sourced from $`WASH_KEYS` ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 'i',
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    issuer: Option<String>,

    /// Path to subject seed key (service). If this flag is not provided, the will be sourced from $`WASH_KEYS` ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 's',
        long = "subject",
        env = "WASH_SUBJECT_KEY",
        hide_env_values = true
    )]
    subject: Option<String>,

    /// Disables autogeneration of signing keys
    #[clap(long = "disable-keygen")]
    disable_keygen: bool,
}

impl From<InspectCommand> for inspect::InspectCliCommand {
    fn from(cmd: InspectCommand) -> Self {
        Self {
            target: cmd.archive,
            jwt_only: false,
            wit: false,
            digest: cmd.digest,
            allow_latest: cmd.allow_latest,
            user: cmd.user,
            password: cmd.password,
            insecure: cmd.insecure,
            insecure_skip_tls_verify: cmd.insecure_skip_tls_verify,
            no_cache: cmd.no_cache,
        }
    }
}

impl From<CreateCommand> for par::ParCreateArgs {
    fn from(cmd: CreateCommand) -> Self {
        Self {
            vendor: cmd.vendor,
            revision: cmd.revision,
            version: cmd.version,
            schema: cmd.schema,
            name: cmd.name,
            arch: cmd.arch,
        }
    }
}

pub async fn handle_command(
    command: ParCliCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    match command {
        ParCliCommand::Create(cmd) => handle_create(cmd, output_kind).await,
        ParCliCommand::Inspect(cmd) => {
            warn!("par inspect will be deprecated in future versions. Use inspect instead.");
            inspect::handle_command(cmd, output_kind).await
        }
        ParCliCommand::Insert(cmd) => handle_insert(cmd, output_kind).await,
    }
}

/// Creates a provider archive using an initial architecture target, provider, and signing keys
pub async fn handle_create(cmd: CreateCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    let mut f = File::open(cmd.binary.clone())
        .with_context(|| format!("failed to load binary [{}]", &cmd.binary))?;
    let mut lib = Vec::new();
    f.read_to_end(&mut lib)?;

    let issuer = extract_keypair(
        cmd.issuer.as_deref(),
        Some(&cmd.binary),
        cmd.directory.clone(),
        KeyPairType::Account,
        cmd.disable_keygen,
        output_kind,
    )?;
    let subject = extract_keypair(
        cmd.subject.as_deref(),
        Some(&cmd.binary),
        cmd.directory.clone(),
        KeyPairType::Service,
        cmd.disable_keygen,
        output_kind,
    )?;

    let extension = if cmd.compress { ".par.gz" } else { ".par" };
    let outfile = match cmd.destination.clone() {
        Some(path) => path,
        None => format!(
            "{}{}",
            PathBuf::from(&cmd.binary)
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap(),
            extension
        ),
    };

    let wit_interface_bytes = match cmd.wit_dir.as_ref() {
        Some(dir) => {
            let mut resolve = wit_parser::Resolve::default();
            let (package_id, _paths) = resolve
                .push_dir(dir)
                .with_context(|| format!("failed to add WIT directory @ [{}]", dir.display()))?;

            let encoded = wit_component::encode(&resolve, package_id)
                .context("Failed to encode WIT package")?;

            Some(encoded)
        }
        None => None,
    };

    let compress = cmd.compress;
    let mut par = create_provider_archive(cmd.into(), &lib, wit_interface_bytes.as_deref())
        .context("failed to create provider archive with built provider")?;
    par.write(&outfile, &issuer, &subject, compress)
        .await
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| {
            format!(
                "Error writing PAR. Please ensure directory {:?} exists",
                PathBuf::from(&outfile).parent().unwrap(),
            )
        })?;

    let mut map = HashMap::new();
    map.insert("file".to_string(), json!(outfile));
    Ok(CommandOutput::new(
        format!("Successfully created archive {outfile}"),
        map,
    ))
}

/// Loads a provider archive and attempts to insert an additional provider into it
pub async fn handle_insert(cmd: InsertCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    let mut buf = Vec::new();
    let mut f = File::open(cmd.archive.clone())
        .with_context(|| format!("failed to load provider archive [{}]", &cmd.archive))?;
    f.read_to_end(&mut buf)?;

    let mut f = File::open(cmd.binary.clone())
        .with_context(|| format!("failed to load binary [{}]", &cmd.archive))?;
    let mut lib = Vec::new();
    f.read_to_end(&mut lib)?;

    let issuer = extract_keypair(
        cmd.issuer.as_deref(),
        Some(&cmd.binary),
        cmd.directory.clone(),
        KeyPairType::Account,
        cmd.disable_keygen,
        output_kind,
    )?;
    let subject = extract_keypair(
        cmd.subject.as_deref(),
        Some(&cmd.binary),
        cmd.directory.clone(),
        KeyPairType::Service,
        cmd.disable_keygen,
        output_kind,
    )?;

    let mut par = ProviderArchive::try_load(&buf)
        .await
        .map_err(convert_error)?;

    par = insert_provider_binary(cmd.arch, &lib, par).await?;
    par.write(&cmd.archive, &issuer, &subject, is_compressed(&buf)?)
        .await
        .map_err(convert_error)?;

    let mut map = HashMap::new();
    map.insert("file".to_string(), json!(cmd.archive));
    Ok(CommandOutput::new(
        format!(
            "Successfully inserted {} into archive {}",
            cmd.binary, cmd.archive
        ),
        map,
    ))
}

/// Inspects the byte slice for a GZIP header, and returns true if the file is compressed
fn is_compressed(input: &[u8]) -> Result<bool> {
    if input.len() < 2 {
        bail!("Not enough bytes to be a valid PAR file");
    }
    Ok(input[0..2] == GZIP_MAGIC)
}

#[cfg(test)]
mod test {
    use super::*;

    #[derive(Parser, Debug)]
    struct Cmd {
        #[clap(subcommand)]
        par: ParCliCommand,
    }

    // Uses all flags and options of the `par create` command
    // to ensure API does not change between versions
    #[test]
    fn test_par_create_comprehensive() {
        const ISSUER: &str = "SAAJLQZDZO57THPTIIEELEY7FJYOJZQWQD7FF4J67TUYTSCOXTF7R4Y3VY";
        const SUBJECT: &str = "SVAH7IN6QE6XODCGIIWZQDZ5LNSSS4FNEO6SNHZSSASW4BBBKSZ6KWTKWY";
        let create_long: Cmd = clap::Parser::try_parse_from([
            "par",
            "create",
            "--arch",
            "x86_64-testrunner",
            "--binary",
            "./testrunner.so",
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
            "--disable-keygen",
            "--compress",
            "--wit-directory",
            "./wit",
        ])
        .unwrap();
        match create_long.par {
            ParCliCommand::Create(CreateCommand {
                vendor,
                revision,
                version,
                schema,
                directory,
                issuer,
                subject,
                name,
                arch,
                binary,
                destination,
                compress,
                disable_keygen,
                wit_dir,
            }) => {
                assert_eq!(arch, "x86_64-testrunner");
                assert_eq!(binary, "./testrunner.so");
                assert_eq!(directory.unwrap(), PathBuf::from("./tests/fixtures"));
                assert_eq!(issuer.unwrap(), ISSUER);
                assert_eq!(subject.unwrap(), SUBJECT);
                assert_eq!(name, "CreateTest");
                assert_eq!(vendor, "TestRunner");
                assert_eq!(destination.unwrap(), "./test.par.gz");
                assert_eq!(revision.unwrap(), 1);
                assert_eq!(version.unwrap(), "1.11.111");
                assert_eq!(schema, None);
                assert!(disable_keygen);
                assert!(compress);
                assert_eq!(wit_dir.unwrap(), PathBuf::from("./wit"));
            }
            cmd => panic!("par insert constructed incorrect command {cmd:?}"),
        }
        let create_short: Cmd = clap::Parser::try_parse_from([
            "par",
            "create",
            "-a",
            "x86_64-testrunner",
            "-b",
            "./testrunner.so",
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
            "--wit-directory",
            "./wit",
        ])
        .unwrap();
        match create_short.par {
            ParCliCommand::Create(CreateCommand {
                vendor,
                revision,
                version,
                schema,
                directory,
                issuer,
                subject,
                name,
                arch,
                binary,
                destination,
                compress,
                disable_keygen,
                wit_dir,
            }) => {
                assert_eq!(arch, "x86_64-testrunner");
                assert_eq!(binary, "./testrunner.so");
                assert_eq!(directory.unwrap(), PathBuf::from("./tests/fixtures"));
                assert_eq!(issuer.unwrap(), ISSUER);
                assert_eq!(subject.unwrap(), SUBJECT);
                assert_eq!(name, "CreateTest");
                assert_eq!(vendor, "TestRunner");
                assert_eq!(destination.unwrap(), "./test.par.gz");
                assert_eq!(revision.unwrap(), 1);
                assert_eq!(version.unwrap(), "1.11.111");
                assert_eq!(schema, None);
                assert!(!disable_keygen);
                assert!(!compress);
                assert_eq!(wit_dir.unwrap(), PathBuf::from("./wit"));
            }
            cmd => panic!("par insert constructed incorrect command {cmd:?}"),
        }
    }

    // Uses all flags and options of the `par insert` command
    // to ensure API does not change between versions
    #[test]
    fn test_par_insert_comprehensive() {
        const ISSUER: &str = "SAAJLQZDZO57THPTQLEELEY7FJYOJZQWQD7FF4J67TUYTSCOXTF7R4Y3VY";
        const SUBJECT: &str = "SVAH7IN6QE6XODCGQAWZQDZ5LNSSS4FNEO6SNHZSSASW4BBBKSZ6KWTKWY";
        let insert_short: Cmd = clap::Parser::try_parse_from([
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
            "--disable-keygen",
        ])
        .unwrap();
        match insert_short.par {
            ParCliCommand::Insert(InsertCommand {
                archive,
                arch,
                binary,
                directory,
                issuer,
                subject,
                disable_keygen,
            }) => {
                assert_eq!(archive, "libtest.par.gz");
                assert_eq!(arch, "x86_64-testrunner");
                assert_eq!(binary, "./testrunner.so");
                assert_eq!(directory.unwrap(), PathBuf::from("./tests/fixtures"));
                assert_eq!(issuer.unwrap(), ISSUER);
                assert_eq!(subject.unwrap(), SUBJECT);
                assert!(disable_keygen);
            }
            cmd => panic!("par insert constructed incorrect command {cmd:?}"),
        }
        let insert_long: Cmd = clap::Parser::try_parse_from([
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
        ])
        .unwrap();
        match insert_long.par {
            ParCliCommand::Insert(InsertCommand {
                archive,
                arch,
                binary,
                directory,
                issuer,
                subject,
                disable_keygen,
            }) => {
                assert_eq!(archive, "libtest.par.gz");
                assert_eq!(arch, "x86_64-testrunner");
                assert_eq!(binary, "./testrunner.so");
                assert_eq!(directory.unwrap(), PathBuf::from("./tests/fixtures"));
                assert_eq!(issuer.unwrap(), ISSUER);
                assert_eq!(subject.unwrap(), SUBJECT);
                assert!(!disable_keygen);
            }
            cmd => panic!("par insert constructed incorrect command {cmd:?}"),
        }
    }

    // Uses all flags and options of the `par inspect` command
    // to ensure API does not change between versions
    #[test]
    fn test_par_inspect_comprehensive() {
        const LOCAL: &str = "./coolthing.par.gz";
        const REMOTE: &str = "wasmcloud.azurecr.io/coolthing.par.gz";

        let inspect_long: Cmd = clap::Parser::try_parse_from([
            "par",
            "inspect",
            LOCAL,
            "--digest",
            "sha256:blah",
            "--password",
            "secret",
            "--user",
            "name",
            "--no-cache",
        ])
        .unwrap();
        match inspect_long.par {
            ParCliCommand::Inspect(InspectCommand {
                archive,
                digest,
                allow_latest,
                user,
                password,
                insecure,
                insecure_skip_tls_verify,
                no_cache,
            }) => {
                assert_eq!(archive, LOCAL);
                assert_eq!(digest.unwrap(), "sha256:blah");
                assert!(!allow_latest);
                assert!(!insecure);
                assert!(!insecure_skip_tls_verify);
                assert_eq!(user.unwrap(), "name");
                assert_eq!(password.unwrap(), "secret");
                assert!(no_cache);
            }
            cmd => panic!("par inspect constructed incorrect command {cmd:?}"),
        }
        let inspect_short: Cmd = clap::Parser::try_parse_from([
            "par",
            "inspect",
            REMOTE,
            "-d",
            "sha256:blah",
            "-p",
            "secret",
            "-u",
            "name",
            "--allow-latest",
            "--insecure",
            "--no-cache",
        ])
        .unwrap();
        match inspect_short.par {
            ParCliCommand::Inspect(InspectCommand {
                archive,
                digest,
                allow_latest,
                user,
                password,
                insecure,
                insecure_skip_tls_verify,
                no_cache,
            }) => {
                assert_eq!(archive, REMOTE);
                assert_eq!(digest.unwrap(), "sha256:blah");
                assert!(allow_latest);
                assert!(insecure);
                assert!(!insecure_skip_tls_verify);
                assert_eq!(user.unwrap(), "name");
                assert_eq!(password.unwrap(), "secret");
                assert!(no_cache);
            }
            cmd => panic!("par inspect constructed incorrect command {cmd:?}"),
        }
    }
}

use super::{cached_oci_file, CommandOutput, OutputKind};
use crate::lib::registry::{get_oci_artifact, OciPullOptions};
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use provider_archive::ProviderArchive;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::{collections::HashMap, fs::File, io::Read, path::PathBuf};
use term_table::{
    row::Row,
    table_cell::{Alignment, TableCell},
    Table,
};
use wascap::jwt::{Claims, Component, Token, TokenValidation, WascapEntity};

#[derive(Debug, Parser, Clone)]
pub struct InspectCliCommand {
    /// Path or OCI URL to signed component or provider archive
    pub target: String,

    /// Extract the raw JWT from the file and print to stdout
    #[clap(name = "jwt_only", long = "jwt-only", conflicts_with = "wit")]
    pub jwt_only: bool,

    /// Extract the WIT world from a component and print to stdout instead of the claims.
    /// When inspecting a provider archive, this flag will be ignored.
    #[clap(
        name = "wit",
        long = "wit",
        alias = "world",
        conflicts_with = "jwt_only"
    )]
    pub wit: bool,

    /// Digest to verify artifact against (if OCI URL is provided for `<target>`)
    #[clap(short = 'd', long = "digest")]
    pub digest: Option<String>,

    /// Allow latest artifact tags (if OCI URL is provided for `<target>`)
    #[clap(long = "allow-latest")]
    pub allow_latest: bool,

    /// OCI username, if omitted anonymous authentication will be used
    #[clap(
        short = 'u',
        long = "user",
        env = "WASH_REG_USER",
        hide_env_values = true
    )]
    pub user: Option<String>,

    /// OCI password, if omitted anonymous authentication will be used
    #[clap(
        short = 'p',
        long = "password",
        env = "WASH_REG_PASSWORD",
        hide_env_values = true
    )]
    pub password: Option<String>,

    /// Allow insecure (HTTP) registry connections
    #[clap(long = "insecure")]
    pub insecure: bool,

    /// Skip checking OCI registry's certificate for validity
    #[clap(long = "insecure-skip-tls-verify")]
    pub insecure_skip_tls_verify: bool,

    /// skip the local OCI cache and pull the artifact from the registry to inspect
    #[clap(long = "no-cache")]
    pub no_cache: bool,
}

/// Attempts to inspect a provider archive or component
pub async fn handle_command(
    command: impl Into<InspectCliCommand>,
    _output_kind: OutputKind,
) -> Result<CommandOutput> {
    let command = command.into();
    let mut buf = Vec::new();
    if PathBuf::from(command.target.clone()).as_path().is_dir() {
        let mut f = File::open(&command.target).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("failed to target file [{}]: {e}", &command.target),
            )
        })?;
        f.read_to_end(&mut buf)?;
    } else {
        let cache_file = (!command.no_cache).then(|| cached_oci_file(&command.target.clone()));
        buf = get_oci_artifact(
            command.target.clone(),
            cache_file,
            OciPullOptions {
                digest: command.digest.clone(),
                allow_latest: command.allow_latest,
                user: command.user.clone(),
                password: command.password.clone(),
                insecure: command.insecure,
                insecure_skip_tls_verify: command.insecure_skip_tls_verify,
            },
        )
        .await?;
    }

    let wit_parsed = wasmparser::Parser::new(0).parse_all(&buf).next();

    let output = match wit_parsed {
        // Inspect the WIT of a Wasm component
        Some(Ok(wasmparser::Payload::Version {
            encoding: wasmparser::Encoding::Component,
            ..
        })) if command.wit => {
            let witty = wit_component::decode(&buf).expect("Failed to decode WIT");
            let resolve = witty.resolve();
            let main = witty.package();
            let mut printer = wit_component::WitPrinter::default();
            printer
                .print(resolve, main, &[])
                .context("should be able to print WIT world from a component")?;
            CommandOutput::from_key_and_text("wit", printer.output.to_string())
        }
        // Catch trying to inspect a WIT from a WASI Preview 1 module
        Some(Ok(wasmparser::Payload::Version {
            encoding: wasmparser::Encoding::Module,
            ..
        })) if command.wit => {
            bail!("No WIT present in Wasm, this looks like a WASI Preview 1 module")
        }
        // Inspect claims inside of Wasm
        Some(Ok(_)) => {
            let module_name = command.target.clone();
            let jwt_only = command.jwt_only;
            let caps = get_caps(command.clone(), &buf).await?;
            let token =
                caps.with_context(|| format!("No capabilities discovered in : {module_name}"))?;

            if jwt_only {
                CommandOutput::from_key_and_text("token", token.jwt)
            } else {
                let validation = wascap::jwt::validate_token::<Component>(&token.jwt)?;
                let is_component = matches!(
                    wit_parsed,
                    Some(Ok(wasmparser::Payload::Version {
                        encoding: wasmparser::Encoding::Component,
                        ..
                    }))
                );
                render_component_claims(token.claims, validation, is_component)
            }
        }
        //  Fallback to inspecting a provider archive
        _ => {
            let artifact = ProviderArchive::try_load(&buf)
                .await
                .map_err(|e| anyhow!("{}", e))?;
            if command.wit {
                let wit_bytes = artifact
                    .wit_world()
                    .ok_or_else(|| anyhow!("No wit encoded in PAR"))?;
                let witty = wit_component::decode(wit_bytes).context("Failed to decode WIT")?;
                let resolve = witty.resolve();
                let main = witty.package();
                let mut printer = wit_component::WitPrinter::default();
                printer
                    .print(resolve, main, &[])
                    .context("should be able to print WIT world from provider archive")?;
                CommandOutput::from_key_and_text("wit", printer.output.to_string())
            } else {
                render_provider_claims(command.clone(), &artifact).await?
            }
        }
    };
    Ok(output)
}

/// Extracts claims for a given OCI artifact
async fn get_caps(
    cmd: InspectCliCommand,
    artifact_bytes: &[u8],
) -> Result<Option<Token<Component>>> {
    let _cache_path = (!cmd.no_cache).then(|| cached_oci_file(&cmd.target));
    // Extract will return an error if it encounters an invalid hash in the claims
    Ok(wascap::wasm::extract_claims(artifact_bytes)?)
}

/// Renders component claims into provided output format
#[must_use]
pub fn render_component_claims(
    claims: Claims<Component>,
    validation: TokenValidation,
    is_component: bool,
) -> CommandOutput {
    let md = claims.metadata.clone().unwrap();
    let name = md.name();
    let friendly_rev = md.rev.unwrap_or(0);
    let friendly_ver = md.ver.unwrap_or_else(|| "None".to_string());
    let friendly = format!("{friendly_ver} ({friendly_rev})");

    let tags = if let Some(tags) = &claims.metadata.as_ref().unwrap().tags {
        if tags.is_empty() {
            "None".to_string()
        } else {
            tags.join(",")
        }
    } else {
        "None".to_string()
    };

    let iss_label = token_label(&claims.issuer).to_ascii_lowercase();
    let sub_label = token_label(&claims.subject).to_ascii_lowercase();

    let mut map = HashMap::new();
    map.insert(iss_label, json!(claims.issuer));
    map.insert(sub_label, json!(claims.subject));
    map.insert("expires".to_string(), json!(validation.expires_human));
    map.insert(
        "can_be_used".to_string(),
        json!(validation.not_before_human),
    );
    map.insert("version".to_string(), json!(friendly_ver));
    map.insert("revision".to_string(), json!(friendly_rev));
    map.insert("tags".to_string(), json!(tags));
    map.insert("name".to_string(), json!(name));

    let mut table = render_core(&claims, validation);

    table.add_row(Row::new(vec![
        TableCell::new("Version"),
        TableCell::new_with_alignment(friendly, 1, Alignment::Right),
    ]));

    table.add_row(Row::new(vec![
        TableCell::new("Embedded WIT"),
        TableCell::new_with_alignment(is_component, 1, Alignment::Right),
    ]));

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

    CommandOutput::new(table.render(), map)
}

// * - we don't need render impls for Operator or Account because those tokens are never embedded into a module,
// only components.
fn token_label(pk: &str) -> String {
    match pk.chars().next().unwrap() {
        'A' => "Account".to_string(),
        'M' => "Component".to_string(),
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
    super::configure_table_style(&mut table);

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

/// Inspects a provider archive
pub(crate) async fn render_provider_claims(
    cmd: InspectCliCommand,
    artifact: &ProviderArchive,
) -> Result<CommandOutput> {
    let _cache_file = (!cmd.no_cache).then(|| cached_oci_file(&cmd.target));
    let claims = artifact
        .claims()
        .ok_or_else(|| anyhow!("No claims found in artifact"))?;
    let metadata = claims
        .metadata
        .ok_or_else(|| anyhow!("No metadata found"))?;

    let friendly_rev = metadata
        .rev
        .map_or("None".to_string(), |rev| rev.to_string());
    let friendly_ver = metadata.ver.unwrap_or_else(|| "None".to_string());
    let name = metadata.name.unwrap_or_else(|| "None".to_string());
    let mut map = HashMap::new();
    map.insert("name".to_string(), json!(name));
    map.insert("issuer".to_string(), json!(claims.issuer));
    map.insert("service".to_string(), json!(claims.subject));
    map.insert("vendor".to_string(), json!(metadata.vendor));
    map.insert("version".to_string(), json!(friendly_ver));
    map.insert("revision".to_string(), json!(friendly_rev));
    map.insert("targets".to_string(), json!(artifact.targets()));
    if let Some(schema) = artifact.schema() {
        map.insert("schema".to_string(), json!(schema));
    }

    let text_table = {
        let mut table = Table::new();
        super::configure_table_style(&mut table);

        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            format!("{name} - Capability Provider"),
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
            TableCell::new("Vendor"),
            TableCell::new_with_alignment(metadata.vendor, 1, Alignment::Right),
        ]));

        table.add_row(Row::new(vec![
            TableCell::new("Version"),
            TableCell::new_with_alignment(friendly_ver, 1, Alignment::Right),
        ]));

        table.add_row(Row::new(vec![
            TableCell::new("Revision"),
            TableCell::new_with_alignment(friendly_rev, 1, Alignment::Right),
        ]));

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

        if artifact.schema().is_some() {
            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                "\nLink Definition Schema",
                2,
                Alignment::Center,
            )]));

            table.add_row(Row::new(vec![TableCell::new_with_alignment(
                "\nUse the JSON output option to extract the schema",
                2,
                Alignment::Left,
            )]));
        }

        table.render()
    };

    Ok(CommandOutput::new(text_table, map))
}

#[cfg(test)]
mod test {
    use super::*;
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct Cmd {
        #[clap(flatten)]
        command: InspectCliCommand,
    }

    #[test]
    /// Check all flags and options of the 'inspect' command
    /// so that the API does not change in between versions
    fn test_inspect_comprehensive() {
        const LOCAL: &str = "./coolthing.par.gz";
        const REMOTE: &str = "wasmcloud.azurecr.io/coolthing.par.gz";
        const SUBSCRIBER_OCI: &str = "wasmcloud.azurecr.io/subscriber:0.2.0";

        let inspect_long: Cmd = Parser::try_parse_from([
            "inspect",
            LOCAL,
            "--digest",
            "sha256:blah",
            "--password",
            "secret",
            "--user",
            "name",
            "--jwt-only",
            "--no-cache",
        ])
        .unwrap();
        let InspectCliCommand {
            target,
            jwt_only,
            digest,
            allow_latest,
            user,
            password,
            insecure,
            insecure_skip_tls_verify,
            no_cache,
            wit,
        } = inspect_long.command;
        assert_eq!(target, LOCAL);
        assert_eq!(digest.unwrap(), "sha256:blah");
        assert!(!allow_latest);
        assert!(!insecure);
        assert!(!insecure_skip_tls_verify);
        assert_eq!(user.unwrap(), "name");
        assert_eq!(password.unwrap(), "secret");
        assert!(jwt_only);
        assert!(no_cache);
        assert!(!wit);

        let inspect_short: Cmd = Parser::try_parse_from([
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
            "--jwt-only",
            "--no-cache",
        ])
        .unwrap();
        let InspectCliCommand {
            target,
            jwt_only,
            digest,
            allow_latest,
            user,
            password,
            insecure,
            insecure_skip_tls_verify,
            no_cache,
            wit,
        } = inspect_short.command;
        assert_eq!(target, REMOTE);
        assert_eq!(digest.unwrap(), "sha256:blah");
        assert!(allow_latest);
        assert!(insecure);
        assert!(!insecure_skip_tls_verify);
        assert_eq!(user.unwrap(), "name");
        assert_eq!(password.unwrap(), "secret");
        assert!(jwt_only);
        assert!(no_cache);
        assert!(!wit);

        let cmd: Cmd = Parser::try_parse_from([
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

        let InspectCliCommand {
            target,
            jwt_only,
            digest,
            allow_latest,
            user,
            password,
            insecure,
            insecure_skip_tls_verify,
            no_cache,
            wit,
        } = cmd.command;
        assert_eq!(target, SUBSCRIBER_OCI);
        assert_eq!(
            digest.unwrap(),
            "sha256:5790f650cff526fcbc1271107a05111a6647002098b74a9a5e2e26e3c0a116b8"
        );
        assert_eq!(user.unwrap(), "name");
        assert_eq!(password.unwrap(), "opensesame");
        assert!(allow_latest);
        assert!(insecure);
        assert!(!insecure_skip_tls_verify);
        assert!(jwt_only);
        assert!(no_cache);
        assert!(!wit);

        let short_cmd: Cmd = Parser::try_parse_from([
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
            "--insecure-skip-tls-verify",
            "--wit",
            "--no-cache",
        ])
        .unwrap();

        let InspectCliCommand {
            target,
            jwt_only,
            digest,
            allow_latest,
            user,
            password,
            insecure,
            insecure_skip_tls_verify,
            no_cache,
            wit,
        } = short_cmd.command;
        assert_eq!(target, SUBSCRIBER_OCI);
        assert_eq!(
            digest.unwrap(),
            "sha256:5790f650cff526fcbc1271107a05111a6647002098b74a9a5e2e26e3c0a116b8"
        );
        assert_eq!(user.unwrap(), "name");
        assert_eq!(password.unwrap(), "opensesame");
        assert!(allow_latest);
        assert!(insecure);
        assert!(insecure_skip_tls_verify);
        assert!(!jwt_only);
        assert!(no_cache);
        assert!(wit);
    }
}

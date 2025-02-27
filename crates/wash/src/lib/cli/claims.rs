use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use nkeys::{KeyPair, KeyPairType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};
use tracing::warn;
use wascap::{
    jwt::{Account, CapabilityProvider, Claims, Component, Operator},
    wasm::{days_from_now_to_jwt_time, sign_buffer_with_claims},
};

use super::{extract_keypair, get::GetClaimsCommand, CommandOutput, OutputKind};
use crate::lib::{
    cli::inspect,
    common::boxed_err_to_anyhow,
    config::WashConnectionOptions,
    parser::{load_config, ComponentConfig, ProjectConfig, ProviderConfig, TypeConfig},
};

#[derive(Debug, Clone, Subcommand)]
pub enum ClaimsCliCommand {
    /// Examine the signing claims information or WIT world from a signed component component
    #[clap(name = "inspect")]
    Inspect(InspectCommand),
    /// Sign a WebAssembly component, specifying capabilities and other claims
    /// including expiration, tags, and additional metadata
    #[clap(name = "sign")]
    Sign(SignCommand),
    /// Generate a signed JWT by supplying basic token information, a signing seed key, and metadata
    #[clap(name = "token", subcommand)]
    Token(TokenCommand),
}

#[derive(Args, Debug, Clone)]
pub struct InspectCommand {
    /// Path to signed component or OCI URL of signed component
    pub component: String,

    /// Extract the raw JWT from the file and print to stdout
    #[clap(name = "jwt_only", long = "jwt-only")]
    pub(crate) jwt_only: bool,

    /// Extract the WIT world from a component and print to stdout instead of the claims
    #[clap(name = "wit", long = "wit", alias = "world")]
    pub wit: bool,

    /// Digest to verify artifact against (if OCI URL is provided for `<component>`)
    #[clap(short = 'd', long = "digest")]
    pub(crate) digest: Option<String>,

    /// Allow latest artifact tags (if OCI URL is provided for `<component>`)
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

    /// Skip checking OCI registry's certificate for validity
    #[clap(long = "insecure-skip-tls-verify")]
    pub insecure_skip_tls_verify: bool,

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
    pub metadata: ComponentMetadata,
}

#[derive(Debug, Clone, Subcommand)]
pub enum TokenCommand {
    /// Generate a signed JWT for an component module
    #[clap(name = "component")]
    Component(ComponentMetadata),
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
    /// Location of key files for signing. Defaults to $`WASH_KEYS` ($HOME/.wash/keys)
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

    /// Path to issuer seed key (self signing operator). If this flag is not provided, the will be sourced from $`WASH_KEYS` ($HOME/.wash/keys) or generated for you if it cannot be found.
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

    /// Path to issuer seed key (operator). If this flag is not provided, the will be sourced from $`WASH_KEYS` ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 'i',
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    issuer: Option<String>,

    /// Path to subject seed key (account). If this flag is not provided, the will be sourced from $`WASH_KEYS` ($HOME/.wash/keys) or generated for you if it cannot be found.
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

    /// A human-readable string identifying the vendor of this provider (e.g. Redis or Cassandra or NATS etc)
    #[clap(short = 'v', long = "vendor")]
    vendor: Option<String>,

    /// Monotonically increasing revision number
    #[clap(short = 'r', long = "revision")]
    revision: Option<i32>,

    /// Human-friendly version string
    #[clap(short = 'e', long = "version")]
    version: Option<String>,

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

    #[clap(flatten)]
    common: GenerateCommon,
}

impl ProviderMetadata {
    #[must_use]
    pub fn update_with_project_config(self, project_config: &ProjectConfig) -> Self {
        let provider_config = match project_config.project_type {
            TypeConfig::Provider(ref provider_config) => provider_config.clone(),
            _ => ProviderConfig::default(),
        };

        Self {
            name: self.name.or(Some(project_config.common.name.clone())),
            revision: self.revision.or(Some(project_config.common.revision)),
            version: self
                .version
                .or(Some(project_config.common.version.to_string())),
            vendor: self.vendor.or(Some(provider_config.vendor)),
            ..self
        }
    }
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ComponentMetadata {
    /// A human-readable, descriptive name for the token
    #[clap(short = 'n', long = "name")]
    pub name: Option<String>,
    /// A list of arbitrary tags to be embedded in the token
    #[clap(short = 't', long = "tag")]
    pub tags: Vec<String>,
    /// Revision number
    #[clap(short = 'r', long = "rev")]
    pub rev: Option<i32>,
    /// Human-readable version string
    #[clap(short = 'v', long = "ver")]
    pub ver: Option<String>,
    /// Developer or human friendly unique alias used for invoking an component, consisting of lowercase alphanumeric characters, underscores '_' and slashes '/'
    #[clap(short = 'a', long = "call-alias")]
    pub call_alias: Option<String>,

    /// Path to issuer seed key (account). If this flag is not provided, the will be sourced from $`WASH_KEYS` ($HOME/.wash/keys) or generated for you if it cannot be found.
    #[clap(
        short = 'i',
        long = "issuer",
        env = "WASH_ISSUER_KEY",
        hide_env_values = true
    )]
    pub issuer: Option<String>,

    /// Path to subject seed key (module). If this flag is not provided, the will be sourced from $`WASH_KEYS` ($HOME/.wash/keys) or generated for you if it cannot be found.
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

impl ComponentMetadata {
    #[must_use]
    pub fn update_with_project_config(self, project_config: &ProjectConfig) -> Self {
        let component_config = match project_config.project_type {
            TypeConfig::Component(ref component_config) => component_config.clone(),
            _ => ComponentConfig::default(),
        };

        Self {
            name: self.name.or(Some(project_config.common.name.clone())),
            rev: self.rev.or(Some(project_config.common.revision)),
            ver: self.ver.or(Some(project_config.common.version.to_string())),
            tags: match component_config.tags.clone() {
                Some(tags) => tags
                    .into_iter()
                    .collect::<BTreeSet<String>>()
                    .union(&self.tags.clone().into_iter().collect::<BTreeSet<String>>())
                    .cloned()
                    .collect::<Vec<String>>(),
                None => self.tags,
            },
            call_alias: self.call_alias,
            common: GenerateCommon {
                directory: self
                    .common
                    .directory
                    .or(Some(component_config.key_directory)),
                ..self.common
            },
            ..self
        }
    }
}

impl From<InspectCommand> for inspect::InspectCliCommand {
    fn from(cmd: InspectCommand) -> Self {
        Self {
            target: cmd.component,
            jwt_only: cmd.jwt_only,
            wit: cmd.wit,
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

pub async fn handle_command(
    command: ClaimsCliCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let project_config = load_config(None, Some(true)).await.ok();
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
                        TypeConfig::Component(ref component_config) => {
                            signcmd
                                .destination
                                .or(component_config.destination.clone().map(|d| {
                                    d.to_str()
                                        .expect("unable to convert destination pathbuf to str")
                                        .to_string()
                                }))
                        }
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
        (TokenCommand::Component(component), Some(config)) => {
            generate_component(component.update_with_project_config(config), output_kind)
        }
        (TokenCommand::Provider(provider), Some(config)) => {
            generate_provider(provider.update_with_project_config(config), output_kind)
        }
        (TokenCommand::Component(component), _) => generate_component(component, output_kind),
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
                Some(k),
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

fn generate_component(
    component: ComponentMetadata,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let issuer = extract_keypair(
        component.issuer.as_deref(),
        component.name.as_deref(),
        component.common.directory.clone(),
        KeyPairType::Account,
        component.common.disable_keygen,
        output_kind,
    )?;
    let subject = extract_keypair(
        component.subject.as_deref(),
        component.name.as_deref(),
        component.common.directory.clone(),
        KeyPairType::Module,
        component.common.disable_keygen,
        output_kind,
    )?;

    let claims = Claims::<Component>::with_dates(
        component.name.context("component name is required")?,
        issuer.public_key(),
        subject.public_key(),
        Some(component.tags.clone()),
        days_from_now_to_jwt_time(component.common.not_before_days),
        days_from_now_to_jwt_time(component.common.expires_in_days),
        false,
        Some(
            component
                .rev
                .context("component revision number is required")?,
        ),
        Some(component.ver.context("component version is required")?),
        sanitize_alias(component.call_alias)?,
    );

    let jwt = claims.encode(&issuer)?;

    Ok(CommandOutput::from_key_and_text("token", jwt))
}

fn generate_operator(operator: OperatorMetadata, output_kind: OutputKind) -> Result<CommandOutput> {
    let self_sign_key = extract_keypair(
        operator.issuer.as_deref(),
        Some(&operator.name),
        operator.common.directory.clone(),
        KeyPairType::Operator,
        operator.common.disable_keygen,
        output_kind,
    )?;

    let additional_keys = match operator.additional_signing_keys {
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
            additional_keys
                .iter()
                .map(nkeys::KeyPair::public_key)
                .collect()
        } else {
            vec![]
        },
    );

    let jwt = claims.encode(&self_sign_key)?;

    Ok(CommandOutput::from_key_and_text("token", jwt))
}

fn generate_account(account: AccountMetadata, output_kind: OutputKind) -> Result<CommandOutput> {
    let issuer = extract_keypair(
        account.issuer.as_deref(),
        Some(&account.name),
        account.common.directory.clone(),
        KeyPairType::Operator,
        account.common.disable_keygen,
        output_kind,
    )?;
    let subject = extract_keypair(
        account.subject.as_deref(),
        Some(&account.name),
        account.common.directory.clone(),
        KeyPairType::Account,
        account.common.disable_keygen,
        output_kind,
    )?;
    let additional_keys = account
        .additional_signing_keys
        .map(|keys| {
            get_keypair_vec(
                &keys,
                account.common.directory.clone(),
                KeyPairType::Account,
                true,
                output_kind,
            )
        })
        .unwrap_or_default();

    let claims: Claims<Account> = Claims::<Account>::with_dates(
        account.name.clone(),
        issuer.public_key(),
        subject.public_key(),
        days_from_now_to_jwt_time(account.common.not_before_days),
        days_from_now_to_jwt_time(account.common.expires_in_days),
        if !additional_keys.is_empty() {
            additional_keys
                .iter()
                .map(nkeys::KeyPair::public_key)
                .collect()
        } else {
            vec![]
        },
    );
    let jwt = claims.encode(&issuer)?;
    Ok(CommandOutput::from_key_and_text("token", jwt))
}

fn generate_provider(provider: ProviderMetadata, output_kind: OutputKind) -> Result<CommandOutput> {
    let issuer = extract_keypair(
        provider.issuer.as_deref(),
        provider.name.as_deref(),
        provider.common.directory.clone(),
        KeyPairType::Account,
        provider.common.disable_keygen,
        output_kind,
    )?;
    let subject = extract_keypair(
        provider.subject.as_deref(),
        provider.name.as_deref(),
        provider.common.directory.clone(),
        KeyPairType::Service,
        provider.common.disable_keygen,
        output_kind,
    )?;

    let claims: Claims<CapabilityProvider> = Claims::<CapabilityProvider>::with_dates(
        provider.name.context("provider name is required")?,
        issuer.public_key(),
        subject.public_key(),
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
        cmd.metadata.issuer.as_deref(),
        Some(&cmd.source),
        cmd.metadata.common.directory.clone(),
        KeyPairType::Account,
        cmd.metadata.common.disable_keygen,
        output_kind,
    )?;
    let subject = extract_keypair(
        cmd.metadata.subject.as_deref(),
        Some(&cmd.source),
        cmd.metadata.common.directory.clone(),
        KeyPairType::Module,
        cmd.metadata.common.disable_keygen,
        output_kind,
    )?;

    let signed = sign_buffer_with_claims(
        cmd.metadata.name.context("component name is required")?,
        &buf,
        &subject,
        &issuer,
        cmd.metadata.common.expires_in_days,
        cmd.metadata.common.not_before_days,
        cmd.metadata.tags.clone(),
        false,
        Some(
            cmd.metadata
                .rev
                .context("component revision number is required")?,
        ),
        Some(cmd.metadata.ver.context("component version is required")?),
        sanitize_alias(cmd.metadata.call_alias)?,
    )?;

    let destination = cmd.destination.unwrap_or_else(|| {
        let source = Path::new(&cmd.source);

        let path = source.parent().unwrap().to_str().unwrap();
        let module_name = source.file_stem().unwrap().to_str().unwrap();
        // If path is empty, user supplied module in current directory
        if path.is_empty() {
            format!("./{module_name}_s.wasm")
        } else {
            format!("{path}/{module_name}_s.wasm")
        }
    });

    let destination_path = Path::new(&destination);
    if let Some(p) = destination_path.parent() {
        fs::create_dir_all(p)?;
    }
    fs::write(destination_path, signed)?;
    let mut map = HashMap::new();
    map.insert("destination".to_string(), json!(destination));
    Ok(CommandOutput::new(
        format!("Successfully signed {destination}"),
        map,
    ))
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

/// Retrieve claims from a given wasmCloud instance
pub async fn get_claims(
    GetClaimsCommand { opts }: GetClaimsCommand,
) -> Result<Vec<HashMap<String, String>>> {
    let wco: WashConnectionOptions = opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;
    client
        .get_claims()
        .await
        .map_err(boxed_err_to_anyhow)
        .map(|c| c.into_data().unwrap_or_default())
        .with_context(|| {
            format!("Was able to connect to NATS, but failed to get claims: {client:?}")
        })
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use super::*;
    use crate::lib::parser::{
        CommonConfig, ComponentConfig, LanguageConfig, RegistryConfig, RustConfig, TypeConfig,
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
            Err(e) => assert_eq!(format!("{e}"), invalid_message),
            _ => panic!("invalid symbols in call alias should not be accepted"),
        };

        let invalid_uppercase = sanitize_alias(Some(INVALID_CAPITAL.to_string()));
        match invalid_uppercase {
            Err(e) => assert_eq!(format!("{e}"), invalid_message),
            _ => panic!("uppercase symbols in call alias should not be accepted"),
        };

        let pkey_message = "Public key cannot be used as a call alias";
        let invalid_pkey = sanitize_alias(Some(INVALID_PKEY.to_string()));
        match invalid_pkey {
            Err(e) => assert_eq!(format!("{e}"), pkey_message),
            _ => panic!("public keys cannot be a call alias"),
        };

        let empty_message = "Call alias cannot be empty";
        let invalid_empty = sanitize_alias(Some(String::new()));
        match invalid_empty {
            Err(e) => assert_eq!(format!("{e}"), empty_message),
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
                component: module,
                jwt_only,
                digest,
                allow_latest,
                user,
                password,
                insecure,
                insecure_skip_tls_verify,
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
                assert!(!insecure_skip_tls_verify);
                assert!(jwt_only);
                assert!(no_cache);
                assert!(!wit);
            }
            cmd => panic!("claims constructed incorrect command: {cmd:?}"),
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
            "--insecure-skip-tls-verify",
            "--jwt-only",
            "--no-cache",
        ])
        .unwrap();

        match short_cmd.claims {
            ClaimsCliCommand::Inspect(InspectCommand {
                component: module,
                jwt_only,
                digest,
                allow_latest,
                user,
                password,
                insecure,
                insecure_skip_tls_verify,
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
                assert!(insecure_skip_tls_verify);
                assert!(jwt_only);
                assert!(no_cache);
                assert!(!wit);
            }
            cmd => panic!("claims constructed incorrect command: {cmd:?}"),
        }
    }

    #[test]
    /// Enumerates all options and flags of the `claims sign` command
    /// to ensure command line arguments do not change between versions
    fn test_claims_sign_comprehensive() {
        const LOCAL_WASM: &str = "./mycomponent.wasm";
        const ISSUER_KEY: &str = "SAAOBYD6BLELXSNN4S3TXUM7STGPB3A5HYU3D5T7XA4WHGVQBDBD4LJPOM";
        const SUBJECT_KEY: &str = "SMAMA4ABHIJUYQR54BDFHEMXIIGQATUXK6RYU6XLTFHDNCRVWT4KSDDSVE";
        let long_cmd: Cmd = Parser::try_parse_from([
            "claims",
            "sign",
            LOCAL_WASM,
            "--name",
            "MyComponent",
            "--destination",
            "./mycomponent_s.wasm",
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
                assert_eq!(destination.unwrap(), "./mycomponent_s.wasm");
                assert_eq!(metadata.common.directory.unwrap(), PathBuf::from("./dir"));
                assert_eq!(metadata.common.expires_in_days.unwrap(), 3);
                assert_eq!(metadata.common.not_before_days.unwrap(), 1);
                assert!(metadata.common.disable_keygen);
                assert_eq!(metadata.name.unwrap(), "MyComponent");
                assert!(!metadata.tags.is_empty());
                assert_eq!(metadata.tags[0], "testtag");
                assert_eq!(metadata.rev.unwrap(), 2);
                assert_eq!(metadata.ver.unwrap(), "0.0.1");
            }
            cmd => panic!("claims constructed incorrect command: {cmd:?}"),
        }
        let short_cmd: Cmd = Parser::try_parse_from([
            "claims",
            "sign",
            LOCAL_WASM,
            "-n",
            "MyComponent",
            "-d",
            "./mycomponent_s.wasm",
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
                assert_eq!(destination.unwrap(), "./mycomponent_s.wasm");
                assert_eq!(metadata.common.directory.unwrap(), PathBuf::from("./dir"));
                assert_eq!(metadata.common.expires_in_days.unwrap(), 3);
                assert_eq!(metadata.common.not_before_days.unwrap(), 1);
                assert!(metadata.common.disable_keygen);
                assert_eq!(metadata.name.unwrap(), "MyComponent");
                assert!(!metadata.tags.is_empty());
                assert_eq!(metadata.tags[0], "testtag");
                assert_eq!(metadata.rev.unwrap(), 2);
                assert_eq!(metadata.ver.unwrap(), "0.0.1");
            }
            cmd => panic!("claims constructed incorrect command: {cmd:?}"),
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
        const COMPONENT_KEY: &str = "SMAA2XB7UP7FZLPLO27NJB65PKYISNQAH7PZ6PJUHR6CUARVANXZ4OTZOU";
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
            cmd => panic!("claims constructed incorrect command: {cmd:?}"),
        }
        let component_cmd: Cmd = Parser::try_parse_from([
            "claims",
            "token",
            "component",
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
            COMPONENT_KEY,
            "--rev",
            "2",
            "--tag",
            "testtag",
            "--ver",
            "0.0.1",
        ])
        .unwrap();
        match component_cmd.claims {
            ClaimsCliCommand::Token(TokenCommand::Component(ComponentMetadata {
                name,
                issuer,
                subject,
                common,
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
                assert_eq!(subject.unwrap(), COMPONENT_KEY);
                assert!(!tags.is_empty());
                assert_eq!(tags[0], "testtag");
                assert_eq!(rev.unwrap(), 2);
                assert_eq!(ver.unwrap(), "0.0.1");
            }
            cmd => panic!("claims constructed incorrect command: {cmd:?}"),
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
            cmd => panic!("claims constructed incorrect command: {cmd:?}"),
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
                assert_eq!(vendor.unwrap(), "test");
                assert_eq!(revision.unwrap(), 0);
                assert_eq!(version.unwrap(), "1.2.3");
            }
            cmd => panic!("claims constructed incorrect command: {cmd:?}"),
        }
    }

    #[tokio::test]
    async fn rust_component_metadata_with_project_config_overrides() -> anyhow::Result<()> {
        let result = load_config(
            Some(PathBuf::from(
                "./tests/parser/files/rust_component_claims_metadata.toml",
            )),
            None,
        )
        .await;

        let project_config = assert_ok!(result);

        assert_eq!(
            project_config.language,
            LanguageConfig::Rust(RustConfig {
                cargo_path: Some("./cargo".into()),
                target_path: Some("./target".into()),
                debug: false,
            })
        );

        assert_eq!(
            project_config.project_type,
            TypeConfig::Component(ComponentConfig {
                key_directory: PathBuf::from("./keys"),
                destination: Some(PathBuf::from("./build/testcomponent.wasm".to_string())),
                tags: Some(HashSet::from([
                    "wasmcloud.com/experimental".into(),
                    "test".into(),
                ])),
                ..ComponentConfig::default()
            })
        );

        assert_eq!(
            project_config.common,
            CommonConfig {
                name: "testcomponent".to_string(),
                version: Version::parse("0.1.0").unwrap(),
                revision: 666,
                project_dir: PathBuf::from("./tests/parser/files/")
                    .canonicalize()
                    .unwrap(),
                build_dir: PathBuf::from("./tests/parser/files/")
                    .canonicalize()
                    .unwrap()
                    .join("build"),
                wit_dir: PathBuf::from("./tests/parser/files/")
                    .canonicalize()
                    .unwrap()
                    .join("wit"),
                wasm_bin_name: None,
                registry: RegistryConfig::default(),
            }
        );

        //=== check project config overrides when cli args are NOT specified...
        let component_metadata =
            ComponentMetadata::default().update_with_project_config(&project_config);
        assert_eq!(
            component_metadata,
            ComponentMetadata {
                name: Some("testcomponent".to_string()),
                ver: Some(Version::parse("0.1.0")?.to_string()),
                rev: Some(666),
                call_alias: None,
                tags: vec!["test".to_string(), "wasmcloud.com/experimental".to_string()],
                common: GenerateCommon {
                    directory: Some(PathBuf::from("./keys")),
                    ..GenerateCommon::default()
                },
                ..ComponentMetadata::default()
            }
        );

        //=== check project config overrides when some cli args are specified...
        const LOCAL_WASM: &str = "./mycomponent.wasm";
        let cmd: Cmd = Parser::try_parse_from([
            "claims",
            "sign",
            LOCAL_WASM,
            "--name",
            "MyComponent",
            "--destination",
            "./mycomponent_s.wasm",
            "--directory",
            "./dir",
            "--rev",
            "777",
            "--tag",
            "test-tag",
            "--ver",
            "0.2.0",
        ])
        .unwrap();

        match cmd.claims {
            ClaimsCliCommand::Sign(signcmd) => {
                let cmd = SignCommand {
                    metadata: signcmd.metadata.update_with_project_config(&project_config),
                    destination: match &project_config.project_type {
                        TypeConfig::Component(ref component_config) => {
                            signcmd
                                .destination
                                .or(component_config.destination.clone().map(|d| {
                                    d.to_str()
                                        .expect("unable to convert destination pathbuf to str")
                                        .to_string()
                                }))
                        }
                        _ => signcmd.destination,
                    },
                    ..signcmd
                };

                assert_eq!(cmd.source, LOCAL_WASM);
                assert_eq!(cmd.destination.unwrap(), "./mycomponent_s.wasm");
                assert_eq!(
                    cmd.metadata.common.directory.unwrap(),
                    PathBuf::from("./dir")
                );
                assert_eq!(cmd.metadata.name.unwrap(), "MyComponent");
                assert_eq!(cmd.metadata.tags.len(), 3);
                assert!(cmd.metadata.tags.contains(&"test-tag".to_string()));
                assert!(cmd.metadata.tags.contains(&"test".to_string())); // from project_config
                assert!(cmd
                    .metadata
                    .tags
                    .contains(&"wasmcloud.com/experimental".to_string())); // from project_config
                assert_eq!(cmd.metadata.rev.unwrap(), 777);
                assert_eq!(cmd.metadata.ver.unwrap(), "0.2.0");
            }

            _ => unreachable!("claims constructed incorrect command"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn rust_provider_metadata_with_project_config_overrides() -> anyhow::Result<()> {
        let result = load_config(
            Some(PathBuf::from(
                "./tests/parser/files/rust_provider_claims_metadata.toml",
            )),
            None,
        )
        .await;

        let project_config = assert_ok!(result);

        let mut expected_default_key_dir = etcetera::home_dir()?;
        expected_default_key_dir.push(".wash/keys");

        assert_eq!(
            project_config.language,
            LanguageConfig::Rust(RustConfig {
                cargo_path: Some("./cargo".into()),
                target_path: Some("./target".into()),
                debug: false,
            })
        );

        assert_eq!(
            project_config.project_type,
            TypeConfig::Provider(ProviderConfig {
                vendor: "wayne-industries".into(),
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                key_directory: expected_default_key_dir,
                wit_world: Some("wasmcloud:httpserver".to_string()),
                rust_target: None,
                bin_name: None,
            })
        );

        assert_eq!(
            project_config.common,
            CommonConfig {
                name: "testprovider".to_string(),
                version: Version::parse("0.1.0").unwrap(),
                revision: 666,
                wasm_bin_name: None,
                project_dir: PathBuf::from("./tests/parser/files/")
                    .canonicalize()
                    .unwrap(),
                build_dir: PathBuf::from("./tests/parser/files/")
                    .canonicalize()
                    .unwrap()
                    .join("build"),
                wit_dir: PathBuf::from("./tests/parser/files/")
                    .canonicalize()
                    .unwrap()
                    .join("wit"),
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
                assert_eq!(metadata.vendor.unwrap(), "test");
                assert_eq!(metadata.revision.unwrap(), 777);
                assert_eq!(metadata.version.unwrap(), "0.2.0");
            }
            _ => unreachable!("claims constructed incorrect command"),
        }

        Ok(())
    }
}

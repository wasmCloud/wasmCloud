use anyhow::{anyhow, Context, Result};
use provider_archive::ProviderArchive;
use std::path::PathBuf;

/// Helper function for detecting the arch used by the current machine
pub fn detect_arch() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

pub struct ParCreateArgs {
    pub capid: String,
    pub vendor: String,
    pub revision: Option<i32>,
    pub version: Option<String>,
    pub schema: Option<PathBuf>,
    pub name: String,
    pub arch: String,
}

pub fn create_provider_archive(
    ParCreateArgs {
        capid,
        vendor,
        revision,
        version,
        schema,
        name,
        arch,
    }: ParCreateArgs,
    binary_bytes: &[u8],
) -> Result<ProviderArchive> {
    let mut par = ProviderArchive::new(&capid, &name, &vendor, revision, version);

    par.add_library(&arch, binary_bytes)
        .map_err(convert_error)?;

    if let Some(ref schema) = schema {
        let bytes = std::fs::read(schema)?;
        par.set_schema(
            serde_json::from_slice::<serde_json::Value>(&bytes)
                .with_context(|| "Unable to parse JSON from file contents".to_string())?,
        )
        .map_err(convert_error)
        .with_context(|| format!("Error parsing JSON schema from file '{:?}'", schema))?;
    }

    Ok(par)
}

pub async fn insert_provider_binary(
    arch: String,
    binary_bytes: &[u8],
    mut par: ProviderArchive,
) -> Result<ProviderArchive> {
    par.add_library(&arch, binary_bytes)
        .map_err(convert_error)?;

    Ok(par)
}

/// Converts error from Send + Sync error to standard anyhow error
pub fn convert_error(e: Box<dyn ::std::error::Error + Send + Sync>) -> anyhow::Error {
    anyhow!(e.to_string())
}

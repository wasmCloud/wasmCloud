use std::path::PathBuf;

use clap::Args;
use wash_lib::cli::{CommandOutput, CommonPackageArgs};
use wasm_pkg_client::{PublishOpts, Registry};

/// Arguments for invoking `wash wit publish`
#[derive(Args, Debug, Clone)]
pub struct PublishArgs {
    /// The file to publish
    file: PathBuf,

    /// The registry domain to use. Overrides configuration file(s).
    #[arg(long = "wit-registry", env = "WASH_WIT_REGISTRY")]
    registry: Option<Registry>,

    #[command(flatten)]
    common: CommonPackageArgs,
}

/// Invoke `wash wit publish`
pub async fn invoke(
    PublishArgs {
        file,
        registry,
        common,
    }: PublishArgs,
) -> anyhow::Result<CommandOutput> {
    let client = common.get_client().await?;

    let (package, version) = client
        .client()?
        .publish_release_file(
            &file,
            PublishOpts {
                registry,
                ..Default::default()
            },
        )
        .await?;

    Ok(CommandOutput::new(
        format!("Published {}@{}", package, version),
        [
            ("package".to_string(), package.to_string().into()),
            ("version".to_string(), version.to_string().into()),
        ]
        .into(),
    ))
}

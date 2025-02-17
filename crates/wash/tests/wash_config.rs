mod common;

use common::TestWashInstance;

use std::collections::HashMap;

use wash::cli::cmd::config::ConfigCliCommand;
use wash::lib::cli::{CliConnectionOpts, OutputKind};

#[tokio::test]
async fn test_config_put_and_get() -> anyhow::Result<()> {
    let wash_instance = TestWashInstance::create().await?;
    // Create a new config
    let config_values = vec!["key=value".to_string(), "key2=value2".to_string()];

    let command = ConfigCliCommand::PutCommand {
        opts: CliConnectionOpts {
            ctl_port: Some(wash_instance.nats_port.to_string()),
            ..Default::default()
        },
        name: "foobar".to_string(),
        config_values,
    };

    // Put the config
    wash::cli::cmd::config::handle_command(command, OutputKind::Json).await?;

    // Assert that the retrieved config deserializes as a HashMap
    let retrieved_config: HashMap<String, serde_json::Value> =
        wash::cli::cmd::config::handle_command(
            ConfigCliCommand::GetCommand {
                opts: CliConnectionOpts {
                    ctl_port: Some(wash_instance.nats_port.to_string()),
                    ..Default::default()
                },
                name: "foobar".to_string(),
            },
            OutputKind::Json,
        )
        .await?
        .map;

    assert_eq!(retrieved_config.len(), 2);
    assert_eq!(retrieved_config.get("key").unwrap(), "value");
    assert_eq!(retrieved_config.get("key2").unwrap(), "value2");

    Ok(())
}

#[tokio::test]
async fn test_config_secret_name_error() -> anyhow::Result<()> {
    // Attempt to create a config with a secret name
    let config_values = vec!["key=value".to_string()];

    let command = ConfigCliCommand::PutCommand {
        opts: CliConnectionOpts::default(),
        name: "SECRET_foo".to_string(),
        config_values,
    };

    // Put the config and expect an error
    let result = wash::cli::cmd::config::handle_command(command, OutputKind::Json).await;

    // Assert that an error occurred
    assert!(result.is_err());

    Ok(())
}

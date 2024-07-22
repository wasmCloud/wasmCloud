mod common;

use std::collections::HashMap;

use anyhow::{bail, Context as _};
use common::TestWashInstance;
use wash_cli::secrets::SecretsCliCommand;
use wash_lib::cli::{CliConnectionOpts, OutputKind};
use wasmcloud_secrets_types::{SECRET_POLICY_TYPE, SECRET_TYPE};

#[tokio::test]
async fn test_secret_put_and_get() -> anyhow::Result<()> {
    let wash_instance = TestWashInstance::create().await?;
    // Create a new secret reference
    let opts = CliConnectionOpts {
        ctl_port: Some(wash_instance.nats_port.to_string()),
        ..Default::default()
    };

    let basic_secret_command = SecretsCliCommand::PutCommand {
        opts: opts.clone(),
        name: "foobar".to_string(),
        backend: "baobun".to_string(),
        key: "path/to/secret".to_string(),
        version: None,
        policy_properties: vec![],
    };

    // Put the config
    wash_cli::secrets::handle_command(basic_secret_command, OutputKind::Json).await?;

    // Assert that the retrieved config deserializes as a HashMap
    let retrieved_secret: HashMap<String, serde_json::Value> = wash_cli::secrets::handle_command(
        SecretsCliCommand::GetCommand {
            opts,
            name: "foobar".to_string(),
        },
        OutputKind::Json,
    )
    .await?
    .map;

    assert_eq!(retrieved_secret.len(), 4);

    assert!(retrieved_secret
        .get("backend")
        .is_some_and(|b| b == "baobun"));
    assert!(retrieved_secret
        .get("key")
        .is_some_and(|k| k == "path/to/secret"));
    assert!(retrieved_secret
        .get("type")
        .is_some_and(|v| v == SECRET_TYPE));
    let serde_json::Value::String(policy) = retrieved_secret
        .get("policy_properties")
        .context("policy_properties should exist")?
    else {
        bail!("policy_properties should be a string");
    };

    let policy: HashMap<String, String> =
        serde_json::from_str(policy).context("policy_properties should be a JSON object")?;
    assert!(policy.get("type").is_some_and(|t| t == SECRET_POLICY_TYPE));
    let Some(properties) = policy.get("properties") else {
        bail!("policy_properties should have a 'properties' field");
    };
    let policy_properties: HashMap<String, String> =
        serde_json::from_str(properties).context("properties should be a JSON object")?;
    assert_eq!(policy_properties.len(), 1);
    assert!(policy_properties
        .get("backend")
        .is_some_and(|b| b == "baobun"));

    assert!(retrieved_secret.contains_key("version"));

    Ok(())
}

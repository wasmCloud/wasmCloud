mod common;

use std::collections::HashMap;

use anyhow::{bail, Context as _};
use common::TestWashInstance;
use wash::cli::secrets::SecretsCliCommand;
use wash::lib::cli::{CliConnectionOpts, OutputKind};
use wasmcloud_secrets_types::{SECRET_POLICY_PROPERTIES_TYPE, SECRET_TYPE};

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
        field: None,
        version: None,
        policy_properties: vec![],
    };

    // Put the config
    wash::cli::secrets::handle_command(basic_secret_command, OutputKind::Json).await?;

    // Assert that the retrieved config deserializes as a HashMap
    let retrieved_secret: HashMap<String, serde_json::Value> = wash::cli::secrets::handle_command(
        SecretsCliCommand::GetCommand {
            opts,
            name: "foobar".to_string(),
        },
        OutputKind::Json,
    )
    .await?
    .map;

    assert_eq!(retrieved_secret.len(), 5);
    assert!(retrieved_secret.get("name").is_some_and(|n| n == "foobar"));
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
        .get("policy")
        .context("policy should exist")?
    else {
        bail!("policy should be a string");
    };

    let serde_json::Value::Object(policy) =
        serde_json::from_str(policy).context("policy should be a JSON object")?
    else {
        bail!("policy should be a JSON object");
    };
    assert!(policy.get("type").is_some_and(
        |t| *t == serde_json::Value::String(SECRET_POLICY_PROPERTIES_TYPE.to_string())
    ));
    let Some(properties) = policy.get("properties").cloned() else {
        bail!("policy should have a 'properties' field");
    };

    let policy_properties: HashMap<String, String> =
        serde_json::from_value(properties).context("properties should be a JSON object")?;
    assert!(policy_properties.is_empty());

    assert!(!retrieved_secret.contains_key("version"));

    Ok(())
}

#[tokio::test]
async fn test_secret_put_and_get_complex() -> anyhow::Result<()> {
    let wash_instance = TestWashInstance::create().await?;
    let opts = CliConnectionOpts {
        ctl_port: Some(wash_instance.nats_port.to_string()),
        ..Default::default()
    };

    let basic_secret_command = SecretsCliCommand::PutCommand {
        opts: opts.clone(),
        name: "mysecret".to_string(),
        backend: "baobuns".to_string(),
        key: "secrets/path/v2/mine".to_string(),
        field: Some("myfield".to_string()),
        version: Some("v1.0.0".to_string()),
        policy_properties: vec!["role=operator".to_string(), "app_id=1234".to_string()],
    };

    // Put the config
    wash::cli::secrets::handle_command(basic_secret_command, OutputKind::Json).await?;

    // Assert that the retrieved config deserializes as a HashMap
    let retrieved_secret: HashMap<String, serde_json::Value> = wash::cli::secrets::handle_command(
        SecretsCliCommand::GetCommand {
            opts,
            name: "mysecret".to_string(),
        },
        OutputKind::Json,
    )
    .await?
    .map;

    assert_eq!(retrieved_secret.len(), 7);
    assert!(retrieved_secret
        .get("name")
        .is_some_and(|n| n == "mysecret"));
    assert!(retrieved_secret
        .get("backend")
        .is_some_and(|b| b == "baobuns"));
    assert!(retrieved_secret
        .get("key")
        .is_some_and(|k| k == "secrets/path/v2/mine"));
    assert!(retrieved_secret
        .get("field")
        .is_some_and(|f| f == "myfield"));
    assert!(retrieved_secret
        .get("version")
        .is_some_and(|k| k == "v1.0.0"));
    assert!(retrieved_secret
        .get("type")
        .is_some_and(|v| v == SECRET_TYPE));
    let serde_json::Value::String(policy) = retrieved_secret
        .get("policy")
        .context("policy should exist")?
    else {
        bail!("policy_properties should be a string");
    };

    let serde_json::Value::Object(policy) =
        serde_json::from_str(policy).context("policy should be a JSON object")?
    else {
        bail!("policy should be a JSON object");
    };
    assert!(policy.get("type").is_some_and(
        |t| *t == serde_json::Value::String(SECRET_POLICY_PROPERTIES_TYPE.to_string())
    ));
    let Some(properties) = policy.get("properties").cloned() else {
        bail!("policy should have a 'properties' field");
    };

    let policy_properties: HashMap<String, String> =
        serde_json::from_value(properties).context("properties should be a JSON object")?;
    assert_eq!(policy_properties.len(), 2);
    assert_eq!(policy_properties.get("role"), Some(&"operator".to_string()));
    assert_eq!(policy_properties.get("app_id"), Some(&"1234".to_string()));

    Ok(())
}

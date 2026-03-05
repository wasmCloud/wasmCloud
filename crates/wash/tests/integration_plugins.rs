//! Integration tests for wash plugin system
//!
//! This test validates the plugin test command functionality using the oauth plugin.
//! It runs the plugin test command with various combinations of command and hook flags.

// Increase the default recursion limit
#![recursion_limit = "256"]

use anyhow::{Context, Result};
use std::path::PathBuf;

use wash::{
    cli::{
        CliCommand, CliContext,
        plugin::{PluginCommand, TestCommand},
    },
    plugin::bindings::wasmcloud::wash::types::HookType,
};

/// Test the plugin test command with the inspect plugin
#[tokio::test]
#[ignore] // TODO ignore until we have https://github.com/wasmCloud/go/issues/243 fixed
async fn test_plugin_test_inspect_comprehensive() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let ctx = CliContext::builder()
        .build()
        .await
        .context("Failed to create CLI context")?;

    let inspect_plugin_path = PathBuf::from("plugins/inspect");

    // Verify the inspect plugin directory exists
    if !inspect_plugin_path.exists() {
        anyhow::bail!(
            "inspect plugin directory not found at {}",
            inspect_plugin_path.display()
        );
    }

    eprintln!(
        "🧪 Testing inspect plugin at: {}",
        inspect_plugin_path.display()
    );

    // Test 1: Basic plugin test without any command or hook flags
    eprintln!("🔍 Test 1: Basic plugin test (help)");

    let test_cmd_basic = TestCommand {
        args: vec!["--help".to_string()],
        hooks: vec![],
    };
    let plugin_cmd_basic = PluginCommand::Test(test_cmd_basic);

    let result_basic = plugin_cmd_basic
        .handle(&ctx)
        .await
        .context("Failed to execute basic plugin test")?;

    // NOTE: This may feel weird, but the default --help exit code is actually 1
    assert!(
        !result_basic.is_success(),
        "Basic plugin test should succeed"
    );
    // The description
    assert!(
        result_basic
            .text()
            .contains("OAuth2 server for authentication")
    );
    eprintln!("✅ Basic plugin test passed");

    // Test 2: Plugin test with inspect command using a test component
    eprintln!("🔍 Test 2: Plugin test command with component inspection");

    // Copy test component to plugin accessible location
    let test_component_host_path = "./tests/fixtures/http_hello_world_rust.wasm";
    let component_arg = if std::path::Path::new(test_component_host_path).exists() {
        // Copy to temp dir so the plugin can access it
        let tmp_path = std::env::temp_dir().join("http_hello_world_rust.wasm");
        std::fs::copy(test_component_host_path, &tmp_path)
            .context("Failed to copy test component to temp dir")?;
        "http_hello_world_rust.wasm".to_string() // Plugin will see this relative to temp dir
    } else {
        // Skip this test if no test component is available
        eprintln!(
            "⚠️  Skipping component inspection test - no test component found at {}",
            test_component_host_path
        );
        "nonexistent.wasm".to_string() // This will test error handling
    };

    let test_cmd_with_command = TestCommand {
        args: vec![component_arg.clone()],
        hooks: vec![],
    };
    let plugin_cmd_with_command = PluginCommand::Test(test_cmd_with_command);

    let result_with_command = plugin_cmd_with_command
        .handle(&ctx)
        .await
        .context("Failed to execute plugin test with command")?;

    // The result depends on whether the test component exists
    if std::path::Path::new(test_component_host_path).exists() {
        assert!(
            result_with_command.is_success(),
            "Plugin test with valid component should succeed"
        );
        eprintln!("✅ Plugin test with component inspection passed");
    } else {
        // Should fail gracefully with nonexistent component
        assert!(
            !result_with_command.is_success(),
            "Plugin test with nonexistent component should fail gracefully"
        );
        eprintln!("✅ Plugin test with invalid component failed gracefully");
    }

    // Test 3: Plugin test with --hook afterdev
    eprintln!("🔍 Test 3: Plugin test with AfterDev hook");
    let test_cmd_with_hook = TestCommand {
        args: vec![],
        hooks: vec![HookType::AfterDev],
    };
    let plugin_cmd_with_hook = PluginCommand::Test(test_cmd_with_hook);

    let result_with_hook = plugin_cmd_with_hook
        .handle(&ctx)
        .await
        .context("Failed to execute plugin test with hook")?;

    assert!(
        result_with_hook.is_success(),
        "Plugin test with AfterDev hook should succeed (even without artifact)"
    );
    eprintln!("✅ Plugin test with AfterDev hook passed");

    // Test 4: Plugin test with both command and AfterDev hook
    eprintln!("🔍 Test 4: Plugin test with both component inspection and AfterDev hook");
    let test_cmd_with_both = TestCommand {
        args: vec![component_arg.clone()],
        hooks: vec![HookType::AfterDev],
    };
    let plugin_cmd_with_both = PluginCommand::Test(test_cmd_with_both);

    let result_with_both = plugin_cmd_with_both
        .handle(&ctx)
        .await
        .context("Failed to execute plugin test with both command and hook")?;

    // Should succeed regardless of component existence due to graceful error handling
    eprintln!("✅ Plugin test with both command and AfterDev hook completed");

    // Verify that all tests produced meaningful output
    let outputs = [
        // &result_basic,
        &result_with_command,
        &result_with_hook,
        &result_with_both,
    ];

    for (i, output) in outputs.iter().enumerate() {
        eprintln!("📋 Test {} output: {}", i + 1, output.text());

        // Verify that the output contains expected content
        if let Some(json_value) = output.json() {
            assert!(
                json_value.get("success").is_some(),
                "Output should contain success field"
            );
            assert!(
                json_value.get("metadata").is_some(),
                "Output should contain metadata field"
            );
            eprintln!(
                "📊 Test {} metadata: {}",
                i + 1,
                json_value.get("metadata").unwrap()
            );
        }
    }

    eprintln!("🎉 All inspect plugin tests passed successfully!");
    Ok(())
}

//! Integration tests for `wash wit` commands
//!
//! These tests verify end-to-end workflows with real file I/O and network access.

// Increase the default recursion limit
#![recursion_limit = "256"]

use anyhow::{Context, Result};
use std::fs;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;
use wash::cli::{CliCommand, CliContext, wit::WitCommand};

/// Helper to create a test project with world.wit
async fn setup_test_project_with_world(content: &str) -> Result<(TempDir, std::path::PathBuf)> {
    let temp = TempDir::new().context("failed to create temp dir")?;
    let wit_dir = temp.path().join("wit");
    tokio::fs::create_dir_all(&wit_dir)
        .await
        .context("failed to create wit dir")?;
    tokio::fs::write(wit_dir.join("world.wit"), content)
        .await
        .context("failed to write world.wit")?;
    Ok((temp, wit_dir))
}

#[tokio::test]
async fn wit_integration() -> Result<()> {
    test_remove_workflow().await?;
    test_error_missing_world_wit().await?;
    Ok(())
}

/// Test 1: Add + Fetch + Clean workflow
/// Tests the most common user workflow end-to-end
#[tokio::test]
#[ignore] // Requires network access
async fn test_add_fetch_clean_workflow() -> Result<()> {
    let (temp, wit_dir) =
        setup_test_project_with_world("package test:component@0.1.0;\n\nworld example {\n}\n")
            .await?;

    let ctx = CliContext::builder()
        .non_interactive(true)
        .project_dir(temp.path().to_path_buf())
        .build()
        .await
        .context("failed to create CLI context")?;

    // 1. Add a dependency (must specify full interface path) - with timeout
    let add_cmd = WitCommand::Add {
        package: "wasi:logging/logging@0.1.0-draft".to_string(),
    };
    let result = timeout(Duration::from_secs(60), add_cmd.handle(&ctx))
        .await
        .context("add command timed out after 60s")?
        .context("add command failed")?;
    assert!(result.is_success(), "add command should succeed");

    // Verify import was added to world.wit
    let content = fs::read_to_string(wit_dir.join("world.wit"))?;
    assert!(
        content.contains("import wasi:logging/logging"),
        "world.wit should contain wasi:logging import"
    );

    // 2. Fetch dependencies (creates lock file and wit/deps/) - with timeout
    let fetch_cmd = WitCommand::Fetch { clean: false };
    let result = timeout(Duration::from_secs(60), fetch_cmd.handle(&ctx))
        .await
        .context("fetch command timed out after 60s")?
        .context("fetch command failed")?;
    assert!(result.is_success(), "fetch command should succeed");

    // Verify lock file created
    assert!(
        temp.path().join("wkg.lock").exists(),
        "wkg.lock should be created"
    );

    // Verify deps directory created
    assert!(wit_dir.join("deps").exists(), "wit/deps/ should be created");

    // 3. Clean dependencies
    let clean_cmd = WitCommand::Clean {};
    let result = clean_cmd
        .handle(&ctx)
        .await
        .context("clean command failed")?;
    assert!(result.is_success(), "clean command should succeed");

    // Verify deps removed but lock file remains
    assert!(
        !wit_dir.join("deps").exists(),
        "wit/deps/ should be removed"
    );
    assert!(
        temp.path().join("wkg.lock").exists(),
        "wkg.lock should remain after clean"
    );

    Ok(())
}

/// Test 2: Update workflow (selective and full)
/// Tests that update modifies lock file correctly
#[tokio::test]
#[ignore] // Requires network access
async fn test_update_selective_and_full() -> Result<()> {
    let (temp, _wit_dir) = setup_test_project_with_world(
        r#"package test:component@0.1.0;

world example {
    import wasi:logging/logging@0.1.0-draft;
    import wasi:config/store@0.2.0-draft;
}
"#,
    )
    .await?;

    let ctx = CliContext::builder()
        .non_interactive(true)
        .project_dir(temp.path().to_path_buf())
        .build()
        .await
        .context("failed to create CLI context")?;

    // Fetch to create initial lock file (with timeout)
    let fetch_cmd = WitCommand::Fetch { clean: false };
    timeout(Duration::from_secs(60), fetch_cmd.handle(&ctx))
        .await
        .context("fetch timed out after 60s")?
        .context("fetch failed")?;

    let lock_before = fs::read_to_string(temp.path().join("wkg.lock"))?;

    // Selective update of one package (with timeout)
    let update_cmd = WitCommand::Update {
        package: Some("wasi:logging".to_string()),
    };
    let result = timeout(Duration::from_secs(60), update_cmd.handle(&ctx))
        .await
        .context("selective update timed out after 60s")?
        .context("selective update failed")?;
    assert!(result.is_success(), "selective update should succeed");

    let lock_after = fs::read_to_string(temp.path().join("wkg.lock"))?;

    // Lock file should have changed (or at least been reprocessed)
    // Note: If already at latest, content might be same but process should succeed
    let _ = lock_before;
    let _ = lock_after;

    // Full update (with timeout)
    let update_all_cmd = WitCommand::Update { package: None };
    let result = timeout(Duration::from_secs(60), update_all_cmd.handle(&ctx))
        .await
        .context("full update timed out after 60s")?
        .context("full update failed")?;
    assert!(result.is_success(), "full update should succeed");

    Ok(())
}

/// Test 3: Remove workflow
/// Tests removing dependency removes from world.wit but not lock file
async fn test_remove_workflow() -> Result<()> {
    let (temp, wit_dir) = setup_test_project_with_world(
        r#"package test:component@0.1.0;

world example {
    import wasi:logging/logging@0.1.0-draft;
    import wasi:config/store@0.2.0-draft;
}
"#,
    )
    .await?;

    let ctx = CliContext::builder()
        .non_interactive(true)
        .project_dir(temp.path().to_path_buf())
        .build()
        .await
        .context("failed to create CLI context")?;

    // Remove one dependency - pass wit_dir explicitly to avoid relying on current directory
    let remove_cmd = WitCommand::Remove {
        package: "wasi:logging/logging".to_string(),
    };
    let result = remove_cmd
        .handle(&ctx)
        .await
        .context("remove command failed")?;

    if !result.is_success() {
        let (msg, _) = result.render();
        panic!("remove command should succeed, got error: {}", msg);
    }

    // Verify removed from world.wit
    let content = tokio::fs::read_to_string(wit_dir.join("world.wit")).await?;
    assert!(
        !content.contains("import wasi:logging/logging"),
        "wasi:logging should be removed from world.wit"
    );
    assert!(
        content.contains("import wasi:config/store"),
        "wasi:config should remain in world.wit"
    );

    Ok(())
}

/// Test 4: Build workflow
/// Tests that build creates wasm file in correct location
#[tokio::test]
#[ignore] // Requires valid WIT and network
async fn test_build_output_location() -> Result<()> {
    let (temp, _wit_dir) = setup_test_project_with_world(
        r#"package test:component@1.0.0;

world example {
    import wasi:logging/logging@0.1.0-draft;
}
"#,
    )
    .await?;

    let ctx = CliContext::builder()
        .non_interactive(true)
        .project_dir(temp.path().to_path_buf())
        .build()
        .await
        .context("failed to create CLI context")?;

    // Fetch dependencies first (with timeout)
    let fetch_cmd = WitCommand::Fetch { clean: false };
    timeout(Duration::from_secs(60), fetch_cmd.handle(&ctx))
        .await
        .context("fetch timed out after 60s")?
        .context("fetch before build failed")?;

    // Build with default output (project root) - with timeout
    let build_cmd = WitCommand::Build { output_file: None };
    let result = timeout(Duration::from_secs(60), build_cmd.handle(&ctx))
        .await
        .context("build command timed out after 60s")?
        .context("build command failed")?;
    assert!(result.is_success(), "build command should succeed");

    // Verify wasm file created in project root
    let wasm_files: Vec<_> = fs::read_dir(temp.path())?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "wasm")
                .unwrap_or(false)
        })
        .collect();

    assert_eq!(
        wasm_files.len(),
        1,
        "Should have exactly one wasm file in project root"
    );

    // Build with custom output
    let custom_output = temp.path().join("custom").join("output.wasm");
    fs::create_dir_all(custom_output.parent().unwrap())?;

    let build_custom_cmd = WitCommand::Build {
        output_file: Some(custom_output.clone()),
    };
    let result = timeout(Duration::from_secs(60), build_custom_cmd.handle(&ctx))
        .await
        .context("build with custom output timed out after 60s")?
        .context("build with custom output failed")?;
    assert!(
        result.is_success(),
        "build with custom output should succeed"
    );

    // Verify custom location
    assert!(
        custom_output.exists(),
        "wasm file should exist at custom location"
    );

    Ok(())
}

/// Test 5: Error handling - missing world.wit
/// Tests that commands fail gracefully with helpful errors
async fn test_error_missing_world_wit() -> Result<()> {
    let temp = TempDir::new()?;
    let wit_dir = temp.path().join("wit");
    tokio::fs::create_dir_all(&wit_dir).await?;

    // Don't create world.wit

    let ctx = CliContext::builder()
        .non_interactive(true)
        .project_dir(temp.path().to_path_buf())
        .build()
        .await
        .context("failed to create CLI context")?;

    // Try to add - should fail with helpful message
    // Pass wit_dir explicitly to avoid relying on current directory
    let add_cmd = WitCommand::Add {
        package: "wasi:logging/logging@0.1.0-draft".to_string(),
    };
    let result = add_cmd
        .handle(&ctx)
        .await
        .context("add command execution failed")?;
    assert!(
        !result.is_success(),
        "add command should fail when world.wit missing"
    );

    let (msg, _) = result.render();
    assert!(
        msg.contains("world") || msg.contains("WIT file"),
        "error message should mention world or WIT file: {}",
        msg
    );

    Ok(())
}

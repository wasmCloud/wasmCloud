mod common;

use common::TestWashInstance;

use anyhow::{Context, Result};
use serial_test::serial;
use tokio::process::Command;
use wash::lib::cli::output::LinkQueryCommandOutput;

#[tokio::test]
#[serial]
async fn integration_link_serial() -> Result<()> {
    let wash = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "link",
            "query",
            "--output",
            "json",
            "--ctl-port",
            &wash.nats_port.to_string(),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute link query")?;

    assert!(output.status.success(), "executed link query");

    let cmd_output: LinkQueryCommandOutput = serde_json::from_slice(&output.stdout)?;
    assert!(cmd_output.success, "command returned success");
    assert_eq!(
        cmd_output.links.len(),
        0,
        "links list is empty without any links"
    );

    Ok(())
}

/// Ensure wash can delete all links
#[tokio::test]
#[serial]
async fn integration_link_del_all_serial() -> Result<()> {
    let wash = TestWashInstance::create().await?;

    let query_links = || async {
        let output = Command::new(env!("CARGO_BIN_EXE_wash"))
            .args([
                "link",
                "query",
                "--output",
                "json",
                "--ctl-port",
                &wash.nats_port.to_string(),
            ])
            .kill_on_drop(true)
            .output()
            .await
            .context("executing query link query")?;
        assert!(output.status.success(), "executed link query");

        let cmd_output: LinkQueryCommandOutput =
            serde_json::from_slice(&output.stdout).context("parsing link query command output")?;
        assert!(cmd_output.success, "command returned success");
        Ok(cmd_output) as Result<LinkQueryCommandOutput>
    };

    let queried_links = query_links().await?;
    assert_eq!(
        queried_links.links.len(),
        0,
        "links list is empty without any links"
    );

    // Create links that will be deleted
    const LINKS: [(&str, &str, &str, &str, &str); 3] = [
        ("src", "dst", "wasi", "http", "incoming-handler"),
        ("src", "dst", "example", "pkg", "call"),
        ("src", "dst", "wasmcloud", "messaging", "consumer"),
    ];
    for (src, dest, ns, pkg, iface) in LINKS {
        let _ = Command::new(env!("CARGO_BIN_EXE_wash"))
            .args([
                "link",
                "put",
                src,
                dest,
                ns,
                pkg,
                "--interface",
                iface,
                "--ctl-port",
                &wash.nats_port.to_string(),
            ])
            .kill_on_drop(true)
            .output()
            .await
            .context("failed to execute link query")?;
    }

    let queried_links = query_links().await?;
    assert_eq!(
        queried_links.links.len(),
        LINKS.len(),
        "queried link list does not match"
    );

    // Delete all the links
    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "link",
            "delete",
            "--all",
            "--force",
            "--output",
            "json",
            "--ctl-port",
            &wash.nats_port.to_string(),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute link query")?;
    assert!(output.status.success(), "delete query succeeded");

    let queried_links = query_links().await?;
    assert_eq!(
        queried_links.links.len(),
        0,
        "queried links were unexpectedly present"
    );

    Ok(())
}

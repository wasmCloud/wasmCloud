//! This module contains tests for the host's ability to handle and respond to auctions

use core::str;
use std::collections::HashMap;

use anyhow::Context as _;
use test_components::RUST_HTTP_HELLO_WORLD;
use tracing::instrument;
use wasmcloud_control_interface::ComponentAuctionRequestBuilder;
use wasmcloud_test_util::{
    assert_scale_component, assert_start_provider, host::WasmCloudTestHost,
    provider::StartProviderArgs,
};

pub mod common;
use common::{nats::start_nats, providers};

const LATTICE: &str = "auctions";
const COMPONENT_REF: &str = "ghcr.io/wasmcloud/hello:1.0.0";
const COMPONENT_ID: &str = "http_hello_world";
const PROVIDER_ID: &str = "http_server";

/// Auction components and ensure the host properly handles the auction
#[instrument(skip_all, ret)]
#[tokio::test]
async fn components() -> anyhow::Result<()> {
    let (nats_server, nats_url, nats_client) =
        start_nats().await.context("failed to start NATS")?;

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICE.to_string())
        .build();
    // Build the host
    let host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let auction = ctl_client
        .perform_component_auction(
            ComponentAuctionRequestBuilder::new(COMPONENT_REF, COMPONENT_ID).build(),
        )
        .await;
    assert!(
        auction.is_ok(),
        "failed to perform component auction: {:?}",
        auction
    );
    assert_eq!(auction.unwrap().len(), 1, "unexpected number of responses");

    let auction_with_constraints = ctl_client
        .perform_component_auction(
            ComponentAuctionRequestBuilder::new(COMPONENT_REF, COMPONENT_ID)
                // This label is hardcoded on the test hosts
                .constraints(HashMap::from([(
                    "wasmcloud_test".to_string(),
                    "true".to_string(),
                )]))
                .build(),
        )
        .await;
    assert!(
        auction_with_constraints.is_ok(),
        "failed to perform component auction: {:?}",
        auction_with_constraints
    );
    assert_eq!(
        auction_with_constraints.unwrap().len(),
        1,
        "unexpected number of responses"
    );

    let auction_with_resource_limits = ctl_client
        .perform_component_auction(
            ComponentAuctionRequestBuilder::new(COMPONENT_REF, COMPONENT_ID)
                // If the host has less than 50 bytes of RAM available, we have other problems
                .max_memory(5)
                .max_instances(10)
                .build(),
        )
        .await;
    assert!(
        auction_with_resource_limits.is_ok(),
        "failed to perform component auction: {:?}",
        auction_with_resource_limits
    );
    assert_eq!(
        auction_with_resource_limits.unwrap().len(),
        1,
        "unexpected number of responses"
    );

    let auction_with_excessive_resource_limits = ctl_client
        .perform_component_auction(
            // Exceeding the host's max linear memory (256 MiB by default)
            ComponentAuctionRequestBuilder::new(COMPONENT_REF, COMPONENT_ID)
                // In total, requesting 300 MiB of memory
                .max_memory(300 * 1024 * 1024)
                .build(),
        )
        .await;
    assert!(
        auction_with_excessive_resource_limits.is_ok(),
        "failed to perform component auction: {:?}",
        auction_with_excessive_resource_limits
    );
    assert_eq!(
        auction_with_excessive_resource_limits.unwrap().len(),
        0,
        "unexpected number of responses"
    );

    let auction_too_many_components = ctl_client
        .perform_component_auction(
            // Exceeding the host's max components (10000 by default)
            ComponentAuctionRequestBuilder::new(COMPONENT_REF, COMPONENT_ID)
                .max_instances(10001)
                .build(),
        )
        .await;
    assert!(
        auction_too_many_components.is_ok(),
        "failed to perform component auction: {:?}",
        auction_too_many_components
    );
    assert_eq!(
        auction_too_many_components.unwrap().len(),
        0,
        "unexpected number of responses"
    );

    // Start a component with the previously auctioned component ID
    assert_scale_component(
        &ctl_client,
        &host.host_key(),
        format!("file://{RUST_HTTP_HELLO_WORLD}"),
        COMPONENT_ID,
        None,
        5,
        Vec::new(),
    )
    .await
    .context("failed to scale `rust-http-hello-world` component")?;

    let auction_no_response = ctl_client
        .perform_component_auction(
            ComponentAuctionRequestBuilder::new(COMPONENT_REF, COMPONENT_ID).build(),
        )
        .await;
    assert!(
        auction_no_response.is_ok(),
        "failed to perform component auction: {:?}",
        auction_no_response
    );
    // The auction should be successful, but the host shouldn't respond
    // because a component with that ID is already running
    assert!(
        auction_no_response.unwrap().is_empty(),
        "unexpected number of responses"
    );

    nats_server.stop().await.context("failed to stop NATS")?;
    Ok(())
}

/// Auction providers and ensure the host properly handles the auction
#[instrument(skip_all, ret)]
#[tokio::test]
async fn providers() -> anyhow::Result<()> {
    let (nats_server, nats_url, nats_client) =
        start_nats().await.context("failed to start NATS")?;

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICE.to_string())
        .build();
    // Build the host
    let host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let auction = ctl_client
        .perform_provider_auction(COMPONENT_REF, PROVIDER_ID, HashMap::new())
        .await;
    assert!(
        auction.is_ok(),
        "failed to perform provider auction: {:?}",
        auction
    );
    assert_eq!(auction.unwrap().len(), 1, "unexpected number of responses");

    let auction_with_constraints = ctl_client
        .perform_provider_auction(
            COMPONENT_REF,
            PROVIDER_ID,
            HashMap::from([("wasmcloud_test".to_string(), "true".to_string())]),
        )
        .await;
    assert!(
        auction_with_constraints.is_ok(),
        "failed to perform provider auction: {:?}",
        auction_with_constraints
    );
    assert_eq!(
        auction_with_constraints.unwrap().len(),
        1,
        "unexpected number of responses"
    );

    let auction_with_mismatched_constraints = ctl_client
        .perform_provider_auction(
            COMPONENT_REF,
            PROVIDER_ID,
            HashMap::from([("foo".to_string(), "bar".to_string())]),
        )
        .await;
    assert!(
        auction_with_mismatched_constraints.is_ok(),
        "failed to perform provider auction: {:?}",
        auction_with_mismatched_constraints
    );
    assert_eq!(
        auction_with_mismatched_constraints.unwrap().len(),
        0,
        "unexpected number of responses"
    );

    let http_server = providers::rust_http_server().await;

    assert_start_provider(StartProviderArgs {
        client: &ctl_client,
        lattice: LATTICE,
        host_key: &host.host_key(),
        provider_key: &http_server.subject,
        provider_id: PROVIDER_ID,
        url: &http_server.url(),
        config: vec![],
    })
    .await
    .context("failed to start provider")?;

    // Now that the provider is running, the host should not respond to the auction
    let auction = ctl_client
        .perform_provider_auction(COMPONENT_REF, PROVIDER_ID, HashMap::new())
        .await;
    assert!(
        auction.is_ok(),
        "failed to perform provider auction: {:?}",
        auction
    );
    assert_eq!(auction.unwrap().len(), 0, "unexpected number of responses");

    nats_server.stop().await.context("failed to stop NATS")?;

    Ok(())
}

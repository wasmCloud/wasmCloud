// todo(vados-cosmonic): re-enable once http-server is working

// use std::collections::HashMap;
// use std::net::Ipv6Addr;
// use std::sync::Arc;

// use anyhow::{anyhow, Context, Result};
// use nkeys::KeyPair;
// use serde::Deserialize;
// use serde_json::json;
// use tokio::fs;
// use tokio::time::Duration;
// use tokio::try_join;
// use url::Url;
// use wascap::jwt;

// use wascap::wasm::extract_claims;
// use wasmcloud_control_interface::ClientBuilder;
// use wasmcloud_host::wasmbus::{Host, HostConfig};

// pub mod common;
// use common::free_port;

// use crate::common::nats::start_nats;
// use crate::common::{
//     assert_advertise_link, assert_start_actor, assert_start_provider, copy_par, stop_server,
// };

// const TEST_LATTICE: &str = "test-lattice-controller";

// /// Test all functionality for the lattice-controller provider
// #[tokio::test(flavor = "multi_thread")]
// async fn lattice_controller_suite() -> Result<()> {
//     let (nats_server, stop_nats_tx, nats_url, nats_client) = start_nats()
//         .await
//         .context("failed to start backing services")?;

//     let httpserver_port = free_port().await?;
//     let httpserver_base_url = format!("http://[{}]:{httpserver_port}", Ipv6Addr::LOCALHOST);

//     // Get provider key/url for pre-built httpserver provider
//     let httpserver_provider_key = KeyPair::from_seed(test_providers::RUST_HTTPSERVER_SUBJECT)
//         .context("failed to parse `rust-httpserver` provider key")?;
//     let (httpserver_provider_url, _httpserver_provider_tmp_path) =
//         copy_par(test_providers::RUST_HTTPSERVER)
//             .await
//             .context("failed to build copied PAR")?;

//     // Get provider key/url for pre-built lattice-controller provider (subject of this test)
//     let lattice_controller_provider_key =
//         KeyPair::from_seed(test_providers::RUST_LATTICE_CONTROLLER_SUBJECT)
//             .context("failed to parse `rust-lattice-controller` provider key")?;
//     let lattice_controller_provider_url =
//         Url::from_file_path(test_providers::RUST_LATTICE_CONTROLLER)
//             .map_err(|()| anyhow!("failed to construct provider ref"))?;

//     // Get actor key/url for pre-built component reactor actor
//     let test_component_actor_url =
//         Url::from_file_path(test_actors::RUST_BUILTINS_COMPONENT_REACTOR_SIGNED)
//             .map_err(|()| anyhow!("failed to construct component reactor actor ref"))?;

//     // Get actor key/url for pre-built  actor
//     // this actor is used in testing start/stop
//     let lattice_control_http_smithy_actor_url =
//         Url::from_file_path(test_actors::RUST_LATTICE_CONTROL_HTTP_SMITHY_SIGNED)
//             .map_err(|()| anyhow!("failed to construct actor ref"))?;

//     // Build client for initial interaction with the lattice
//     let ctl_client = ClientBuilder::new(nats_client.clone())
//         .lattice(TEST_LATTICE.to_string())
//         .build();

//     // Start a wasmcloud host
//     let cluster_key = Arc::new(KeyPair::new_cluster());
//     let host_key = Arc::new(KeyPair::new_server());
//     let (_host, shutdown_host) = Host::new(HostConfig {
//         ctl_nats_url: nats_url.clone(),
//         rpc_nats_url: nats_url.clone(),
//         // NOTE: the host *must* set a default RPC timeout that is higher than the value
//         // used by the lattice controller, otherwise waiting for things like auctions will fail
//         // with timeouts.
//         //
//         // For example, the default auction timeout for lattice-controller is 3s (previously 5s)
//         // so the rpc_timeout in the host must be larger than that value
//         //
//         // By default the lattice-controller waits for auctions 3x as long as  as a regular timeout
//         rpc_timeout: tokio::time::Duration::from_secs(6),
//         lattice: TEST_LATTICE.into(),
//         cluster_key: Some(Arc::clone(&cluster_key)),
//         cluster_issuers: Some(vec![cluster_key.public_key(), cluster_key.public_key()]),
//         host_key: Some(Arc::clone(&host_key)),
//         provider_shutdown_delay: Some(Duration::from_millis(300)),
//         allow_file_load: true,
//         ..Default::default()
//     })
//     .await
//     .context("failed to initialize host")?;

//     // Retrieve claims from actor
//     let jwt::Token {
//         claims: lattice_control_http_smithy_claims,
//         ..
//     } = extract_claims(
//         fs::read(test_actors::RUST_LATTICE_CONTROL_HTTP_SMITHY_SIGNED)
//             .await
//             .context("failed to read http smithy actor wasm")?,
//     )
//     .context("failed to extract lattice-controller http smithy actor claims")?
//     .context("component actor claims missing")?;

//     // Link the actor to both providers
//     //
//     // this must be done *before* the provider is started to avoid a race condition
//     // to ensure the link is advertised before the actor would normally subscribe
//     assert_advertise_link(
//         &ctl_client,
//         &lattice_control_http_smithy_claims,
//         &httpserver_provider_key,
//         "wasmcloud:httpserver",
//         "default",
//         HashMap::from([(
//             "config_json".into(),
//             format!(
//                 r#"{{"address":"[{}]:{httpserver_port}"}}"#,
//                 Ipv6Addr::LOCALHOST,
//             ),
//         )]),
//     )
//     .await?;
//     assert_advertise_link(
//         &ctl_client,
//         &lattice_control_http_smithy_claims,
//         &lattice_controller_provider_key,
//         "wasmcloud:latticecontrol",
//         "default",
//         HashMap::new(),
//     )
//     .await?;

//     // Start the lattice-control-http-smithy actor
//     assert_start_actor(
//         &ctl_client,
//         &nats_client,
//         TEST_LATTICE,
//         &host_key,
//         &lattice_control_http_smithy_actor_url,
//         1,
//     )
//     .await?;

//     // Start the HTTP provider
//     assert_start_provider(
//         &ctl_client,
//         &nats_client,
//         TEST_LATTICE,
//         &host_key,
//         &httpserver_provider_key,
//         "default",
//         httpserver_provider_url,
//         None,
//     )
//     .await?;

//     // Start the lattice-controller provider
//     assert_start_provider(
//         &ctl_client,
//         &nats_client,
//         TEST_LATTICE,
//         &host_key,
//         &lattice_controller_provider_key,
//         "default",
//         lattice_controller_provider_url,
//         None,
//     )
//     .await?;

//     let http_client = reqwest::Client::default();

//     // Set credentials & lattice ID to use during the test
//     let resp_json: ResponseEnvelope<CtlOperationAck> = http_client
//         .post(format!("{httpserver_base_url}/set-lattice-credentials"))
//         .body(serde_json::to_string(&json!({
//             "latticeId": TEST_LATTICE,
//             "userJwt": null,
//             "userSeed": null,
//             "natsUrl": nats_url.to_string().replace("nats://localhost", "127.0.0.1"),
//             "jsDomain": null,
//         }))?)
//         .send()
//         .await
//         .context("failed to perform POST /set-lattice-credentials")?
//         .json()
//         .await
//         .context("failed to read /set-lattice-credentials response body as json")?;
//     assert_eq!(
//         resp_json.status, "success",
//         "set-lattice-credentials succeeded"
//     );
//     assert!(
//         resp_json.data.accepted,
//         "set-lattice-credentials operation ack was accepted",
//     );

//     // Perform POST request to trigger a lattice-control host-inventory
//     let resp_json: ResponseEnvelope<Vec<LatticeHost>> = http_client
//         .post(format!("{httpserver_base_url}/get-hosts"))
//         .body(serde_json::to_string(&json!({
//             "latticeId": TEST_LATTICE,
//         }))?)
//         .send()
//         .await
//         .context("failed to perform POST /get-hosts")?
//         .json()
//         .await
//         .context("failed to read /get-hosts response body as json")?;
//     assert_eq!(resp_json.status, "success", "get-hosts succeeded");
//     assert_eq!(resp_json.data.len(), 1, "a host is present");

//     // Test operations on actors
//     test_ops_actors(
//         &httpserver_base_url,
//         &http_client,
//         TEST_LATTICE,
//         &test_component_actor_url,
//     )
//     .await?;

//     // Shutdown the host and backing services
//     shutdown_host.await?;
//     try_join!(stop_server(nats_server, stop_nats_tx),).context("failed to stop servers")?;

//     Ok(())
// }

// /// Helper function that tests operations on actors:
// ///
// /// - auction_actor
// /// - start_actor
// /// - scale_actor
// /// - stop_actor
// ///
// /// In order to perform these tests, this function assumes you have a:
// ///
// /// - running httpserver provider, accessible at `base_url`
// /// - running lattice-control-http-smithy actor, connected to that httpserver provider
// async fn test_ops_actors(
//     base_url: impl AsRef<str>,
//     http_client: &reqwest::Client,
//     lattice: impl AsRef<str>,
//     actor_ref: impl AsRef<str>,
// ) -> Result<()> {
//     let base_url = base_url.as_ref();
//     let actor_ref = actor_ref.as_ref();

//     // Perform POST request to trigger a latticecontrol auction-actor
//     let resp_json: ResponseEnvelope<Vec<ActorAuctionAck>> = http_client
//         .post(format!("{base_url}/auction-actor"))
//         .body(serde_json::to_string(&json!({
//             "latticeId": lattice.as_ref(),
//             "actorRef": actor_ref,
//             "constraints": HashMap::<String,String>::new(),
//         }))?)
//         .send()
//         .await
//         .context("failed to perform POST /auction-actor")?
//         .json()
//         .await
//         .context("failed to read /auction-actor response body as json")?;
//     assert_eq!(resp_json.status, "success", "auction-actor succeeded");
//     // TODO: the single host in the lattice isn't bidding...
//     assert_eq!(resp_json.data.len(), 1, "there is exactly one bidding host");
//     let ack = &resp_json
//         .data
//         .first()
//         .context("missing single bidder host")?;
//     assert_eq!(
//         ack.actor_ref, actor_ref,
//         "actor_ref on ack matches actor_ref"
//     );

//     let host_id = &ack.host_id;
//     assert!(!host_id.is_empty(), "host ID is non-empty");

//     // Perform POST request to trigger a lattice-control scale-actor (starting the first actor)
//     let resp_json: ResponseEnvelope<CtlOperationAck> = http_client
//         .post(format!("{base_url}/scale-actor"))
//         .body(serde_json::to_string(&json!({
//             "latticeId": lattice.as_ref(),
//             "actorRef": actor_ref,
//             "hostId": host_id,
//             "annotations": HashMap::<String, String>::new(),
//             "count": 1,
//         }))?)
//         .send()
//         .await
//         .context("failed to perform POST /scale-actor")?
//         .json()
//         .await
//         .context("failed to read /scale-actor response body as json")?;
//     assert_eq!(resp_json.status, "success", "scale-actor succeeded");
//     assert!(
//         resp_json.data.accepted && resp_json.data.error.is_empty(),
//         "ctl operation accepted"
//     );

//     // Perform POST request to trigger a lattice-control host-inventory to find the started actor
//     let resp_json: ResponseEnvelope<HostInventory> = http_client
//         .post(format!("{base_url}/get-host-inventory"))
//         .body(serde_json::to_string(&json!({
//             "latticeId": lattice.as_ref(),
//             "hostId": host_id,
//         }))?)
//         .send()
//         .await
//         .context("failed to perform POST /get-host-inventory")?
//         .json()
//         .await
//         .context("failed to read /get-host-inventory response body as json")?;
//     assert_eq!(resp_json.status, "success", "get-host-inventory succeeded");

//     // Wait for the actor shows up in host inventory
//     let actor_id = tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
//         loop {
//             if let Some(actor_id) = http_client
//                 .post(format!("{base_url}/get-host-inventory"))
//                 .body(serde_json::to_string(&json!({
//                     "latticeId": lattice.as_ref(),
//                     "hostId": host_id,
//                 }))?)
//                 .send()
//                 .await
//                 .context("failed to perform POST /get-host-inventory")?
//                 .json::<ResponseEnvelope<HostInventory>>()
//                 .await
//                 .context("failed to read /get-host-inventory response body as json")?
//                 .data
//                 .actors
//                 .iter()
//                 .find(|a| a.image_ref.as_ref().is_some_and(|v| v == actor_ref))
//                 .map(|ad| &ad.id)
//             {
//                 return Ok(actor_id.clone()) as Result<String>;
//             }
//             tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
//         }
//     })
//     .await
//     .context("timeout failed")?
//     .context("failed to get actor ID")?;
//     assert!(!actor_id.is_empty(), "actor ID is non-empty");

//     // Perform POST request to trigger a lattice-control scale-actor (getting one more actor instance)
//     let resp_json: ResponseEnvelope<CtlOperationAck> = http_client
//         .post(format!("{base_url}/scale-actor"))
//         .body(serde_json::to_string(&json!({
//             "latticeId": lattice.as_ref(),
//             "actorRef": actor_ref,
//             "hostId": host_id,
//             "actorId": actor_id,
//             "annotations": HashMap::<String, String>::new(),
//             "count": 2,
//         }))?)
//         .send()
//         .await
//         .context("failed to perform POST /scale-actor")?
//         .json()
//         .await
//         .context("failed to read /scale-actor response body as json")?;
//     assert_eq!(resp_json.status, "success", "scale-actor succeeded");
//     assert!(
//         resp_json.data.accepted && resp_json.data.error.is_empty(),
//         "ctl operation accepted"
//     );

//     // NOTE(thomastaylor312): The old smithy interface doesn't return the max_instances number, so
//     // we can't verify that it scales. Doing 2 scale commands so close to each other actually is a
//     // race condition because we spawn the handle scale task in parallel in the host. This means if
//     // two requests go in at the same time, the scale actor up could happen after the scale actor
//     // down. Only one happens at a time due to the RwLock on the actors map but who gets the lock
//     // first is not deterministic. So we wait here instead

//     tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

//     // Perform POST request to trigger a lattice-control scale-actor to go to 0
//     let resp_json: ResponseEnvelope<CtlOperationAck> = http_client
//         .post(format!("{base_url}/scale-actor"))
//         .body(serde_json::to_string(&json!({
//             "latticeId": lattice.as_ref(),
//             "actorRef": actor_ref,
//             "hostId": host_id,
//             "actorId": actor_id,
//             "annotations": HashMap::<String, String>::new(),
//             "count": 0,
//         }))?)
//         .send()
//         .await
//         .context("failed to perform POST /scale-actor")?
//         .json()
//         .await
//         .context("failed to read /scale-actor response body as json")?;
//     assert_eq!(resp_json.status, "success", "scale-actor succeeded");
//     assert!(
//         resp_json.data.accepted && resp_json.data.error.is_empty(),
//         "ctl operation accepted"
//     );

//     // Perform POST request to trigger a lattice-control host-inventory
//     tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
//         loop {
//             let resp_json: ResponseEnvelope<HostInventory> = http_client
//                 .post(format!("{base_url}/get-host-inventory"))
//                 .body(serde_json::to_string(&json!({
//                     "latticeId": lattice.as_ref(),
//                     "hostId": host_id,
//                 }))?)
//                 .send()
//                 .await
//                 .context("failed to perform POST /get-host-inventory")?
//                 .json()
//                 .await
//                 .context("failed to read /get-host-inventory response body as json")?;
//             assert_eq!(resp_json.status, "success", "get-host-inventory succeeded");
//             if !resp_json
//                 .data
//                 .actors
//                 .iter()
//                 .any(|a| a.image_ref.as_ref().is_some_and(|v| v == actor_ref))
//             {
//                 return Ok::<(), anyhow::Error>(());
//             }
//             tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
//         }
//     })
//     .await
//     .context("reached timeout waiting for actor scale down")?
//     .expect("inventory fetch failed");

//     Ok(())
// }

// #[derive(Debug, PartialEq, Eq, Deserialize)]
// struct ResponseEnvelope<T> {
//     pub status: String,
//     pub data: T,
// }

// /// Copy of [`wasmcloud_interface_lattice_control::ActorAuctionAck`] (normally bindgen-generated)
// #[derive(Debug, Deserialize, PartialEq, Eq)]
// #[serde(rename_all = "camelCase")]
// pub struct ActorAuctionAck {
//     actor_ref: String,
//     host_id: String,
// }

// /// Copy of [`wasmcloud_interface_lattice_control::CtlOperationAck`] (normally bindgen-generated)
// #[derive(Debug, Deserialize, PartialEq, Eq)]
// #[serde(rename_all = "camelCase")]
// pub struct CtlOperationAck {
//     accepted: bool,
//     error: String,
// }

// /// Copy of [`wasmcloud_interface_lattice_control::HostInventory`] (normally bindgen-generated)
// #[derive(Debug, Deserialize, PartialEq, Eq)]
// #[serde(rename_all = "camelCase")]
// pub struct HostInventory {
//     host_id: String,
//     labels: HashMap<String, String>,
//     actors: Vec<ActorDescription>,
//     providers: Vec<ProviderDescription>,
// }

// /// Copy of [`wasmcloud_interface_lattice_control::ActorDescription`] (normally bindgen-generated)
// #[derive(Debug, Deserialize, PartialEq, Eq)]
// #[serde(rename_all = "camelCase")]
// pub struct ActorDescription {
//     id: String,
//     image_ref: Option<String>,
//     name: Option<String>,
//     instances: Vec<ActorInstance>,
// }

// /// Copy of [`wasmcloud_interface_lattice_control::ActorInstance`] (normally bindgen-generated)
// #[derive(Debug, Deserialize, PartialEq, Eq)]
// #[serde(rename_all = "camelCase")]
// pub struct ActorInstance {
//     instance_id: String,
//     revision: i32,
//     annotations: HashMap<String, String>,
// }

// /// Copy of [`wasmcloud_interface_lattice_control::ProviderDescription`] (normally bindgen-generated)
// #[derive(Debug, Deserialize, PartialEq, Eq)]
// #[serde(rename_all = "camelCase")]
// pub struct ProviderDescription {
//     id: String,
//     link_name: String,
//     image_ref: Option<String>,
//     name: Option<String>,
//     revision: i32,
//     annotations: HashMap<String, String>,
// }

// /// Copy of [`wasmcloud_interface_lattice_control::Host`] (normally bindgen-generated)
// #[derive(Debug, Deserialize, PartialEq, Eq)]
// #[serde(rename_all = "camelCase")]
// pub struct LatticeHost {
//     id: String,
//     uptime_seconds: u64,
//     uptime_human: Option<String>,
//     labels: HashMap<String, String>,
//     version: Option<String>,
//     cluster_issuers: Option<String>,
//     js_domain: Option<String>,
//     ctl_host: Option<String>,
//     prov_rpc_host: Option<String>,
//     rpc_host: Option<String>,
//     lattice: Option<String>,
// }

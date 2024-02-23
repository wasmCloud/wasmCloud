// todo(vadossi-cosmonic): re-enable once http-server is working

// use std::collections::{BTreeSet, HashMap};
// use std::env;
// use std::env::consts::{ARCH, FAMILY, OS};
// use std::net::Ipv6Addr;
// use std::str::FromStr;
// use std::sync::Arc;
// use std::time::Duration;

// use anyhow::{anyhow, bail, ensure, Context};
// use http_body_util::BodyExt;
// use hyper::body::Bytes;
// use hyper::service::service_fn;
// use nkeys::KeyPair;
// use redis::{Commands, ConnectionLike};
// use serde::Deserialize;
// use tokio::net::TcpListener;
// use tokio::{fs, spawn, try_join};
// use tokio_stream::StreamExt;
// use tracing_subscriber::prelude::*;
// use url::Url;
// use uuid::Uuid;
// use wascap::jwt;
// use wascap::wasm::extract_claims;
// use wasmcloud_control_interface::{
//     ActorAuctionAck, ActorDescription, ActorInstance, ClientBuilder, CtlOperationAck,
//     Host as HostInfo, HostInventory, ProviderAuctionAck,
// };
// use wasmcloud_host::wasmbus::{Host, HostConfig};

// pub mod common;
// use common::nats::start_nats;
// use common::redis::start_redis;
// use common::{
//     assert_advertise_link, assert_config_put, assert_delete_label, assert_put_label,
//     assert_remove_link, assert_scale_actor, assert_start_actor, assert_start_provider, free_port,
//     stop_server, tempdir,
// };

// async fn http_handler(
//     req: hyper::Request<hyper::body::Incoming>,
// ) -> anyhow::Result<hyper::Response<http_body_util::Full<Bytes>>> {
//     let (
//         hyper::http::request::Parts {
//             method,
//             uri,
//             headers: _, // TODO: Verify headers
//             ..
//         },
//         body,
//     ) = req.into_parts();
//     ensure!(method == &hyper::Method::PUT);
//     ensure!(uri == "/test");
//     let body = body
//         .collect()
//         .await
//         .context("failed to read body")?
//         .to_bytes();
//     ensure!(body == b"test"[..]);
//     let res = hyper::Response::builder()
//         .status(hyper::StatusCode::OK)
//         .body(http_body_util::Full::new("test".into()))
//         .context("failed to construct response")?;
//     Ok(res)
// }

// async fn assert_handle_http_request(
//     http_port: u16,
//     nats_client: async_nats::Client,
//     redis_client: &mut redis::Client,
// ) -> anyhow::Result<(Vec<u8>, HashMap<String, Vec<u8>>)> {
//     let (mut nats_publish_sub, mut nats_request_sub, mut nats_request_multi_sub) = try_join!(
//         nats_client.subscribe("test-messaging-publish"),
//         nats_client.subscribe("test-messaging-request"),
//         nats_client.subscribe("test-messaging-request-multi"),
//     )
//     .context("failed to subscribe to NATS topics")?;

//     redis_client
//         .req_command(&redis::Cmd::set("foo", "bar"))
//         .context("failed to set `foo` key in Redis")?;

//     let nats_requests = spawn(async move {
//         let res = nats_request_sub
//             .next()
//             .await
//             .context("failed to receive NATS response to `request`")?;
//         ensure!(res.payload == "foo");
//         let reply = res.reply.context("no reply set on `request`")?;
//         nats_client
//             .publish(reply, "bar".into())
//             .await
//             .context("failed to publish response to `request`")?;

//         let res = nats_request_multi_sub
//             .next()
//             .await
//             .context("failed to receive NATS response to `request_multi`")?;
//         ensure!(res.payload == "foo");
//         let reply = res.reply.context("no reply on set `request_multi`")?;
//         nats_client
//             .publish(reply, "bar".into())
//             .await
//             .context("failed to publish response to `request_multi`")?;
//         Ok(())
//     });

//     let lis = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0))
//         .await
//         .context("failed to start TCP listener")?;
//     let out_port = lis
//         .local_addr()
//         .context("failed to query listener local address")?
//         .port();
//     let http_server = spawn(async move {
//         use hyper_util::rt::{TokioExecutor, TokioIo};
//         use hyper_util::server::conn::auto;

//         let (stream, _addr) = lis.accept().await.expect("failed to accept connection");
//         auto::Builder::new(TokioExecutor::new())
//             .serve_connection(TokioIo::new(stream), service_fn(http_handler))
//             .await
//             .expect("failed to handle HTTP request");
//     });

//     let http_client = reqwest::Client::builder()
//         .timeout(Duration::from_secs(20))
//         .connect_timeout(Duration::from_secs(20))
//         .build()
//         .context("failed to build HTTP client")?;
//     let http_res = http_client
//         .post(format!("http://localhost:{http_port}/foo?bar=baz"))
//         .header("test-header", "test-value")
//         .body(format!(
//             r#"{{"min":42,"max":4242,"port":{out_port},"config_key":"test-config-data"}}"#
//         ))
//         .send()
//         .await
//         .context("failed to connect to server")?
//         .text()
//         .await
//         .context("failed to get response text")?;

//     // TODO: Instead of duplication here, reuse the same struct used in `wasmcloud-runtime` tests
//     #[derive(Deserialize)]
//     #[serde(deny_unknown_fields)]
//     // NOTE: If values are truly random, we have nothing to assert for some of these fields
//     struct Response {
//         #[allow(dead_code)]
//         get_random_bytes: [u8; 8],
//         #[allow(dead_code)]
//         get_random_u64: u64,
//         guid: String,
//         random_in_range: u32,
//         #[allow(dead_code)]
//         random_32: u32,
//         #[allow(dead_code)]
//         long_value: String,
//         config_value: Vec<u8>,
//         all_config: Vec<(String, Vec<u8>)>,
//     }
//     let Response {
//         get_random_bytes: _,
//         get_random_u64: _,
//         guid,
//         random_32: _,
//         random_in_range,
//         long_value: _,
//         config_value,
//         all_config,
//     } = serde_json::from_str(&http_res).context("failed to decode body as JSON")?;
//     ensure!(Uuid::from_str(&guid).is_ok());
//     ensure!(
//         (42..=4242).contains(&random_in_range),
//         "{random_in_range} should have been within range from 42 to 4242 inclusive"
//     );
//     let nats_res = nats_publish_sub
//         .next()
//         .await
//         .context("failed to receive NATS response")?;
//     ensure!(nats_res.payload == http_res);
//     ensure!(nats_res.reply.as_deref() == Some("noreply"));

//     nats_requests
//         .await
//         .context("failed to await NATS request task")?
//         .context("failed to handle NATS requests")?;

//     let redis_keys: BTreeSet<String> = redis_client
//         .get_connection()
//         .context("failed to get connection")?
//         .keys("*")
//         .context("failed to list keys in Redis")?;
//     let expected_redis_keys = BTreeSet::from(["counter".into(), "result".into()]);
//     ensure!(
//         redis_keys == expected_redis_keys,
//         r#"invalid keys in Redis:
// got: {redis_keys:?}
// expected: {expected_redis_keys:?}"#
//     );

//     let redis_res = redis_client
//         .req_command(&redis::Cmd::get("counter"))
//         .context("failed to get `counter` key in Redis")?;
//     ensure!(redis_res == redis::Value::Data(b"42".to_vec()));
//     let redis_res = redis_client
//         .req_command(&redis::Cmd::get("result"))
//         .context("failed to get `result` key in Redis")?;
//     ensure!(redis_res == redis::Value::Data(http_res.into()));

//     http_server
//         .await
//         .context("failed to join HTTP server task")?;
//     Ok((config_value, all_config.into_iter().collect()))
// }

// #[tokio::test(flavor = "multi_thread")]
// async fn wasmbus() -> anyhow::Result<()> {
//     tracing_subscriber::registry()
//         .with(tracing_subscriber::fmt::layer().pretty().without_time())
//         .with(
//             tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
//                 tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
//             }),
//         )
//         .init();

//     let (
//         (ctl_nats_server, ctl_stop_nats_tx, ctl_nats_url, ctl_nats_client),
//         (rpc_nats_server, rpc_stop_nats_tx, rpc_nats_url, rpc_nats_client),
//         (component_nats_server, component_stop_nats_tx, component_nats_url, component_nats_client),
//     ) = try_join!(start_nats(), start_nats(), start_nats())?;

//     let ((component_redis_server, component_stop_redis_tx, component_redis_url),) =
//         try_join!(start_redis())?;

//     let mut component_redis_client =
//         redis::Client::open(component_redis_url.as_str()).context("failed to connect to Redis")?;

//     const TEST_LATTICE: &str = "test-lattice";

//     let cluster_key = KeyPair::new_cluster();
//     let host_key = KeyPair::new_server();

//     let cluster_key_two = KeyPair::new_cluster();
//     let host_key_two = KeyPair::new_server();

//     let base_labels = HashMap::from([
//         ("hostcore.arch".into(), ARCH.into()),
//         ("hostcore.os".into(), OS.into()),
//         ("hostcore.osfamily".into(), FAMILY.into()),
//         ("label1".into(), "value1".into()),
//         ("PATH".into(), "test-path".into()),
//     ]);

//     let cluster_key = Arc::new(cluster_key);
//     let host_key = Arc::new(host_key);
//     let (host, shutdown) = Host::new(HostConfig {
//         ctl_nats_url: ctl_nats_url.clone(),
//         rpc_nats_url: rpc_nats_url.clone(),
//         lattice: TEST_LATTICE.to_string(),
//         js_domain: None,
//         labels: HashMap::from([
//             ("label1".into(), "value1".into()),
//             ("PATH".into(), "test-path".into()),
//         ]),
//         cluster_key: Some(Arc::clone(&cluster_key)),
//         cluster_issuers: Some(vec![cluster_key.public_key(), cluster_key_two.public_key()]),
//         host_key: Some(Arc::clone(&host_key)),
//         provider_shutdown_delay: Some(Duration::from_millis(300)),
//         allow_file_load: true,
//         ..Default::default()
//     })
//     .await
//     .context("failed to initialize host")?;

//     let cluster_key_two = Arc::new(cluster_key_two);
//     let host_key_two = Arc::new(host_key_two);
//     let (host_two, shutdown_two) = Host::new(HostConfig {
//         ctl_nats_url: ctl_nats_url.clone(),
//         rpc_nats_url: rpc_nats_url.clone(),
//         lattice: TEST_LATTICE.to_string(),
//         labels: HashMap::from([
//             ("label1".into(), "value1".into()),
//             ("PATH".into(), "test-path".into()),
//         ]),
//         cluster_key: Some(Arc::clone(&cluster_key_two)),
//         cluster_issuers: Some(vec![cluster_key.public_key(), cluster_key_two.public_key()]),
//         host_key: Some(Arc::clone(&host_key_two)),
//         provider_shutdown_delay: Some(Duration::from_millis(400)),
//         allow_file_load: true,
//         ..Default::default()
//     })
//     .await
//     .context("failed to initialize host two")?;

//     let ctl_client = ClientBuilder::new(ctl_nats_client.clone())
//         .lattice(TEST_LATTICE.to_string())
//         .build();
//     let mut hosts = ctl_client
//         .get_hosts()
//         .await
//         .map_err(|e| anyhow!(e).context("failed to get hosts"))?;

//     ensure!(hosts.len() == 2, "Should have found 2 hosts");

//     // Put the first host as the last host in the list (so we can pop it off) if it isn't there
//     // already
//     if hosts[0].id == host_key.public_key() {
//         hosts.swap(0, 1);
//     }

//     match (hosts.pop(), hosts.pop(), hosts.as_slice()) {
//         (
//             Some(HostInfo {
//                 cluster_issuers,
//                 ctl_host,
//                 id,
//                 js_domain,
//                 labels,
//                 lattice,
//                 rpc_host,
//                 uptime_human,
//                 uptime_seconds,
//                 version,
//                 friendly_name,
//                 ..
//             }),
//             Some(HostInfo {
//                 cluster_issuers: cluster_issuers_two,
//                 ..
//             }),
//             [],
//         ) => {
//             // TODO: Validate `issuer`
//             ensure!(
//                 cluster_issuers
//                     == Some(format!(
//                         "{},{}",
//                         cluster_key.public_key(),
//                         cluster_key_two.public_key()
//                     ))
//             );
//             ensure!(cluster_issuers == cluster_issuers_two);
//             ensure!(ctl_host == Some(ctl_nats_url.to_string()));
//             ensure!(
//                 id == host_key.public_key(),
//                 "invalid host id from get hosts:\nGot: {id}\nExpected: {}",
//                 host_key.public_key()
//             );
//             ensure!(js_domain == None);
//             ensure!(
//                 labels.as_ref() == Some(&base_labels),
//                 r#"invalid labels:
// got: {labels:?}
// expected: {base_labels:?}"#
//             );
//             ensure!(lattice == Some(TEST_LATTICE.into()));
//             ensure!(rpc_host == Some(rpc_nats_url.to_string()));
//             ensure!(uptime_human.unwrap().len() > 0);
//             ensure!(uptime_seconds >= 0);
//             ensure!(version == Some(env!("CARGO_PKG_VERSION").into()));
//             ensure!(!friendly_name.is_empty());
//         }
//         (_, _, []) => bail!("not enough hosts in the lattice"),
//         _ => bail!("more than two hosts in the lattice"),
//     }

//     let (component_actor, foobar_actor) = try_join!(
//         fs::read(test_actors::RUST_BUILTINS_COMPONENT_REACTOR_PREVIEW2_SIGNED),
//         fs::read(test_actors::RUST_FOOBAR_COMPONENT_COMMAND_PREVIEW2_SIGNED),
//     )
//     .context("failed to read actors")?;

//     let jwt::Token {
//         claims: component_actor_claims,
//         ..
//     } = extract_claims(component_actor)
//         .context("failed to extract component actor claims")?
//         .context("component actor claims missing")?;

//     let jwt::Token {
//         claims: foobar_actor_claims,
//         ..
//     } = extract_claims(foobar_actor)
//         .context("failed to extract foobar actor claims")?
//         .context("foobar actor claims missing")?;

//     let component_actor_url =
//         Url::from_file_path(test_actors::RUST_BUILTINS_COMPONENT_REACTOR_PREVIEW2_SIGNED)
//             .expect("failed to construct component actor ref");
//     let foobar_actor_url =
//         Url::from_file_path(test_actors::RUST_FOOBAR_COMPONENT_COMMAND_PREVIEW2_SIGNED)
//             .expect("failed to construct foobar actor ref");

//     let mut ack = ctl_client
//         .perform_actor_auction(foobar_actor_url.as_str(), HashMap::default())
//         .await
//         .map_err(|e| anyhow!(e).context("failed to perform actor auction"))?;
//     ensure!(ack.len() == 2, "Should have received 2 acks");

//     // Put the first host as the last host in the list (so we can pop it off) if it isn't there
//     // already
//     if ack[0].host_id == host_key.public_key() {
//         ack.swap(0, 1);
//     }

//     match (ack.pop(), ack.pop(), ack.as_slice()) {
//         (
//             Some(ActorAuctionAck {
//                 actor_ref,
//                 host_id,
//                 constraints,
//             }),
//             Some(ActorAuctionAck {
//                 actor_ref: actor_ref_two,
//                 host_id: host_id_two,
//                 constraints: constraints_two,
//             }),
//             [],
//         ) => {
//             ensure!(
//                 host_id == host_key.public_key(),
//                 "invalid host id from actor auction:\nGot: {host_id}\nExpected: {}",
//                 host_key.public_key()
//             );
//             ensure!(actor_ref == foobar_actor_url.as_str());
//             ensure!(constraints.is_empty());

//             ensure!(
//                 host_id_two == host_key_two.public_key(),
//                 "invalid second host id from actor auction:\nGot: {host_id_two}\nExpected: {}",
//                 host_key_two.public_key()
//             );
//             ensure!(actor_ref_two == foobar_actor_url.as_str());
//             ensure!(constraints_two.is_empty());
//         }
//         (_, _, []) => bail!("not enough actor auction acks received"),
//         _ => bail!("more than two actor auction acks received"),
//     }

//     try_join!(
//         assert_start_actor(
//             &ctl_client,
//             &ctl_nats_client,
//             TEST_LATTICE,
//             &host_key,
//             &component_actor_url,
//             1,
//         ),
//         assert_start_actor(
//             &ctl_client,
//             &ctl_nats_client,
//             TEST_LATTICE,
//             &host_key,
//             &foobar_actor_url,
//             1,
//         )
//     )
//     .context("failed to start actors")?;

//     let blobstore_fs_provider_key = KeyPair::from_seed(test_providers::RUST_BLOBSTORE_FS_SUBJECT)
//         .context("failed to parse `rust-blobstore-fs` provider key")?;
//     let blobstore_fs_provider_url = Url::from_file_path(test_providers::RUST_BLOBSTORE_FS)
//         .expect("failed to construct provider ref");

//     let httpclient_provider_key = KeyPair::from_seed(test_providers::RUST_HTTPCLIENT_SUBJECT)
//         .context("failed to parse `rust-httpclient` provider key")?;
//     let httpclient_provider_url = Url::from_file_path(test_providers::RUST_HTTPCLIENT)
//         .expect("failed to construct provider ref");

//     let httpserver_provider_key = KeyPair::from_seed(test_providers::RUST_HTTPSERVER_SUBJECT)
//         .context("failed to parse `rust-httpserver` provider key")?;
//     let httpserver_provider_url = Url::from_file_path(test_providers::RUST_HTTPSERVER)
//         .expect("failed to construct provider ref");

//     let kvredis_provider_key = KeyPair::from_seed(test_providers::RUST_KVREDIS_SUBJECT)
//         .context("failed to parse `rust-kvredis` provider key")?;
//     let kvredis_provider_url = Url::from_file_path(test_providers::RUST_KVREDIS)
//         .expect("failed to construct provider ref");

//     let nats_provider_key = KeyPair::from_seed(test_providers::RUST_NATS_SUBJECT)
//         .context("failed to parse `rust-nats` provider key")?;
//     let nats_provider_url =
//         Url::from_file_path(test_providers::RUST_NATS).expect("failed to construct provider ref");

//     let mut ack = ctl_client
//         .perform_provider_auction(
//             httpserver_provider_url.as_str(),
//             "httpserver",
//             HashMap::default(),
//         )
//         .await
//         .map_err(|e| anyhow!(e).context("failed to perform provider auction"))?;
//     ensure!(ack.len() == 2, "Should have received 2 acks");

//     // Put the first host as the last host in the list (so we can pop it off) if it isn't there
//     // already
//     if ack[0].host_id == host_key.public_key() {
//         ack.swap(0, 1);
//     }

//     match (ack.pop(), ack.pop(), ack.as_slice()) {
//         (
//             Some(ProviderAuctionAck {
//                 provider_ref,
//                 host_id,
//                 link_name,
//                 ..
//             }),
//             Some(ProviderAuctionAck {
//                 provider_ref: provider_ref_two,
//                 host_id: host_id_two,
//                 link_name: link_name_two,
//                 ..
//             }),
//             [],
//         ) => {
//             // TODO: Validate `constraints`
//             ensure!(
//                 host_id == host_key.public_key(),
//                 "invalid host id from provider auction:\nGot: {host_id}\nExpected: {}",
//                 host_key.public_key()
//             );
//             ensure!(provider_ref == httpserver_provider_url.as_str());
//             ensure!(link_name == "httpserver");

//             ensure!(
//                 host_id_two == host_key_two.public_key(),
//                 "invalid second host id from provider auction:\nGot: {host_id_two}\nExpected: {}",
//                 host_key_two.public_key()
//             );
//             ensure!(provider_ref_two == httpserver_provider_url.as_str());
//             ensure!(link_name_two == "httpserver");
//         }
//         (_, _, []) => bail!("not enough provider auction acks received"),
//         _ => bail!("more than two provider auction acks received"),
//     }

//     let component_http_port = free_port().await?;

//     let component_blobstore_dir = tempdir()?;
//     // NOTE: Links are advertised before the provider is started to prevent race condition, which
//     // occurs if link is established after the providers starts, but before it subscribes to NATS
//     // topics
//     try_join!(
//         assert_advertise_link(
//             &ctl_client,
//             &component_actor_claims,
//             &blobstore_fs_provider_key,
//             "wasmcloud:blobstore",
//             "blobstore",
//             HashMap::from([(
//                 "ROOT".into(),
//                 component_blobstore_dir.path().to_string_lossy().into(),
//             )]),
//         ),
//         assert_advertise_link(
//             &ctl_client,
//             &component_actor_claims,
//             &httpclient_provider_key,
//             "wasmcloud:httpclient",
//             "httpclient",
//             HashMap::default(),
//         ),
//         assert_advertise_link(
//             &ctl_client,
//             &component_actor_claims,
//             &httpserver_provider_key,
//             "wasmcloud:httpserver",
//             "httpserver",
//             HashMap::from([(
//                 "config_json".into(),
//                 format!(
//                     r#"{{"address":"[{}]:{component_http_port}"}}"#,
//                     Ipv6Addr::UNSPECIFIED
//                 )
//             )]),
//         ),
//         assert_advertise_link(
//             &ctl_client,
//             &component_actor_claims,
//             &kvredis_provider_key,
//             "wasmcloud:keyvalue",
//             "keyvalue",
//             HashMap::from([("URL".into(), format!("{component_redis_url}"))]),
//         ),
//         assert_advertise_link(
//             &ctl_client,
//             &component_actor_claims,
//             &nats_provider_key,
//             "wasmcloud:messaging",
//             "messaging",
//             HashMap::from([(
//                 "config_json".into(),
//                 format!(r#"{{"cluster_uris":["{component_nats_url}"]}}"#)
//             )]),
//         ),
//     )
//     .context("failed to advertise links")?;

//     try_join!(
//         assert_config_put(
//             &ctl_client,
//             &component_actor_claims,
//             "test-config-data",
//             "test-config-value"
//         ),
//         assert_config_put(
//             &ctl_client,
//             &component_actor_claims,
//             "test-config-data2",
//             "test-config-value2"
//         )
//     )
//     .context("failed to put config")?;

//     try_join!(
//         assert_start_provider(
//             &ctl_client,
//             &rpc_nats_client,
//             TEST_LATTICE,
//             &host_key,
//             &blobstore_fs_provider_key,
//             "blobstore",
//             &blobstore_fs_provider_url,
//             None,
//         ),
//         assert_start_provider(
//             &ctl_client,
//             &rpc_nats_client,
//             TEST_LATTICE,
//             &host_key,
//             &httpclient_provider_key,
//             "httpclient",
//             &httpclient_provider_url,
//             None,
//         ),
//         assert_start_provider(
//             &ctl_client,
//             &rpc_nats_client,
//             TEST_LATTICE,
//             &host_key,
//             &httpserver_provider_key,
//             "httpserver",
//             &httpserver_provider_url,
//             None,
//         ),
//         assert_start_provider(
//             &ctl_client,
//             &rpc_nats_client,
//             TEST_LATTICE,
//             &host_key,
//             &kvredis_provider_key,
//             "keyvalue",
//             &kvredis_provider_url,
//             None,
//         ),
//         assert_start_provider(
//             &ctl_client,
//             &rpc_nats_client,
//             TEST_LATTICE,
//             &host_key_two,
//             &nats_provider_key,
//             "messaging",
//             &nats_provider_url,
//             None,
//         )
//     )
//     .context("failed to start providers")?;

//     let ctl_client = ClientBuilder::new(ctl_nats_client.clone())
//         .lattice(TEST_LATTICE.to_string())
//         .build();

//     let mut claims_from_bucket = ctl_client
//         .get_claims()
//         .await
//         .map_err(|e| anyhow!(e).context("failed to query claims via bucket"))?;
//     claims_from_bucket.sort_by(|a, b| a.get("sub").unwrap().cmp(b.get("sub").unwrap()));
//     ensure!(claims_from_bucket.len() == 8); // 5 providers, 3 actors

//     let mut links_from_bucket = ctl_client
//         .query_links()
//         .await
//         .map_err(|e| anyhow!(e).context("failed to query links via bucket"))?;
//     links_from_bucket.sort_by(|a, b| match a.actor_id.cmp(&b.actor_id) {
//         std::cmp::Ordering::Equal => match a.provider_id.cmp(&b.provider_id) {
//             std::cmp::Ordering::Equal => a.link_name.cmp(&b.link_name),
//             unequal => unequal,
//         },
//         unequal => unequal,
//     });
//     ensure!(links_from_bucket.len() == 10);

//     let pinged_hosts = ctl_client
//         .get_hosts()
//         .await
//         .map_err(|e| anyhow!(e).context("failed to ping hosts"))?;

//     ensure!(pinged_hosts.len() == 2);

//     let pinged_host = &pinged_hosts[0];

//     ensure!(
//         pinged_host.cluster_issuers
//             == Some([cluster_key.public_key(), cluster_key_two.public_key()].join(","))
//     );
//     ensure!(pinged_host.ctl_host == Some(ctl_nats_url.to_string()));
//     ensure!(pinged_host.js_domain == None);
//     ensure!(pinged_host.labels == Some(base_labels.clone()));
//     ensure!(pinged_host.lattice == Some(TEST_LATTICE.into()));
//     ensure!(pinged_host.rpc_host == Some(rpc_nats_url.to_string()));
//     ensure!(pinged_host.uptime_human.clone().unwrap().len() > 0);
//     ensure!(pinged_host.uptime_seconds > 0);
//     ensure!(pinged_host.version == Some(env!("CARGO_PKG_VERSION").into()));

//     let host_id = host_key.public_key();
//     let host_id_two = host_key_two.public_key();
//     try_join!(
//         assert_put_label(&ctl_client, &host_id, "my-name-is", "chka-chka"),
//         assert_put_label(&ctl_client, &host_id_two, "my-name-is", "Slim Shady")
//     )
//     .context("failed to put labels")?;

//     let expected_labels: HashMap<String, String> = base_labels
//         .clone()
//         .into_iter()
//         .chain([("my-name-is".into(), "chka-chka".into())])
//         .collect();

//     let expected_labels_two: HashMap<String, String> = base_labels
//         .clone()
//         .into_iter()
//         .chain([("my-name-is".into(), "Slim Shady".into())])
//         .collect();

//     let HostInventory {
//         mut actors,
//         host_id,
//         labels,
//         mut providers,
//         issuer,
//         friendly_name,
//         version,
//         uptime_human,
//         uptime_seconds,
//     } = ctl_client
//         .get_host_inventory(&host_key.public_key())
//         .await
//         .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;
//     ensure!(friendly_name != ""); // TODO: Make sure it's actually friendly?
//     ensure!(
//         host_id == host_key.public_key(),
//         "invalid host id from inventory:\nGot: {host_id}\nExpected: {}",
//         host_key.public_key()
//     );
//     ensure!(issuer == cluster_key.public_key());
//     ensure!(
//         labels == expected_labels,
//         r#"invalid labels:
// got: {labels:?}
// expected: {expected_labels:?}"#
//     );
//     ensure!(version == env!("CARGO_PKG_VERSION"), "invalid version");
//     ensure!(!uptime_human.is_empty());
//     ensure!(uptime_seconds > 0);
//     actors.sort_by(|a, b| b.name.cmp(&a.name));
//     match (actors.pop(), actors.pop(), actors.pop(), actors.as_slice()) {
//         (
//             Some(ActorDescription {
//                 id: component_id,
//                 image_ref: component_image_ref,
//                 instances: mut component_instances,
//                 name: component_name,
//             }),
//             Some(ActorDescription {
//                 id: foobar_id,
//                 image_ref: foobar_image_ref,
//                 instances: mut foobar_instances,
//                 name: foobar_name,
//             }),
//             [],
//         ) => {
//             // TODO: Validate `constraints`
//             ensure!(component_id == component_actor_claims.subject);
//             let jwt::Actor {
//                 name: expected_name,
//                 rev: expected_revision,
//                 ..
//             } = component_actor_claims
//                 .metadata
//                 .as_ref()
//                 .context("missing component actor metadata")?;
//             ensure!(component_image_ref == Some(component_actor_url.to_string()));
//             ensure!(
//                 component_name == *expected_name,
//                 r#"invalid component actor name:
// got: {component_name:?}
// expected: {expected_name:?}"#
//             );
//             let ActorInstance {
//                 annotations,
//                 instance_id,
//                 revision,
//                 image_ref,
//                 max_instances,
//             } = component_instances
//                 .pop()
//                 .context("no component actor instances found")?;
//             ensure!(
//                 component_instances.is_empty(),
//                 "more than one component actor instance found"
//             );
//             ensure!(annotations == Some(HashMap::default()));
//             ensure!(Uuid::parse_str(&instance_id).is_ok());
//             ensure!(revision == expected_revision.unwrap_or_default());
//             ensure!(image_ref == component_image_ref);
//             ensure!(max_instances == 1);

//             // TODO: Validate `constraints`
//             ensure!(foobar_id == foobar_actor_claims.subject);
//             let jwt::Actor {
//                 name: expected_name,
//                 rev: expected_revision,
//                 ..
//             } = foobar_actor_claims
//                 .metadata
//                 .as_ref()
//                 .context("missing foobar actor metadata")?;
//             ensure!(foobar_image_ref == Some(foobar_actor_url.to_string()));
//             ensure!(
//                 foobar_name == *expected_name,
//                 r#"invalid foobar actor name:
// got: {foobar_name:?}
// expected: {expected_name:?}"#
//             );
//             let ActorInstance {
//                 annotations,
//                 instance_id,
//                 revision,
//                 image_ref,
//                 max_instances,
//             } = foobar_instances
//                 .pop()
//                 .context("no foobar actor instances found")?;
//             ensure!(
//                 foobar_instances.is_empty(),
//                 "more than one foobar actor instance found"
//             );
//             ensure!(annotations == Some(HashMap::default()));
//             ensure!(Uuid::parse_str(&instance_id).is_ok());
//             ensure!(revision == expected_revision.unwrap_or_default());
//             ensure!(image_ref == foobar_image_ref);
//             ensure!(max_instances == 1);
//         }
//         (None, None, None, []) => bail!("no actor found"),
//         _ => bail!("more than 3 actors found"),
//     }
//     providers.sort_unstable_by(|a, b| b.name.cmp(&a.name));
//     match (
//         providers.pop(),
//         providers.pop(),
//         providers.pop(),
//         providers.pop(),
//         providers.as_slice(),
//     ) {
//         (Some(blobstore_fs), Some(httpclient), Some(httpserver), Some(kvredis), []) => {
//             // TODO: Validate `constraints`
//             ensure!(blobstore_fs.annotations == Some(HashMap::new()));
//             ensure!(blobstore_fs.id == blobstore_fs_provider_key.public_key());
//             ensure!(blobstore_fs.image_ref == Some(blobstore_fs_provider_url.to_string()));
//             ensure!(blobstore_fs.contract_id == "wasmcloud:blobstore");
//             ensure!(blobstore_fs.link_name == "blobstore");
//             ensure!(blobstore_fs.name.as_deref() == Some("wasmcloud-provider-blobstore-fs"));
//             ensure!(blobstore_fs.revision == 0);

//             // TODO: Validate `constraints`
//             ensure!(httpclient.annotations == Some(HashMap::new()));
//             ensure!(httpclient.id == httpclient_provider_key.public_key());
//             ensure!(httpclient.image_ref == Some(httpclient_provider_url.to_string()));
//             ensure!(httpclient.contract_id == "wasmcloud:httpclient");
//             ensure!(httpclient.link_name == "httpclient");
//             ensure!(httpclient.name.as_deref() == Some("wasmcloud-provider-httpclient"));
//             ensure!(httpclient.revision == 0);

//             // TODO: Validate `constraints`
//             ensure!(httpserver.annotations == Some(HashMap::new()));
//             ensure!(httpserver.id == httpserver_provider_key.public_key());
//             ensure!(httpserver.image_ref == Some(httpserver_provider_url.to_string()));
//             ensure!(httpserver.contract_id == "wasmcloud:httpserver");
//             ensure!(httpserver.link_name == "httpserver");
//             ensure!(httpserver.name.as_deref() == Some("wasmcloud-provider-httpserver"));
//             ensure!(httpserver.revision == 0);

//             // TODO: Validate `constraints`
//             ensure!(kvredis.annotations == Some(HashMap::new()));
//             ensure!(kvredis.id == kvredis_provider_key.public_key());
//             ensure!(kvredis.image_ref == Some(kvredis_provider_url.to_string()));
//             ensure!(kvredis.contract_id == "wasmcloud:keyvalue");
//             ensure!(kvredis.link_name == "keyvalue");
//             ensure!(kvredis.name.as_deref() == Some("wasmcloud-provider-kvredis"));
//             ensure!(kvredis.revision == 0);
//         }
//         _ => bail!("invalid provider count"),
//     }

//     let HostInventory {
//         actors,
//         host_id,
//         labels,
//         mut providers,
//         issuer,
//         friendly_name,
//         version,
//         uptime_human,
//         uptime_seconds,
//     } = ctl_client
//         .get_host_inventory(&host_key_two.public_key())
//         .await
//         .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;
//     ensure!(friendly_name != ""); // TODO: Make sure it's actually friendly?
//     ensure!(
//         host_id == host_key_two.public_key(),
//         "invalid second host id from inventory:\nGot: {host_id}\nExpected: {}",
//         host_key_two.public_key()
//     );
//     ensure!(issuer == cluster_key_two.public_key());
//     ensure!(
//         labels == expected_labels_two,
//         r#"invalid labels:
// got: {labels:?}
// expected: {expected_labels_two:?}"#
//     );
//     ensure!(actors.is_empty());
//     ensure!(version == env!("CARGO_PKG_VERSION"), "invalid version");
//     ensure!(!uptime_human.is_empty());
//     ensure!(uptime_seconds > 0);

//     match (providers.pop(), providers.as_slice()) {
//         (Some(nats), []) => {
//             // TODO: Validate `constraints`
//             ensure!(nats.annotations == Some(HashMap::new()));
//             ensure!(nats.id == nats_provider_key.public_key());
//             ensure!(nats.image_ref == Some(nats_provider_url.to_string()));
//             ensure!(nats.contract_id == "wasmcloud:messaging");
//             ensure!(nats.link_name == "messaging");
//             ensure!(nats.name.as_deref() == Some("wasmcloud-provider-nats"));
//             ensure!(nats.revision == 0);
//         }
//         _ => bail!("invalid provider count"),
//     }

//     try_join!(async {
//         let (config_value, all_config) = assert_handle_http_request(
//             component_http_port,
//             component_nats_client.clone(),
//             &mut component_redis_client,
//         )
//         .await
//         .context("component actor test failed")?;
//         ensure!(
//             config_value == b"test-config-value",
//             "should have returned the correct config value"
//         );
//         let expected = HashMap::from([
//             ("test-config-data".into(), b"test-config-value".to_vec()),
//             ("test-config-data2".into(), b"test-config-value2".to_vec()),
//         ]);
//         if all_config == expected {
//             Ok(())
//         } else {
//             Err(anyhow!("should have returned all config values.\nExpected: {expected:?}\nGot: {all_config:?}"))
//         }
//     },)?;

//     try_join!(
//         assert_remove_link(
//             &ctl_client,
//             &component_actor_claims,
//             "wasmcloud:blobstore",
//             "blobstore"
//         ),
//         assert_remove_link(
//             &ctl_client,
//             &component_actor_claims,
//             "wasmcloud:httpserver",
//             "httpserver"
//         ),
//         assert_remove_link(
//             &ctl_client,
//             &component_actor_claims,
//             "wasmcloud:keyvalue",
//             "keyvalue"
//         ),
//         assert_remove_link(
//             &ctl_client,
//             &component_actor_claims,
//             "wasmcloud:messaging",
//             "messaging",
//         ),
//     )
//     .context("failed to remove links")?;

//     // Test specific scale annotation logic
//     assert_scale_actor(
//         &ctl_client,
//         &ctl_nats_client,
//         TEST_LATTICE,
//         &host_key,
//         &foobar_actor_url,
//         Some(HashMap::from_iter([("foo".to_string(), "bar".to_string())])),
//         5,
//     )
//     .await
//     .context("failed to scale foobar actor")?;
//     tokio::time::sleep(std::time::Duration::from_secs(5)).await;
//     let HostInventory { actors, .. } = ctl_client
//         .get_host_inventory(&host_key.public_key())
//         .await
//         .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;
//     let foobar_actor = actors
//         .iter()
//         .find(|a| a.image_ref == Some(foobar_actor_url.to_string()))
//         .expect("foobar actor to be in the list");
//     // 1 with no annotations, 1 with annotations (max scale 5)
//     ensure!(foobar_actor.instances.len() == 2);
//     assert_scale_actor(
//         &ctl_client,
//         &ctl_nats_client,
//         TEST_LATTICE,
//         &host_key,
//         &foobar_actor_url,
//         Some(HashMap::from_iter([("foo".to_string(), "bar".to_string())])),
//         u32::MAX,
//     )
//     .await
//     .context("failed to scale foobar actor")?;
//     let HostInventory { actors, .. } = ctl_client
//         .get_host_inventory(&host_key.public_key())
//         .await
//         .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;
//     let foobar_actor = actors
//         .iter()
//         .find(|a| a.image_ref == Some(foobar_actor_url.to_string()))
//         .expect("foobar actor to be in the list");
//     // 1 with no annotations, 1 with annotations (with unbounded scale)
//     ensure!(foobar_actor.instances.len() == 2);

//     assert_scale_actor(
//         &ctl_client,
//         &ctl_nats_client,
//         TEST_LATTICE,
//         &host_key,
//         &foobar_actor_url,
//         Some(HashMap::from_iter([("foo".to_string(), "bar".to_string())])),
//         0,
//     )
//     .await
//     .context("failed to scale foobar actor")?;
//     let HostInventory { actors, .. } = ctl_client
//         .get_host_inventory(&host_key.public_key())
//         .await
//         .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;
//     let foobar_actor = actors
//         .iter()
//         .find(|a| a.image_ref == Some(foobar_actor_url.to_string()))
//         .expect("foobar actor to be in the list");
//     // 1 with no annotations, 0 with annotations
//     ensure!(foobar_actor.instances.len() == 1);

//     let host_id = host_key.public_key();
//     let host_id_two = host_key_two.public_key();
//     try_join!(
//         assert_delete_label(&ctl_client, &host_id, "my-name-is"),
//         assert_delete_label(&ctl_client, &host_id_two, "my-name-is")
//     )
//     .context("failed to remove labels")?;

//     let HostInventory { labels, .. } = ctl_client
//         .get_host_inventory(&host_key.public_key())
//         .await
//         .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;

//     ensure!(
//         labels == base_labels,
//         r#"invalid labels:
// got: {labels:?}
// expected: {base_labels:?}"#
//     );

//     let HostInventory { labels, .. } = ctl_client
//         .get_host_inventory(&host_key_two.public_key())
//         .await
//         .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;

//     ensure!(
//         labels == base_labels,
//         r#"invalid labels:
//     got: {labels:?}
//     expected: {base_labels:?}"#
//     );

//     // Shutdown host one
//     let CtlOperationAck { accepted, error } = ctl_client
//         .stop_host(&host_key.public_key(), None)
//         .await
//         .map_err(|e| anyhow!(e).context("failed to stop host"))?;
//     ensure!(error == "");
//     ensure!(accepted);

//     let _ = host.stopped().await;
//     shutdown.await.context("failed to shutdown host")?;

//     // Shutdown host two
//     let CtlOperationAck { accepted, error } = ctl_client
//         .stop_host(&host_key_two.public_key(), None)
//         .await
//         .map_err(|e| anyhow!(e).context("failed to stop host"))?;
//     ensure!(error == "");
//     ensure!(accepted);

//     let _ = host_two.stopped().await;
//     shutdown_two.await.context("failed to shutdown host")?;

//     try_join!(
//         stop_server(ctl_nats_server, ctl_stop_nats_tx),
//         stop_server(rpc_nats_server, rpc_stop_nats_tx),
//         stop_server(component_nats_server, component_stop_nats_tx),
//         stop_server(component_redis_server, component_stop_redis_tx),
//     )
//     .context("failed to stop servers")?;

//     Ok(())
// }

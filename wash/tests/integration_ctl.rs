mod common;
use common::{output_to_string, wash, Result};
use wasmcloud_host::HostBuilder;

#[actix_rt::test]
async fn integration_ctl_get_comprehensive() -> Result<()> {
    const NS: &str = "get_comprehensive";
    // Without hosts running, should be an empty list
    let ctl_get_hosts_empty = wash()
        .args(&["ctl", "get", "hosts", "-o", "json", "-n", NS])
        .output()
        .expect("failed to get hosts from ctl get hosts");
    assert!(ctl_get_hosts_empty.status.success());
    assert_eq!(output_to_string(ctl_get_hosts_empty), "{\"hosts\":[]}\n");

    // Start a host, ensure it is returned in the get hosts output
    let host_id = create_host(NS.to_string()).await?;
    let ctl_get_hosts = wash()
        .args(&[
            "ctl",
            "get",
            "hosts",
            "-o",
            "json",
            "-n",
            NS,
            "--timeout",
            "15",
        ])
        .output()
        .expect("failed to get hosts from ctl get hosts");
    assert!(ctl_get_hosts.status.success());
    // Used `starts_with` and `ends_with` here as we can't test for exact uptime seconds
    let output = output_to_string(ctl_get_hosts);
    assert!(output.starts_with(&format!(
        "{{\"hosts\":[{{\"id\":\"{}\",\"uptime\":",
        host_id
    )));
    assert!(output.ends_with("}]}\n"));

    let ctl_get_inventory = wash()
        .args(&["ctl", "get", "inventory", &host_id, "-o", "json", "-n", NS])
        .output()
        .expect("failed to get host inventory with ctl get inventory");

    assert!(ctl_get_inventory.status.success());
    let output = output_to_string(ctl_get_inventory);
    // Ensure all appropriate sections are there and host id is correct
    // We could ensure that the extras and wasmcloud_lattice_cache providers
    // are present, but that's not a `wash`'s responsibility.
    assert!(output.contains("\"inventory\":"));
    assert!(output.contains("\"actors\":[]"));
    assert!(output.contains(&format!("\"host_id\":\"{}\"", host_id)));
    assert!(output.contains("\"test_mode\":\"true\""));
    assert!(output.contains("\"providers\":[{"));

    let ctl_get_claims = wash()
        .args(&["ctl", "get", "claims", "-n", NS, "-o", "json"])
        .output()
        .expect("failed to get claims with ctl get claims");
    assert!(ctl_get_claims.status.success());
    let output = output_to_string(ctl_get_claims);
    assert_eq!(output, "{\"claims\":{\"claims\":[]}}\n");

    Ok(())
}

#[actix_rt::test]
/// Tests starting, calling, and stopping an actor
async fn integration_ctl_actor_roundtrip() -> Result<()> {
    const ECHO: &str = "wasmcloud.azurecr.io/echo:0.2.0";
    const ECHO_PKEY: &str = "MBCFOPM6JW2APJLXJD3Z5O4CN7CPYJ2B4FTKLJUR5YR5MITIU7HD3WD5";
    const NS: &str = "start_stop_roundtrip";
    let host_id = create_host(NS.to_string()).await?;

    let start_echo = wash()
        .args(&["ctl", "start", "actor", ECHO, "-h", &host_id, "-n", NS])
        .output()
        .expect("failed to get start actor acknowledgement");
    assert!(start_echo.status.success());

    assert!(wait_for_start(&host_id, NS, ECHO_PKEY, 30).await);

    // Should fail, can't have two instances of the same actor in a single host
    let start_echo_again = wash()
        .args(&[
            "ctl", "start", "actor", ECHO, "-h", &host_id, "-n", NS, "-o", "json",
        ])
        .output()
        .expect("failed to get start actor acknowledgement");
    let failed_echo = output_to_string(start_echo_again);
    assert!(failed_echo.contains(&format!("\"actor_ref\":\"{}\"", ECHO)));
    assert!(failed_echo.contains(&format!(
        "\"failure\":\"Actor with image ref \'{}\' is already running on this host\"",
        ECHO
    )));
    assert!(failed_echo.contains(&format!("\"host_id\":\"{}\"", host_id)));

    let payload = "{\"method\": \"GET\", \"path\": \"/echo\", \"body\": \"\", \"queryString\":\"\", \"header\":{}}";
    let call_echo = wash()
        .args(&[
            "ctl",
            "call",
            ECHO_PKEY,
            "HandleRequest",
            payload,
            "-n",
            NS,
            "-o",
            "json",
        ])
        .output()
        .expect("failed to call echo actor");
    assert!(call_echo.status.success());
    // The glyphs and excessive escapes are because of the messagepack raw deserialization
    // See https://github.com/wasmcloud/wash/issues/32 for more information
    let call_response = "{\"response\":\"��statusCode�Ȧstatus�OK�header��body�H{\\\"method\\\":\\\"GET\\\",\\\"path\\\":\\\"/echo\\\",\\\"query_string\\\":\\\"\\\",\\\"headers\\\":{},\\\"body\\\":[]}\"}\n";
    assert_eq!(output_to_string(call_echo), call_response);

    let stop_actor = wash()
        .args(&[
            "ctl", "stop", "actor", &host_id, ECHO_PKEY, "-n", NS, "-o", "json",
        ])
        .output()
        .expect("failed to stop actor");
    assert!(stop_actor.status.success());

    assert!(wait_for_stop(&host_id, NS, ECHO_PKEY, 30).await);

    let stop_actor_fail = wash()
        .args(&[
            "ctl", "stop", "actor", &host_id, ECHO_PKEY, "-n", NS, "-o", "json",
        ])
        .output()
        .expect("failed to stop actor");
    assert_eq!(
        output_to_string(stop_actor_fail),
        "{\"error\":\"Actor is either not running on this host or host controller unresponsive\"}\n"
    );

    // Calling a stopped actor should fail
    let call_echo_fail = wash()
        .args(&[
            "ctl",
            "call",
            ECHO_PKEY,
            "HandleRequest",
            payload,
            "-n",
            NS,
            "-o",
            "json",
        ])
        .output()
        .expect("failed to call echo actor");
    let success = call_echo_fail.status.success();
    assert!(!success);
    // The glyphs and excessive escapes are because of the messagepack raw deserialization
    // See https://github.com/wasmcloud/wash/issues/32 for more information
    assert_eq!(output_to_string(call_echo_fail), "");

    Ok(())
}

#[actix_rt::test]
/// Tests starting an actor and provider, linking them, and using
/// an HTTP client library to make a request to the actor
async fn integration_ctl_actor_provider_roundtrip() -> Result<()> {
    const ECHO: &str = "wasmcloud.azurecr.io/echo:0.2.0";
    const ECHO_PKEY: &str = "MBCFOPM6JW2APJLXJD3Z5O4CN7CPYJ2B4FTKLJUR5YR5MITIU7HD3WD5";
    const HTTPSERVER: &str = "wasmcloud.azurecr.io/httpserver:0.11.1";
    const HTTPSERVER_PKEY: &str = "VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M";
    const CONTRACT: &str = "wasmcloud:httpserver";
    const NS: &str = "actor_provider_roundtrip";
    let host_id = create_host(NS.to_string()).await?;

    let start_echo = wash()
        .args(&["ctl", "start", "actor", ECHO, "-h", &host_id, "-n", NS])
        .output()
        .expect("failed to get start actor acknowledgement");
    assert!(start_echo.status.success());

    let start_httpserver = wash()
        .args(&[
            "ctl", "start", "provider", HTTPSERVER, "-h", &host_id, "-n", NS,
        ])
        .output()
        .expect("failed to get start actor acknowledgement");
    assert!(start_httpserver.status.success());

    assert!(wait_for_start(&host_id, NS, ECHO_PKEY, 30).await);
    assert!(wait_for_start(&host_id, NS, HTTPSERVER_PKEY, 30).await);

    // Should fail, can't have two instances of the same provider in a single host
    let start_httpserver_again = wash()
        .args(&[
            "ctl", "start", "provider", HTTPSERVER, "-h", &host_id, "-n", NS, "-o", "json",
        ])
        .output()
        .expect("failed to get start actor acknowledgement");
    let failed_httpserver = output_to_string(start_httpserver_again);
    assert!(failed_httpserver.contains(&format!(
        "\"failure\":\"Provider with image ref \'{}\' is already running on this host.\"",
        HTTPSERVER
    )));
    assert!(failed_httpserver.contains(&format!("\"host_id\":\"{}\"", host_id)));
    // TODO: this should be tested, but is a bug as of wasmcloud-host 0.15.1.
    // Once https://github.com/wasmcloud/wasmcloud/issues/106 is closed, this should be uncommented
    // assert!(failed_httpserver.contains(&format!("\"provider_ref\":\"{}\"", HTTPSERVER)));

    let link_echo_httpserver = wash()
        .args(&[
            "ctl",
            "link",
            ECHO_PKEY,
            HTTPSERVER_PKEY,
            CONTRACT,
            "PORT=8080",
            "-n",
            NS,
            "-o",
            "json",
        ])
        .output()
        .expect("failed to link echo actor and httpserver provider");
    assert!(link_echo_httpserver.status.success());
    let link_output = output_to_string(link_echo_httpserver);
    assert!(link_output.contains(&format!("\"actor_id\":\"{}\"", ECHO_PKEY)));
    assert!(link_output.contains(&format!("\"provider_id\":\"{}\"", HTTPSERVER_PKEY)));
    assert!(link_output.contains("\"result\":\"published\""));

    // Links are idempotent and can be called multiple times without failure
    for _ in 0..5 {
        let link_echo_httpserver = wash()
            .args(&[
                "ctl",
                "link",
                ECHO_PKEY,
                HTTPSERVER_PKEY,
                CONTRACT,
                "PORT=8080",
                "-n",
                NS,
                "-o",
                "json",
            ])
            .output()
            .expect("failed to link echo actor and httpserver provider");
        assert!(link_echo_httpserver.status.success());
    }

    let resp = reqwest::blocking::get("http://localhost:8080/echotest")?.text()?;
    assert!(resp.contains("\"method\":\"GET\""));
    assert!(resp.contains("\"path\":\"/echotest\""));
    assert!(resp.contains("\"query_string\":\"\""));
    assert!(resp.contains("\"host\":\"localhost:8080\""));
    assert!(resp.contains("\"body\":[]"));

    let stop_actor = wash()
        .args(&[
            "ctl", "stop", "actor", &host_id, ECHO_PKEY, "-n", NS, "-o", "json",
        ])
        .output()
        .expect("failed to stop actor");
    assert!(stop_actor.status.success());

    let stop_provider = wash()
        .args(&[
            "ctl",
            "stop",
            "provider",
            &host_id,
            HTTPSERVER_PKEY,
            "default",
            CONTRACT,
            "-n",
            NS,
            "-o",
            "json",
        ])
        .output()
        .expect("failed to stop actor");
    assert!(stop_provider.status.success());

    assert!(wait_for_stop(&host_id, NS, ECHO_PKEY, 30).await);
    assert!(wait_for_stop(&host_id, NS, HTTPSERVER_PKEY, 30).await);

    // Now that actor and provider aren't running, this request should fail
    assert!(reqwest::blocking::get("http://localhost:8080/echotest").is_err());

    Ok(())
}

//TODO: Updates with actors with different OCI references are not yet supported.
// This issue is being tracked at https://github.com/wasmcloud/wasmcloud/issues/108
// #[actix_rt::test]
// /// Tests starting and updating an actor
// async fn integration_ctl_update_actor() -> Result<()> {
//     const ECHO: &str = "wasmcloud.azurecr.io/echo:0.2.0";
//     const ECHO_NEW: &str = "wasmcloud.azurecr.io/echo:0.2.1";
//     const ECHO_PKEY: &str = "MBCFOPM6JW2APJLXJD3Z5O4CN7CPYJ2B4FTKLJUR5YR5MITIU7HD3WD5";
//     const NS: &str = "update_actor";
//     let host_id = create_host(NS.to_string()).await?;

//     let start_echo = wash()
//         .args(&["ctl", "start", "actor", ECHO, "-h", &host_id, "-n", NS])
//         .output()
//         .expect("failed to get start actor acknowledgement");
//     assert!(start_echo.status.success());

//     assert!(wait_for_start(&host_id, NS, ECHO, 30).await);

//     let update_echo = wash()
//         .args(&[
//             "ctl", "update", "actor", &host_id, ECHO_PKEY, ECHO_NEW, "-n", NS, "-o", "json",
//         ])
//         .output()
//         .expect("failed to issue update actor command");
//     assert!(update_echo.status.success());
//     assert!(wait_for_start(&host_id, NS, ECHO_NEW, 30).await);

//     Ok(())
// }

/// Helper function to initialize a host in a separate thread
/// and return its ID. We create a host in a separate thread because
/// issuing control interface commands to a host in the same thread
/// will fail, the host is unable to respond as it is "blocked" waiting
/// for the control interface command to come back.
///
/// `namespace` is used to create hosts in isolation in the lattice,
/// as we wouldn't want multiple hosts to interact between tests
async fn create_host(namespace: String) -> Result<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut rt = actix_rt::System::new("testhost");
        rt.block_on(async move {
            let nats_conn = nats::asynk::connect("0.0.0.0:4222").await.unwrap();
            let host = HostBuilder::new()
                .with_namespace(&namespace)
                .with_rpc_client(nats_conn.clone())
                .with_control_client(nats_conn)
                .with_label("test_mode", "true")
                .oci_allow_latest()
                .oci_allow_insecure(vec!["localhost:5000".to_string()])
                .enable_live_updates()
                .build();
            host.start().await.unwrap();
            tx.send(host.id()).unwrap();
            // Since CTRL+C won't be captured by this thread, host will stop when test exits
            actix_rt::signal::ctrl_c().await.unwrap();
            host.stop().await;
        });
    });
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.into())
}

/// Helper function to query host inventory for a specific resource.
/// This can be used to ensure that a resource is present in a host,
/// e.g. an actor or a provider
async fn wait_for_start(host_id: &str, namespace: &str, resource: &str, retries: u32) -> bool {
    let mut count: u32 = 0;
    while count < retries {
        let host_inv = wash()
            .args(&["ctl", "get", "inventory", host_id, "-n", namespace])
            .output()
            .expect("failed to get host inventory");
        if output_to_string(host_inv).contains(resource) {
            return true;
        } else {
            count += 1;
            actix_rt::time::delay_for(std::time::Duration::from_secs(1)).await;
        }
    }
    false
}

/// Helper function to query host inventory for a specific resource.
/// This can be used to ensure that a resource is present in a host,
/// e.g. an actor or a provider
async fn wait_for_stop(host_id: &str, namespace: &str, resource: &str, retries: u32) -> bool {
    let mut count: u32 = 0;
    while count < retries {
        let host_inv = wash()
            .args(&["ctl", "get", "inventory", host_id, "-n", namespace])
            .output()
            .expect("failed to get host inventory");
        if !output_to_string(host_inv).contains(resource) {
            return true;
        } else {
            count += 1;
            actix_rt::time::delay_for(std::time::Duration::from_secs(1)).await;
        }
    }
    false
}

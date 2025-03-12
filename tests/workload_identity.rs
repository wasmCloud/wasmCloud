#![cfg(all(unix, feature = "wasmcloud"))]
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use async_nats::service::ServiceExt;
use bytes::Bytes;
use futures::StreamExt;

mod common;
use common::{spire::start_spire_agent, tempdir};

use nats_jwt_rs::{
    account::{Account, ExternalAuthorization},
    authorization::{AuthRequest, AuthResponse},
    operator::Operator,
    types::{Permission, Permissions},
    user::User,
    Claims,
};
use nkeys::KeyPair;
use wasmcloud_host::wasmbus::connect_nats;
use wasmcloud_host::workload_identity::WorkloadIdentityConfig;
use wasmcloud_test_util::env::EnvVarGuard;
use wasmcloud_test_util::testcontainers::{
    AsyncRunner as _, ContainerAsync, ExecCommand, ImageExt, NatsConfig, NatsResolver, NatsServer,
    SpireServer,
};

#[tokio::test]
async fn connect_to_nats_with_workload_identity() -> anyhow::Result<()> {
    // Generate the NATS Server configuration that's needed for configuring decentralized Auth Callout
    let (nats_config, auth_account_nkey, auth_user_nkey) = generate_nats_config()?;
    let nats = NatsServer::default()
        .with_config(nats_config)
        .with_startup_timeout(Duration::from_secs(5))
        .start()
        .await?;
    let nats_address = &format!(
        "{}:{}",
        nats.get_host().await?,
        nats.get_host_port_ipv4(4222).await?
    );

    // Set up Sentinel credentials for code under test to use to establish the initial NATS connection
    // that'll be used to send the JWT-SVID to the Auth Callout service.
    let sentinel_kp = nkeys::KeyPair::new_user();
    let mut sentinel_claims = User::new_claims("sentinel".to_string(), sentinel_kp.public_key());
    // These permissions mimic the recommended sentinel credentials
    sentinel_claims.nats.permissions.permissions = Permissions {
        publish: Permission {
            allow: vec![],
            deny: vec![">".to_string()],
        },
        subscribe: Permission {
            allow: vec![],
            deny: vec![">".to_string()],
        },
        resp: None,
    };
    let sentinel_jwt = sentinel_claims.encode(&auth_account_nkey)?;

    let spire_server = SpireServer::default()
        .with_startup_timeout(Duration::from_secs(5))
        .start()
        .await?;
    let spire_server_url = &format!(
        "http://{}:{}",
        spire_server.get_host().await?,
        spire_server.get_host_port_ipv4(8081).await?
    );

    let agent_spiffe_id = "spiffe://example.org/test-agent";
    let workload_spiffe_id = "spiffe://example.org/test-workload";
    let auth_callout_service_audience = "spiffe://example.org/auth-callout";

    // Generate the join_token used by the SPIRE Agent to establish it's identity with the SPIRE Server
    let agent_join_token = generate_join_token(agent_spiffe_id, &spire_server)
        .await
        .context("should have generated join token for SPIRE Agent")?;

    // Temporary directory for storing all of the SPIRE Agent configuration and sockets
    let tmp_dir = tempdir().context("should have create a temporary directory for SPIRE Agent")?;

    // Start the SPIRE Agent on the local machine so that we can use the Workload API socket for connecting to NATS
    let (agent, api_socket, _) =
        start_spire_agent(&agent_join_token, spire_server_url, tmp_dir.path()).await?;

    // Set environment variables to be used in Auth Callout Service and WorkloadIdentityConfig
    let _spiffe_endpoint = EnvVarGuard::set(
        "SPIFFE_ENDPOINT_SOCKET",
        format!("unix:{}", api_socket.display()),
    );
    let _auth_audience = EnvVarGuard::set(
        "WASMCLOUD_WORKLOAD_IDENTITY_AUTH_SERVICE_AUDIENCE",
        auth_callout_service_audience,
    );

    // Start the Auth Callout Service that validates the JWT-SVIDs sent by the code under test
    let nats_server_address = nats_address.clone();
    tokio::spawn(async move {
        let _ = start_workload_identity_auth_callout(
            &nats_server_address,
            auth_account_nkey,
            auth_user_nkey,
        )
        .await;
    });

    // Read SPIRE Agent API socket metadata to get the current user id so it
    // can be used as part of the workload selector on the SPIRE Server
    let metadata = std::fs::metadata(api_socket.clone())
        .context("should have read file metadata for the SPIRE Agent Workload API socket")?;
    let workload_selector = format!("unix:uid:{}", metadata.uid());

    // Register the test workload on the SPIRE Server
    register_spiffe_workload(
        &spire_server,
        agent_spiffe_id,
        workload_spiffe_id,
        &workload_selector,
    )
    .await
    .context("should have registered workload on the SPIRE Server")?;

    // Wait for the workload registration to propagate to the agent before attempting to connect
    validate_workload_registration_within_timeout(&api_socket, Duration::from_secs(15)).await?;

    let wid_cfg =
        WorkloadIdentityConfig::from_env().expect("should initialize workload identity config");

    // Actually run the code under test
    let client = connect_nats(
        nats_address,
        Some(&sentinel_jwt),
        Some(Arc::new(sentinel_kp)),
        false,
        None,
        Some(wid_cfg),
    )
    .await?;

    // Shut down the SPIRE Agent running locally
    agent.stop().await?;

    assert_eq!(
        client.connection_state(),
        async_nats::connection::State::Connected
    );

    Ok(())
}

// Generates a join token used by the SPIRE Agent to establish it's identity
// with the SPIRE Server.
async fn generate_join_token(
    agent_spiffe_id: &str,
    spire_server: &ContainerAsync<SpireServer>,
) -> anyhow::Result<String> {
    let mut join_token_cmd = spire_server
        .exec(ExecCommand::new(vec![
            "/opt/spire/bin/spire-server",
            "token",
            "generate",
            "-spiffeID",
            agent_spiffe_id,
            "-output",
            "json",
            "-ttl",
            "3600",
        ]))
        .await
        .context("should generate join token")?;

    let join_token_response = join_token_cmd
        .stdout_to_vec()
        .await
        .context("should parse join token generate response")?;

    let join_token: serde_json::Value = serde_json::from_slice(&join_token_response)
        .context("should parse SPIRE Server join token response")?;

    Ok(join_token
        .get("value")
        .context("should find a 'value' field in the join token response")?
        .as_str()
        .context("should return join token 'value' field")?
        .to_string())
}

// Uses the SPIRE Server container to register the test workload under the
// local SPIRE Agent
async fn register_spiffe_workload(
    spire_server: &ContainerAsync<SpireServer>,
    agent_spiffe_id: &str,
    workload_spiffe_id: &str,
    workload_selector: &str,
) -> anyhow::Result<()> {
    _ = spire_server
        .exec(ExecCommand::new(vec![
            "/opt/spire/bin/spire-server",
            "entry",
            "create",
            "-parentID",
            agent_spiffe_id,
            "-spiffeID",
            workload_spiffe_id,
            "-selector",
            workload_selector,
        ]))
        .await
        .context("failed to register create SPIFFE entry for workload")?;
    Ok(())
}

// Ensure that workloads can be fetched (within provided timeout), so that
// subsequent calls to the SPIRE Agent from the test workload and Auth Callout
// service succeed
async fn validate_workload_registration_within_timeout(
    agent_socket: &Path,
    timeout: Duration,
) -> anyhow::Result<()> {
    tokio::time::timeout(timeout, async move {
        loop {
            if let Ok(status) = tokio::process::Command::new(
                std::env::var("TEST_SPIRE_AGENT_BIN")
                    .as_deref()
                    .unwrap_or("spire-agent"),
            )
            .args([
                "api",
                "fetch",
                "x509",
                "-silent",
                "-socketPath",
                &agent_socket.display().to_string(),
            ])
            .status()
            .await
            {
                if status.success() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await?;
    Ok(())
}

// A very naive Auth Callout service implementation for the purposes of
// validating that the workload under test is passing a valid JWT-SVID
async fn start_workload_identity_auth_callout(
    nats_address: &str,
    auth_account_nkey: KeyPair,
    auth_user_nkey: KeyPair,
) -> anyhow::Result<()> {
    // Create a JWT with the Auth Callout User KeyPair to connect to NATS
    let claims = User::new_claims("auth-callout".to_string(), auth_user_nkey.public_key());
    let jwt = claims.encode(&auth_account_nkey)?;
    // We have to use JWT based authentication here since we're using decentralized auth.
    let nats_client = async_nats::ConnectOptions::new()
        .jwt(jwt, move |nonce| {
            let kp = auth_user_nkey.clone();
            async move { kp.sign(&nonce).map_err(async_nats::AuthError::new) }
        })
        .name("auth-callout")
        .connect(nats_address)
        .await?;

    // Establish the SPIFFE client against the local SPIRE Agent so we can validate the incoming JWT-SVIDs
    let mut spiffe_client = spiffe::WorkloadApiClient::default()
        .await
        .context("failed to create SPIFFE client for Auth Callout")?;

    // Re-use the environment variable we set up the client-side with to match the audience we should be expecting
    // for the purposes of validating the SVID.
    let auth_callout_audience = std::env::var("WASMCLOUD_WORKLOAD_IDENTITY_AUTH_SERVICE_AUDIENCE")?;

    // NOTE: We use NATS Service API here for the convenience it provides for responding to messages,
    // this is by no means required, we could simply do nats_client.subscribe("$SYS.REQ.USER.AUTH").await
    // and handle responding to messages received on that subscription manually, but this is testing code
    // so we're trying to keep it simple.
    let service = nats_client
        .service_builder()
        .start("workload-identity-auth-callout", "0.0.0")
        .await
        .expect("failed to create workload-identity-auth-callout service");

    // Set up the endpoint for listening to messages from NATS Server on the Auth Callout subject
    let mut endpoint = service
        .endpoint("$SYS.REQ.USER.AUTH")
        .await
        .expect("failed to register auth callout endpoint");

    // Listen for messages on the Auth Callout endpoint
    while let Some(request) = endpoint.next().await {
        let payload = String::from_utf8(request.message.payload.to_vec()).unwrap();

        let auth = match Claims::<AuthRequest>::decode(&payload) {
            Ok(decoded) => decoded.payload().clone(),
            Err(err) => {
                // Nothing to do here since we presumably got an incomplete (or entirely wrong) Authorization Request
                // NOTE: normally we would just continue, but since this is for testing, panic is "fine".
                panic!("Got error while decoding Authorization Request: {err:#?}");
            }
        };

        let server_id = auth.server.id;
        let user_nkey = auth.user_nkey;

        // Attempt to validate the Auth Token from the Authorization Request against SPIRE Agent
        let svid = match spiffe_client
            .validate_jwt_token(
                &auth_callout_audience,
                &auth.connect_opts.auth_token.unwrap_or_default(),
            )
            .await
        {
            Ok(svid) => svid,
            Err(err) => {
                // Prepare an Authorization Response for the error
                let mut resp = AuthResponse::generic_claim(user_nkey);
                // Set the NATS Server we got the Authorization Request from as the recipient
                resp.aud = Some(server_id);
                // Include the error we received from attempting to validate the provided Auth Token
                resp.nats.error = err.to_string();
                // Encode the Authorization Response
                let encoded_response = resp.encode(&auth_account_nkey)?;
                request.respond(Ok(Bytes::from(encoded_response))).await?;
                continue;
            }
        };

        // Create the User Claims that place the successfully validated user into an Account for the connection
        let mut user_claims = User::new_claims(svid.spiffe_id().to_string(), user_nkey.clone());
        // Map the User to WASMCLOUD account
        user_claims.aud = Some("WASMCLOUD".to_string());
        // Turn the user claims into a JWT that'll be included in the Auth Response back to NATS Server
        let encoded_user = user_claims.encode(&auth_account_nkey)?;
        // Create an Authorization Response to be sent back to the NATS Server
        let mut auth_response = AuthResponse::generic_claim(user_nkey);
        // Set the NATS Server we got the Authorization Request from as the recipient
        auth_response.aud = Some(server_id);
        // Embed the encoded User Claims JWT in the Authorization Response
        auth_response.nats.jwt = encoded_user;
        // Encode the Authorization Response for sending back to the NATS Server
        let encoded_response = auth_response.encode(&auth_account_nkey)?;
        // Respond back to the NATS Server with the Authorization Response that includes the User Claims/JWT.
        request.respond(Ok(Bytes::from(encoded_response))).await?;
    }

    Ok(())
}

// generate_nats_config is a convenience function for generating a NATS Config that
// sets up the necessary structure for decentralized Auth Callout along with the necessary
// accounts and preloads them into an in-memory resolver.
fn generate_nats_config() -> anyhow::Result<(NatsConfig, KeyPair, KeyPair)> {
    // Set up the Operator for the purposes of signing accounts
    let operator_kp = KeyPair::new_operator();
    // Create Operator Claims for the NATS Config's "operator" field
    let mut operator = Operator::new_claims(
        "workload-identity-test".to_string(),
        operator_kp.public_key(),
    );
    // Set up SYS Account's Public Key with Operator as the System Account
    let sys_kp = KeyPair::new_account();
    operator.nats.system_account = Some(sys_kp.public_key());
    // Operator JWT for setting up trusted operators
    let operator_jwt = operator.encode(&operator_kp)?;
    // SYS Account JWT for resolver preloading
    let sys_claims = Account::new_claims("SYS".to_string(), sys_kp.public_key());
    let sys_jwt = sys_claims.encode(&operator_kp)?;
    // AUTH Account that will be used for the Auth Callout service and issuing Sentinel Credentials
    let auth_account_kp = KeyPair::new_account();
    let mut auth_claims = Account::new_claims("AUTH".to_string(), auth_account_kp.public_key());
    // AUTH User nkey for the purposes of the Auth Callout service
    let auth_user_kp = KeyPair::new_user();
    // Set up decentralized Auth Callout on the AUTH Account
    auth_claims.nats.authorization = Some(ExternalAuthorization {
        auth_users: Some(BTreeSet::from([auth_user_kp.public_key()])),
        allowed_accounts: Some(BTreeSet::from(["*".to_string()])),
        xkey: None,
    });
    // AUTH Account JWT for resolver preloading
    let auth_account_jwt = auth_claims.encode(&operator_kp)?;
    // WASMCLOUD Account - used by Auth Callout service for placing users into it
    let wasmcloud_kp = KeyPair::new_account();
    let wasmcloud_claims = Account::new_claims("WASMCLOUD".to_string(), wasmcloud_kp.public_key());
    let wasmcloud_jwt = wasmcloud_claims.encode(&operator_kp)?;

    // Bare minimum NATS Server configuration to set up decentralized Auth Callout with an in-memory resolver
    let config = NatsConfig {
        operator: Some(operator_jwt),
        system_account: Some(sys_kp.public_key()),
        resolver_preload: Some(BTreeMap::from([
            (sys_kp.public_key(), sys_jwt),
            (auth_account_kp.public_key(), auth_account_jwt),
            (wasmcloud_kp.public_key(), wasmcloud_jwt),
        ])),
        resolver: Some(NatsResolver::Memory {}),
        ..Default::default()
    };

    Ok((config, auth_account_kp, auth_user_kp))
}

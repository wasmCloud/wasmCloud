use serde_json::json;
use sqldb_dynamodb_lib::{SqlDbClient, StorageConfig};
use wasmbus_rpc::provider::prelude::*;
use wasmcloud_interface_sqldb::*;
use wasmcloud_test_util::{
    check,
    cli::print_test_results,
    provider_test::test_provider,
    run_selected_spawn,
    testing::{TestOptions, TestResult},
};

#[tokio::test]
async fn run_all() {
    // start provider (not necessary to call this here since it is create lazily,
    // but debug output order makes more sense)
    let provider = test_provider().await;
    // allow time for it to start up and finish linking before we send rpc
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let opts = TestOptions::default();
    let res = run_selected_spawn!(opts, health_check);
    print_test_results(&res);

    let passed = res.iter().filter(|tr| tr.passed).count();
    let total = res.len();
    assert_eq!(passed, total, "{} passed out of {}", passed, total);

    // ask the provider to shut down gracefully
    let _ = provider.shutdown().await;
}

/// test that health check returns healthy
async fn health_check(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    // health check
    let hc = prov.health_check().await;
    check!(hc.is_ok())?;
    Ok(())
}

/// Helper function to create a StorageClient with local testing overrides
async fn test_client() -> SqlDbClient {
    let conf = StorageConfig {
        endpoint: Some("http://localhost:8000".to_string()),
        access_key_id: Some("DUMMYIDEXAMPLE".to_string()),
        secret_access_key: Some("DUMMYIDEXAMPLE".to_string()),
        ..Default::default()
    };

    SqlDbClient::new(conf, None).await
}

#[tokio::test]
async fn test_execute() {
    let client: SqlDbClient = test_client().await;
    let ctx = wasmbus_rpc::common::Context::default();

    let statement = Statement {
        database: None,
        parameters: None,
        sql: "INSERT INTO GameScores value{'UserId':'A1','GameTitle':'wasm','TopScore':1000}"
            .to_string(),
    };
    client.execute(&ctx, &statement).await.unwrap();

    let statement = Statement {
        database: None,
        parameters: None,
        sql: "SELECT * FROM GameScores".to_string(),
    };
    //test query and verify row was added.
    let result = client.query(&ctx, &statement).await.unwrap();
    assert_eq!(
        String::from_utf8(result.rows).unwrap(),
        format!(
            "xH{}",
            json!([{"UserId": {"S": "A1"}, "GameTitle": {"S": "wasm"}, "TopScore": {"N":
        "1000"}}])
        )
    );
}

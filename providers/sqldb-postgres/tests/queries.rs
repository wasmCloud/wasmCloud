use wasmbus_rpc::{minicbor::Decode, provider::prelude::*};
use wasmcloud_interface_sqldb::*;
use wasmcloud_test_util::{
    check,
    cli::print_test_results,
    provider_test::{test_provider, Provider},
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
    let res = run_selected_spawn!(opts, health_check, query, flavor_test);
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

/// response to "select typname from pg_catalog.pg_type ..."
#[derive(Decode)]
struct BuiltinTypes {
    #[n(0)]
    typname: String,
}

/// decode results from cbor to concrete structure, and print results
fn process_results(resp: QueryResult) -> Result<(), SqlDbError> {
    println!("Received {} rows: ", resp.num_rows);
    let rows: Vec<BuiltinTypes> = minicbor::decode(&resp.rows)?;
    assert_eq!(resp.num_rows, 8, "should be 8 int* types");

    for r in rows.iter() {
        println!("type: {}", r.typname,);
    }
    Ok(())
}

/// send select query to provider,
/// and call process_results to decode and dump results
async fn query(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let client = SqlDbSender::via(prov);
    let ctx = Context::default();

    // use a table that's alrady there - schema types in pg_catalog
    let resp = client
        .query(
            &ctx,
            &Statement {
                sql: "select typname from pg_catalog.pg_type where typname like 'int%'".to_string(),
                ..Default::default()
            },
        )
        .await?;

    process_results(resp)?;

    Ok(())
}

/// test to drop/create table, insert rows, and query
async fn flavor_test(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    let client = SqlDbSender::via(prov);
    let ctx = Context::default();
    flavor_queries(&ctx, &client).await?;
    Ok(())
}

#[derive(Decode)]
struct FlavorResult {
    #[n(0)]
    flavor: String,
}

async fn flavor_queries(ctx: &Context, client: &SqlDbSender<Provider>) -> Result<(), SqlDbError> {
    // remove it in case earlier test crashed
    let resp = client
        .execute(
            ctx,
            &Statement {
                sql: "drop table if exists test_flavors".to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(resp.rows_affected, 0);

    let resp = client
        .execute(
            ctx,
            &Statement {
                sql: r#"create table test_flavors
            ( 
              id INT4 NOT NULL,
              flavor VARCHAR(30) NOT NULL
             );"#
                .to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(resp.rows_affected, 0);

    let resp = client
        .execute(
            ctx,
            &Statement {
                sql: r#"insert into test_flavors (id,flavor) values
            (1, 'Vanilla'),
            (2, 'Chocolate'),
            (3, 'Mint Chocolate Chip'),
            (4, 'Strawberry'),
            (5, 'Cherry Garcia'),
            (6, 'Rum Raisin')
            ;
            "#
                .to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(resp.rows_affected, 6, "6 rows inserted");

    let resp = client
        .query(
            ctx,
            &Statement {
                sql: r#"select flavor from test_flavors 
                    where flavor like '%Chocolate%' order by id"#
                    .to_string(),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(resp.num_rows, 2, "select should have returned 2 rows");
    let rows: Vec<FlavorResult> = minicbor::decode(&resp.rows)?;
    assert_eq!(rows.len(), 2);
    assert_eq!(&rows.get(0).unwrap().flavor, "Chocolate",);
    assert_eq!(&rows.get(1).unwrap().flavor, "Mint Chocolate Chip",);

    let _resp = client
        .execute(
            ctx,
            &Statement {
                sql: "drop table if exists test_flavors".to_string(),
                ..Default::default()
            },
        )
        .await?;

    Ok(())
}

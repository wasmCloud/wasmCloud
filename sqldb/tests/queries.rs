use minicbor::Decode;
use wasmbus_rpc::provider::prelude::*;
use wasmcloud_interface_sqldb::*;
use wasmcloud_test_util::provider_test::Provider;
use wasmcloud_test_util::run_selected_spawn;
use wasmcloud_test_util::{
    check,
    cli::print_test_results,
    provider_test::test_provider,
    testing::{TestOptions, TestResult},
};

#[tokio::test]
async fn run_all() {
    // start provider (not necessary to call this here since it is create lazily,
    // but debug output order makes more sense)
    let _prov = test_provider().await;

    let opts = TestOptions::default();
    let res = run_selected_spawn!(&opts, health_check, query, flavor_test);
    print_test_results(&res);

    let passed = res.iter().filter(|tr| tr.pass).count();
    let total = res.len();
    assert_eq!(passed, total, "{} passed out of {}", passed, total);

    // try to let the provider shut dowwn gracefully
    let provider = test_provider().await;
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

/// response to "select first_name from customer"
#[derive(Decode)]
struct Name {
    #[n(0)]
    first_name: String,
}

fn process_results(resp: FetchResult) -> Result<(), SqlDbError> {
    println!("Received {} rows: ", resp.num_rows);
    let rows: Vec<Name> = minicbor::decode(&resp.rows)?;

    for r in rows.iter() {
        println!("Name: {}", r.first_name);
    }
    Ok(())
}

/// GET request
async fn query(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let client = SqlDbSender::via(prov);
    let ctx = Context::default();

    let resp = client
        .fetch(
            &ctx,
            &"select first_name from customer limit 10".to_string(),
        )
        .await?;

    process_results(resp)?;

    Ok(())
}

/// Execute request
async fn flavor_test(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    eprintln!("DBG: starting flavor_queries test");
    // create client and ctx
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
        .execute(&ctx, &"drop table if exists test_flavors".to_string())
        .await?;
    assert_eq!(resp.rows_affected, 0);

    let resp = client
        .execute(
            &ctx,
            &r#"create table test_flavors
            ( 
              id INT4 NOT NULL,
              flavor VARCHAR(30) NOT NULL
             );"#
            .to_string(),
        )
        .await?;
    assert_eq!(resp.rows_affected, 0);

    let resp = client
        .execute(
            &ctx,
            &r#"insert into test_flavors (id,flavor) values
            (1, 'Vanilla'),
            (2, 'Chocolate'),
            (3, 'Mint Chocolate Chip'),
            (4, 'Strawberry'),
            (5, 'Cherry Garcia'),
            (6, 'Rum Raisin')
            ;
            "#
            .to_string(),
        )
        .await?;
    assert_eq!(resp.rows_affected, 6, "6 rows inserted");

    let resp = client
        .fetch(
            &ctx,
            &r#"select flavor from test_flavors 
                    where flavor like '%Chocolate%' order by id"#
                .to_string(),
        )
        .await?;
    assert_eq!(resp.num_rows, 2, "select should have returned 2 rows");
    let rows: Vec<FlavorResult> = minicbor::decode(&resp.rows)?;
    assert_eq!(rows.len(), 2);
    assert_eq!(&rows.get(0).unwrap().flavor, "Chocolate",);
    assert_eq!(&rows.get(1).unwrap().flavor, "Mint Chocolate Chip",);

    let resp = client
        .execute(&ctx, &"drop table if exists test_flavors".to_string())
        .await?;
    eprintln!("DBG: drop table responded: {:?}", resp.rows_affected);

    Ok(())
}

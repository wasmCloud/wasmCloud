use wasmbus_rpc::provider::prelude::*;
use wasmcloud_interface_httpclient::*;
use wasmcloud_test_util::{
    check,
    cli::print_test_results,
    provider_test::test_provider,
    testing::{TestOptions, TestResult},
};
#[allow(unused_imports)]
use wasmcloud_test_util::{run_selected, run_selected_spawn};

#[tokio::test]
async fn run_all() {
    let opts = TestOptions::default();
    let res = run_selected_spawn!(&opts, health_check, get_request,);
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

/// GET request
async fn get_request(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    // create client and ctx
    let client = HttpClientSender::via(prov);
    let ctx = Context::default();

    let resp = client
        .request(&ctx, &HttpRequest::get("https://wttr.in/London?format=3"))
        .await?;
    assert_eq!(resp.status_code, 200, "status code");

    let body = String::from_utf8_lossy(&resp.body);
    assert!(body.contains("London"), "unexpected response: {}", &body);

    Ok(())
}

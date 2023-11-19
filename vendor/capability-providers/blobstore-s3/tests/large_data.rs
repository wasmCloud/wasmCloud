use std::iter::repeat_with;
use tracing::debug;
#[allow(unused_imports)]
use wasmbus_rpc::{
    common::{Context, Message, Transport},
    error::{RpcError, RpcResult},
};
use wasmcloud_interface_blobstore::{Blobstore, BlobstoreSender, Chunk, PutObjectRequest};
//use wasmcloud_interface_blobstore::{Blobstore, BlobstoreSender};
#[allow(unused_imports)]
use wasmcloud_test_util::{
    check, check_eq,
    cli::print_test_results,
    provider_test::test_provider,
    run_selected_spawn,
    testing::{TestOptions, TestResult},
};

const TYPE_OCTET_STREAM: &str = "application/octet-stream";

fn gen_bytes(len: usize) -> (Vec<u8>, u32) {
    let rng = fastrand::Rng::new();
    let bytes: Vec<u8> = repeat_with(|| rng.u8(..)).take(len).collect();
    let checksum = crc32fast::hash(&bytes);
    (bytes, checksum)
}

#[tokio::test]
async fn run_all() {
    let opts = TestOptions::default();
    tracing::try_init().ok();

    // initialize provider
    let _prov = test_provider().await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let res = run_selected_spawn!(opts, health_check, get_set_large,);
    print_test_results(&res);

    let passed = res.iter().filter(|tr| tr.passed).count();
    let total = res.len();
    assert_eq!(passed, total, "{} passed out of {}", passed, total);

    // try to let the provider shut down gracefully
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

async fn send_receive<T: Transport + Sync>(
    s3: &BlobstoreSender<T>,
    bucket: &str,
    len: usize,
    name: &str,
) -> RpcResult<()> {
    let ctx = wasmbus_rpc::common::Context::default();

    let (arr, _sum) = gen_bytes(len);
    debug!(%len, "sending object");
    s3.put_object(
        &ctx,
        &PutObjectRequest {
            chunk: Chunk {
                bytes: arr,
                container_id: bucket.to_string(),
                is_last: true,
                object_id: name.to_string(),
                offset: 0,
            },
            content_encoding: None,
            content_type: Some(TYPE_OCTET_STREAM.to_string()),
        },
    )
    .await
    .expect("send object");

    // get_object is disabled at the moment - when S3 returns an object it only returns 8KB in the
    // first chunk .. so it needs to be decided whether S3 get operaion should change
    // to buffer up a larger first chunk, (and then possibly using chunking api to process),
    // or leave it as streaming ..
    // For now, at least, blobstore-s3 can't be used to test InvocationResponse chunking,
    // because the response is always smaller than the default chunk size (128KB).
    //
    // operations, or
    //
    /*
    let res = s3
        .get_object(
            &ctx,
            &GetObjectRequest {
                container_id: bucket.to_string(),
                object_id: name.to_string(),
                range_end: None,
                range_start: None,
            },
        )
        .await
        .expect("get object");

    if let Some(ref err) = res.error {
        assert!(false, "GetObject ({}) failed: {}", len, err);
    }


    check!(res.error.is_none())?;
    check_eq!(res.content_length, len as u64)?;

    if let Some(chunk) = res.initial_chunk {
        check_eq!(chunk.is_last, true)?;
        check_eq!(res.content_length, chunk.bytes.len() as u64)?;
        check_eq!(chunk.object_id.as_str(), name)?;
        check_eq!(chunk.container_id.as_str(), bucket)?;

        let checksum = crc32fast::hash(&chunk.bytes);
        check_eq!(sum, checksum)?;
    } else {
        assert!(false, "missing initial chunk!");
    }
     */

    Ok(())
}

/// get and set large arrays
async fn get_set_large(_opt: &TestOptions) -> RpcResult<()> {
    let prov = test_provider().await;

    let ctx = Context::default();
    let s3 = BlobstoreSender::via(prov);

    let num = rand::random::<u64>();
    let bucket = format!("test.bigfile.{}", num);
    s3.create_container(&ctx, &bucket).await?;

    // 500 bytes
    send_receive(&s3, &bucket, 500, "arr500")
        .await
        .expect("500");

    // 50KB bytes
    send_receive(&s3, &bucket, 50 * 1024, "arr50K")
        .await
        .expect("50K");

    // 500KB bytes
    send_receive(&s3, &bucket, 500 * 1024, "arr500K")
        .await
        .expect("500K");

    // 900KB bytes
    send_receive(&s3, &bucket, 900 * 1024, "arr900K")
        .await
        .expect("900K");

    // 1MB bytes
    send_receive(&s3, &bucket, 1024 * 1024, "arr1MB")
        .await
        .expect("1MB");

    // 5MB bytes
    send_receive(&s3, &bucket, 5 * 1024 * 1024, "arr5MB")
        .await
        .expect("5MB");

    // 10MB bytes
    send_receive(&s3, &bucket, 10 * 1024 * 1024, "arr10MB")
        .await
        .expect("10MB");

    // clean up
    s3.remove_containers(&ctx, &vec![bucket])
        .await
        .expect("remove container");
    Ok(())
}

use crate::FileUpload;
use codec::core::CapabilityConfiguration;
use futures::TryStreamExt;
use rusoto_core::credential::{DefaultCredentialsProvider, StaticProvider};
use rusoto_core::Region;
use rusoto_s3::HeadObjectOutput;
use rusoto_s3::ListObjectsV2Output;
use rusoto_s3::Object;
use rusoto_s3::{
    CreateBucketRequest, DeleteBucketRequest, DeleteObjectRequest, GetObjectRequest,
    HeadObjectRequest, ListObjectsV2Request, PutObjectRequest, S3Client, S3,
};

use std::error::Error;

pub(crate) fn client_for_config(
    config: &CapabilityConfiguration,
) -> std::result::Result<S3Client, Box<dyn std::error::Error>> {
    let region = if config.values.contains_key("REGION") {
        Region::Custom {
            name: config.values["REGION"].clone(),
            endpoint: if config.values.contains_key("ENDPOINT") {
                config.values["ENDPOINT"].clone()
            } else {
                "s3.us-east-1.amazonaws.com".to_string()
            },
        }
    } else {
        Region::UsEast1
    };

    let client = if config.values.contains_key("AWS_ACCESS_KEY") {
        let provider = StaticProvider::new(
            config.values["AWS_ACCESS_KEY"].to_string(),
            config.values["AWS_SECRET_ACCESS_KEY"].to_string(),
            config.values.get("AWS_TOKEN").cloned(),
            config
                .values
                .get("TOKEN_VALID_FOR")
                .map(|t| t.parse::<i64>().unwrap()),
        );
        S3Client::new_with(
            rusoto_core::request::HttpClient::new().expect("Failed to create HTTP client"),
            provider,
            region,
        )
    } else {
        let provider = DefaultCredentialsProvider::new()?;
        S3Client::new_with(
            rusoto_core::request::HttpClient::new().expect("Failed to create HTTP client"),
            provider,
            region,
        )
    };

    Ok(client)
}

pub(crate) async fn create_bucket(client: &S3Client, name: &str) -> Result<(), Box<dyn Error>> {
    let create_bucket_req = CreateBucketRequest {
        bucket: name.to_string(),
        ..Default::default()
    };
    client.create_bucket(create_bucket_req).await?;
    Ok(())
}

pub(crate) async fn remove_bucket(client: &S3Client, bucket: &str) -> Result<(), Box<dyn Error>> {
    let delete_bucket_req = DeleteBucketRequest {
        bucket: bucket.to_owned(),
        ..Default::default()
    };

    client.delete_bucket(delete_bucket_req).await?;

    Ok(())
}

pub(crate) async fn remove_object(
    client: &S3Client,
    bucket: &str,
    id: &str,
) -> Result<(), Box<dyn Error>> {
    let delete_object_req = DeleteObjectRequest {
        bucket: bucket.to_string(),
        key: id.to_string(),
        ..Default::default()
    };

    client.delete_object(delete_object_req).await?;

    Ok(())
}

pub(crate) async fn get_blob_range(
    client: &S3Client,
    bucket: &str,
    id: &str,
    start: u64,
    end: u64,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let get_req = GetObjectRequest {
        bucket: bucket.to_owned(),
        key: id.to_owned(),
        range: Some(format!("bytes={}-{}", start, end)),
        ..Default::default()
    };

    let result = client.get_object(get_req).await?;
    let stream = result.body.unwrap();
    let body = stream
        .map_ok(|b| bytes::BytesMut::from(&b[..]))
        .try_concat()
        .await
        .unwrap();
    Ok(body.to_vec())
}

pub(crate) async fn complete_upload(
    client: &S3Client,
    upload: &FileUpload,
) -> Result<(), Box<dyn Error>> {
    let bytes = upload
        .chunks
        .iter()
        .fold(vec![], |a, c| [&a[..], &c.chunk_bytes[..]].concat());
    let put_request = PutObjectRequest {
        bucket: upload.container.to_string(),
        key: upload.id.to_string(),
        body: Some(bytes.into()),
        ..Default::default()
    };

    client.put_object(put_request).await?;
    Ok(())
}

pub(crate) async fn list_objects(
    client: &S3Client,
    bucket: &str,
) -> Result<Option<Vec<Object>>, Box<dyn Error>> {
    let list_obj_req = ListObjectsV2Request {
        bucket: bucket.to_owned(),
        ..Default::default()
    };
    let res: ListObjectsV2Output = client.list_objects_v2(list_obj_req).await?;

    Ok(res.contents)
}

pub(crate) async fn head_object(
    client: &S3Client,
    bucket: &str,
    key: &str,
) -> Result<HeadObjectOutput, Box<dyn Error>> {
    let head_req = HeadObjectRequest {
        bucket: bucket.to_owned(),
        key: key.to_owned(),
        ..Default::default()
    };

    client.head_object(head_req).await.map_err(|e| e.into())
}

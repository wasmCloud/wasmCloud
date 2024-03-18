wit_bindgen::generate!({
    world: "blobby",
});

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    str::FromStr,
};

use anyhow::{Context, Result};
use http::{StatusCode};

use exports::wasi::http::incoming_handler::Guest;
use wasi::blobstore::blobstore;
use wasi::http::types::*;

struct Blobby;

// lazy_static::lazy_static! {
//     static ref ALLOW_HEADERS: HashMap<String, Vec<String>> = vec![
//         (
//             http::header::ALLOW.to_string(),
//             vec![[Method::GET.to_string(), Method::POST.to_string(), Method::PUT.to_string(), Method::DELETE.to_string()].join(", ")]
//         )
//     ].into_iter().collect();
// }

const DEFAULT_CONTAINER_NAME: &str = "default";
const CONTAINER_HEADER_NAME: &str = "blobby-container";
const CONTAINER_PARAM_NAME: &str = "container";

/// A helper that will automatically create a container if it doesn't exist and returns an owned copy of the name for immediate use
fn ensure_container(name: &String) -> Result<()> {
    if !blobstore::container_exists(name).context("Unable check if container exists")? {
        blobstore::create_container(name).context("Unable to create container")?;
    }
    Ok(())
}

fn send_response_error(response_out: ResponseOutparam, status_code: u16, body: &str) {
    let response = OutgoingResponse::new(Fields::new());
    response
                .set_status_code(status_code)
                .expect("Unable to set status code");
            let response_body = response.body().unwrap();
            response_body
                .write()
                .unwrap()
                .blocking_write_and_flush(body.as_bytes())
                .unwrap();
            OutgoingBody::finish(response_body, None).expect("failed to finish response body");
            ResponseOutparam::set(response_out, Ok(response));
}

/// Implementation of Blobby trait methods
impl Guest for Blobby {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let response_body = response.body().unwrap();
        let container_id = request
            .path_with_query()
            .unwrap_or_else(|| "/".to_string())
            .split_once('?')
            .map(|(path, _query)| path.trim_matches('/').to_string())
            .unwrap_or_else(|| String::new());

        // Check that there isn't any subpathing. If we can split, it means that we have more than
        // one path element

        // /foo?container=bar
        // /foo

        if container_id.split_once('/').is_some() {
            send_response_error(response_out, StatusCode::BAD_REQUEST.as_u16(), "Cannot use a subpathed file name (e.g. foo/bar.txt)\n");
            return;
        }
        let container_id = container_id.to_owned();

        // Get the container name from the request
        if let Err(e) = ensure_container(&container_id) {
            send_response_error(response_out, StatusCode::BAD_GATEWAY.as_u16(), e.to_string().as_str());
            return;
        }

        match request.method() {
            Method::Get => get_object(&container_id, container_id).await,
            Method::Post | Method::Put => {
                let content_type = req
                    .header
                    .get(http::header::CONTENT_TYPE.as_str())
                    .and_then(|vals| {
                        if !vals.is_empty() {
                            Some(vals[0].clone())
                        } else {
                            None
                        }
                    });
                put_object(
                    ctx,
                    container_name,
                    container_id,
                    req.body.clone(),
                    content_type,
                )
                .await
            }
            Method::Delete => delete_object(ctx, container_name, container_id).await,
            _ => Ok(HttpResponse {
                status_code: StatusCode::METHOD_NOT_ALLOWED.as_u16(),
                header: ALLOW_HEADERS.clone(),
                body: Vec::with_capacity(0),
            }),
        }
    }
}

async fn get_object(
    container_name: &String,
    object_name: &String,
) -> Result<()> {

    // eprintln!("call `wasi:blobstore/container.container.has-object`...");
    // assert!(!created_container
    //     .has_object(&result_key)
    //     .expect("failed to check whether `result` object exists"));
    // eprintln!("call `wasi:blobstore/container.container.delete-object`...");
    // created_container
    //     .delete_object(&result_key)
    //     .expect("failed to delete object");

    // let result_value = blobstore::types::OutgoingValue::new_outgoing_value();
    // let mut result_stream = result_value
    //     .outgoing_value_write_body()
    //     .expect("failed to get outgoing value output stream");
    // let mut result_stream_writer = OutputStreamWriter::from(&mut result_stream);
    // eprintln!("write body to outgoing blobstore stream...");
    // result_stream_writer
    //     .write_all(&body)
    //     .expect("failed to write result to blobstore output stream");
    // eprintln!("flush outgoing blobstore stream...");
    // result_stream_writer
    //     .flush()
    //     .expect("failed to flush blobstore output stream");
    // eprintln!("call `wasi:blobstore/container.container.write-data`...");
    // created_container
    //     .write_data(&result_key, &result_value)
    //     .expect("failed to write `result`");


    let blobstore = BlobstoreSender::new();
    // Check that the object exists first. If it doesn't return the proper http response
    if !blobstore
        .object_exists(
            ctx,
            &ContainerObject {
                container_id: container_name.clone(),
                object_id: object_name.clone(),
            },
        )
        .await?
    {
        return Ok(HttpResponse::not_found());
    }

    let get_object_request = GetObjectRequest {
        object_id: object_name,
        container_id: container_name,
        range_start: Some(0),
        range_end: None,
    };
    let o = blobstore.get_object(ctx, &get_object_request).await?;
    if !o.success {
        return Ok(HttpResponse {
            status_code: StatusCode::BAD_GATEWAY.as_u16(),
            body: o.error.unwrap_or_default().into_bytes(),
            ..Default::default()
        });
    }
    info!("successfully got an object!");
    let body = match o.initial_chunk {
        Some(c) => c.bytes,
        None => {
            return Ok(HttpResponse {
                status_code: StatusCode::BAD_GATEWAY.as_u16(),
                body: "Blobstore sent empty data chunk when full file was requested"
                    .as_bytes()
                    .to_vec(),
                ..Default::default()
            })
        }
    };

    let headers = o
        .content_type
        .map(|c| {
            vec![(http::header::CONTENT_TYPE.to_string(), vec![c])]
                .into_iter()
                .collect::<HashMap<String, Vec<String>>>()
        })
        .unwrap_or_default();

    Ok(HttpResponse {
        body,
        header: headers,
        ..Default::default()
    })
}

async fn delete_object(
    ctx: &Context,
    container_name: String,
    object_name: String,
) -> RpcResult<HttpResponse> {
    let blobstore = BlobstoreSender::new();

    let mut res = blobstore
        .remove_objects(
            ctx,
            &RemoveObjectsRequest {
                container_id: container_name,
                objects: vec![object_name],
            },
        )
        .await?;

    if !res.is_empty() {
        // SAFETY: We checked that the vec wasn't empty above
        let res = res.remove(0);
        return Ok(HttpResponse {
            status_code: StatusCode::BAD_GATEWAY.as_u16(),
            body: format!(
                "Error when deleting object from store: {}",
                res.error.unwrap_or_default()
            )
            .into_bytes(),
            ..Default::default()
        });
    }

    Ok(HttpResponse::ok(""))
}

async fn put_object(
    ctx: &Context,
    container_name: String,
    object_name: String,
    data: Vec<u8>,
    content_type: Option<String>,
) -> RpcResult<HttpResponse> {
    let blobstore = BlobstoreSender::new();

    blobstore
        .put_object(
            ctx,
            &PutObjectRequest {
                chunk: Chunk {
                    container_id: container_name,
                    object_id: object_name,
                    bytes: data,
                    offset: 0,
                    is_last: true,
                },
                content_type,
                ..Default::default()
            },
        )
        .await?;

    Ok(HttpResponse::ok(Vec::with_capacity(0)))
}

// Gets the container name from the header or a query param. The query param takes precedence
fn get_container_name(req: &HttpRequest) -> Cow<'_, str> {
    if let Some(param) = form_urlencoded::parse(req.query_string.as_bytes())
        .find(|(n, _)| n == CONTAINER_PARAM_NAME)
        .map(|(_, v)| v)
    {
        param
    } else if let Some(header) = req.header.get(CONTAINER_HEADER_NAME).and_then(|vals| {
        // Not using `vals.get` because you can't return data owned by the current function
        if !vals.is_empty() {
            // There should only be one, but if there are more than one, only grab the first one
            Some(Cow::from(vals[0].as_str()))
        } else {
            None
        }
    }) {
        header
    } else {
        Cow::from(DEFAULT_CONTAINER_NAME)
    }
}

// export! defines that the `Blobby` struct defined below is going to define
// the exports of the `world`, namely the `run` function.
export!(Blobby);

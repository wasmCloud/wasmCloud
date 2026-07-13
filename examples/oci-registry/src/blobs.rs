//! Blob content operations: serving (`GET`/`HEAD`/`DELETE`), streaming uploads
//! with digest verification, and cross-repository mount.

use crate::bindings;
use crate::http::{error_response, method_not_allowed, respond, respond_owned, stream_response};
use crate::keys::blob_key;
use crate::storage::{
    blob_err, copy_object, delete_object, has_object, object_size, open_object_stream,
};
use crate::uploads::begin_upload_session;
use crate::{Container, Method, Request, Response};
use sha2::{Digest, Sha256};

pub(crate) async fn handle_blob(
    container: &Container,
    method: &Method,
    name: &str,
    digest: &str,
) -> Result<Response, String> {
    let key = blob_key(name, digest);

    match method {
        Method::Get | Method::Head => {
            if !has_object(container, &key).await? {
                return Ok(error_response(404, "BLOB_UNKNOWN", "blob unknown"));
            }
            let size = object_size(container, &key).await?;
            let headers = vec![
                (
                    "content-type".to_string(),
                    "application/octet-stream".to_string(),
                ),
                ("docker-content-digest".to_string(), digest.to_string()),
                ("content-length".to_string(), size.to_string()),
            ];
            if matches!(method, Method::Head) {
                // HEAD carries no body, so Content-Length is omitted (a declared
                // length with an empty body is rejected by the runtime).
                return Ok(respond_owned(
                    200,
                    headers.into_iter().take(2).collect(),
                    Vec::new(),
                ));
            }
            // Stream the blob straight from the blobstore into the HTTP response
            // body: the reader returned by `get-data` is handed to the response
            // unchanged, so bytes never pass through a guest-side buffer.
            let body = open_object_stream(container, &key, size).await?;
            Ok(stream_response(200, headers, body))
        }
        Method::Delete => {
            if !has_object(container, &key).await? {
                return Ok(error_response(404, "BLOB_UNKNOWN", "blob unknown"));
            }
            delete_object(container, &key).await?;
            Ok(respond(202, &[], Vec::new()))
        }
        _ => Ok(method_not_allowed()),
    }
}

/// Cross-repository blob mount: if `digest` exists in the `from` repository,
/// copy it into `name` and return `201`; otherwise fall back to a normal upload
/// session (`202`), as the spec permits.
pub(crate) async fn mount_blob(
    container: &Container,
    name: &str,
    digest: &str,
    from: &str,
) -> Result<Response, String> {
    let source = blob_key(from, digest);
    if !has_object(container, &source).await? {
        return begin_upload_session(container, name).await;
    }
    let dest = blob_key(name, digest);
    // Content-addressed, so skip the copy if the target repo already has it.
    if !has_object(container, &dest).await? {
        copy_object(&source, &dest).await?;
    }
    Ok(blob_created(name, digest))
}

/// Stream an upload straight into the blobstore, hashing as bytes flow and
/// verifying the digest at end-of-stream. `prior` bytes (buffered from earlier
/// PATCH chunks) are written first; a monolithic upload passes an empty `prior`
/// and never buffers the blob.
///
/// Blobs are content-addressed, so an existing object at the target key is
/// already exactly `expected` and is left untouched (idempotent). Only a blob
/// this call newly created is removed on a digest mismatch, so a bad push can
/// never clobber a previously-stored blob. (A concurrent first-time push of the
/// same digest where one side sends mismatched content is not guarded — an
/// example-acceptable edge; a production registry would stage to a temp key.)
pub(crate) async fn stream_and_finalize(
    container: &Container,
    name: &str,
    expected: &str,
    prior: Vec<u8>,
    request: Request,
) -> Result<Response, String> {
    let key = blob_key(name, expected);

    // Already present (content-addressed) — accept without rewriting; the request
    // body is dropped unread.
    if has_object(container, &key).await? {
        return Ok(blob_created(name, expected));
    }

    let (res_tx, res_rx) = bindings::wit_future::new(|| Ok(()));
    let (mut body, _trailers) = Request::consume_body(request, res_rx);
    let (mut tx, rx) = bindings::wit_stream::new();

    // The writer task tees the request body into the blobstore stream (`tx`) and
    // a running hasher; the digest is stored before `tx` is dropped so it's
    // visible once `write_data` (which drains `rx`) returns.
    let digest_cell = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    let writer_digest = digest_cell.clone();
    wit_bindgen::spawn_local(async move {
        let mut hasher = Sha256::new();
        if !prior.is_empty() {
            hasher.update(&prior);
            let _ = tx.write_all(prior).await;
        }
        loop {
            let (status, chunk) = body.read(Vec::with_capacity(64 * 1024)).await;
            if !chunk.is_empty() {
                hasher.update(&chunk);
                let _ = tx.write_all(chunk).await;
            }
            if matches!(status, wit_bindgen::StreamResult::Dropped) {
                break;
            }
        }
        *writer_digest.borrow_mut() = format!("sha256:{}", hex::encode(hasher.finalize()));
        drop(tx);
        drop(res_tx);
    });

    container
        .write_data(key.clone(), rx)
        .await
        .map_err(blob_err)?;

    let actual = digest_cell.borrow().clone();
    if actual != expected {
        // Remove the just-written object whose contents didn't match the claimed
        // digest (it can only be the blob this call created — see the doc above).
        let _ = delete_object(container, &key).await;
        return Ok(error_response(
            400,
            "DIGEST_INVALID",
            "provided digest did not match uploaded content",
        ));
    }

    Ok(blob_created(name, expected))
}

/// The `201 Created` response returned once a blob is committed at its digest.
fn blob_created(name: &str, digest: &str) -> Response {
    respond_owned(
        201,
        vec![
            ("location".to_string(), format!("/v2/{name}/blobs/{digest}")),
            ("docker-content-digest".to_string(), digest.to_string()),
        ],
        Vec::new(),
    )
}

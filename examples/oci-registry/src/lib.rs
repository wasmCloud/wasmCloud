//! An OCI Distribution Spec (v2) registry implemented as a WebAssembly component,
//! backed by the async, native-stream `wasmcloud:blobstore@0.1.0` interface.
//!
//! The component is wasip3: it exports `wasi:http/handler@0.3.0` and stores every
//! blob, manifest, and tag as a blobstore object. Object bodies move as native
//! component-model `stream<u8>` values (no `wasi:io`), and every blobstore call is
//! `async`.
//!
//! | Operation          | Method | Path                                          |
//! | ------------------ | ------ | --------------------------------------------- |
//! | API version check  | GET    | `/v2/`                                         |
//! | Initiate upload    | POST   | `/v2/<name>/blobs/uploads/`                    |
//! | Cross-repo mount   | POST   | `/v2/<name>/blobs/uploads/?mount=&from=`      |
//! | Upload a chunk     | PATCH  | `/v2/<name>/blobs/uploads/<session>`          |
//! | Complete upload    | PUT    | `/v2/<name>/blobs/uploads/<session>?digest=`  |
//! | Pull a blob        | GET    | `/v2/<name>/blobs/<digest>`                    |
//! | Check a blob       | HEAD   | `/v2/<name>/blobs/<digest>`                    |
//! | Delete a blob      | DELETE | `/v2/<name>/blobs/<digest>`                    |
//! | Push a manifest    | PUT    | `/v2/<name>/manifests/<reference>`            |
//! | Pull a manifest    | GET    | `/v2/<name>/manifests/<reference>`            |
//! | Check a manifest   | HEAD   | `/v2/<name>/manifests/<reference>`            |
//! | Delete a manifest  | DELETE | `/v2/<name>/manifests/<reference>`            |
//! | List tags          | GET    | `/v2/<name>/tags/list`                         |
//! | List referrers     | GET    | `/v2/<name>/referrers/<digest>`               |
//!
//! The registry logic is split by functionality: [`route`] (URL parsing),
//! [`storage`] (blobstore + key naming), [`http`] (response building), [`util`]
//! (digests/queries), and one module per resource: [`blobs`], [`manifests`],
//! [`tags`], [`referrers`], [`uploads`].

mod bindings {
    wit_bindgen::generate!({
        world: "oci-registry",
        path: "wit",
        generate_all,
    });
}

mod blobs;
mod http;
mod manifests;
mod referrers;
mod route;
mod storage;
mod tags;
mod uploads;
mod util;

use bindings::exports::wasi::http::handler::Guest as Handler;

// Generated types shared across the modules.
use bindings::wasi::http::types::ErrorCode;
pub(crate) use bindings::wasi::http::types::{Fields, Method, Request, Response};
pub(crate) use bindings::wasmcloud::blobstore::container::Container;

use crate::blobs::{handle_blob, mount_blob, stream_and_finalize};
use crate::http::{error_response, header_str, method_not_allowed, read_request_body, respond};
use crate::manifests::handle_manifest;
use crate::referrers::handle_referrers;
use crate::route::Route;
use crate::storage::{delete_object, ensure_container, read_object, upload_key};
use crate::tags::handle_tags_list;
use crate::uploads::{begin_upload_session, handle_upload};
use crate::util::query_param;

struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        // OCI-level outcomes (404/400/etc.) are normal responses; only an
        // unexpected backend failure bubbles up as a 500.
        Ok(match dispatch(request).await {
            Ok(response) => response,
            Err(message) => error_response(500, "INTERNAL_ERROR", &message),
        })
    }
}

async fn dispatch(request: Request) -> Result<Response, String> {
    let method = request.get_method();
    let path_and_query = request.get_path_with_query().unwrap_or_default();
    let (raw_path, query) = path_and_query
        .split_once('?')
        .unwrap_or((path_and_query.as_str(), ""));
    let path = raw_path.trim_matches('/').to_string();
    let query = query.to_string();

    let headers = request.get_headers();
    let content_type = header_str(&headers, "content-type");
    let content_range = header_str(&headers, "content-range");

    // `GET /v2/` is the base API version check used by clients to probe support.
    if path == "v2" {
        return Ok(respond(
            200,
            &[
                ("docker-distribution-api-version", "registry/2.0"),
                ("content-type", "application/json"),
            ],
            b"{}".to_vec(),
        ));
    }

    let Some(spec) = path.strip_prefix("v2/") else {
        return Ok(error_response(
            404,
            "UNSUPPORTED",
            "only the /v2/ registry API is supported",
        ));
    };

    let container = ensure_container().await?;
    let route = Route::parse(spec);

    // Streaming upload fast-paths: pipe the request body straight into the
    // blobstore while hashing, instead of buffering the whole blob. Monolithic
    // uploads (empty session) stream end-to-end; a PUT that finalizes prior
    // PATCH chunks streams those buffered bytes first.
    if let Some(Route::UploadInit { name }) = route
        && matches!(method, Method::Post)
    {
        // Cross-repository blob mount: `?mount=<digest>&from=<source-repo>`.
        if let Some(mount_digest) = query_param(&query, "mount") {
            return match query_param(&query, "from") {
                Some(from) => mount_blob(&container, name, &mount_digest, &from).await,
                // `mount` without `from`: no automatic content discovery, so fall
                // back to a normal upload session.
                None => begin_upload_session(&container, name).await,
            };
        }
        return match query_param(&query, "digest") {
            Some(expected) => {
                stream_and_finalize(&container, name, &expected, Vec::new(), request).await
            }
            None => begin_upload_session(&container, name).await,
        };
    }
    if let Some(Route::Upload { name, session }) = route
        && matches!(method, Method::Put)
        && let Some(expected) = query_param(&query, "digest")
    {
        let key = upload_key(name, session);
        let Some(prior) = read_object(&container, &key).await? else {
            return Ok(error_response(
                404,
                "BLOB_UPLOAD_UNKNOWN",
                "upload session unknown",
            ));
        };
        let response = stream_and_finalize(&container, name, &expected, prior, request).await?;
        delete_object(&container, &key).await?;
        return Ok(response);
    }

    // Remaining body-bearing methods (manifests, PATCH chunks) are buffered.
    let body = if matches!(method, Method::Put | Method::Post | Method::Patch) {
        read_request_body(request).await
    } else {
        Vec::new()
    };

    match route {
        Some(Route::TagsList { name }) => handle_tags_list(&container, name, &query).await,
        Some(Route::Referrers { name, digest }) => {
            handle_referrers(&container, name, digest, &query).await
        }
        Some(Route::Manifest { name, reference }) => {
            handle_manifest(
                &container,
                &method,
                name,
                reference,
                content_type.as_deref(),
                &body,
            )
            .await
        }
        // POST is handled by the streaming fast-path above; anything else here
        // is an unsupported method on the uploads endpoint.
        Some(Route::UploadInit { .. }) => Ok(method_not_allowed()),
        Some(Route::Upload { name, session }) => {
            handle_upload(
                &container,
                &method,
                name,
                session,
                content_range.as_deref(),
                &body,
            )
            .await
        }
        Some(Route::Blob { name, digest }) => handle_blob(&container, &method, name, digest).await,
        None => Ok(error_response(
            404,
            "NAME_UNKNOWN",
            "unrecognized registry route",
        )),
    }
}

bindings::export!(Component with_types_in bindings);

//! Blob upload sessions: initiation, chunked `PATCH` appends, and status.
//!
//! Monolithic and finalizing (`PUT ?digest=`) uploads are handled by the
//! streaming fast-path in `crate::dispatch` (see [`crate::blobs`]).

use crate::http::{error_response, method_not_allowed, respond_owned};
use crate::keys::upload_key;
use crate::storage::{has_object, object_size, read_object, write_object};
use crate::util::{new_session_id, range_header, range_start};
use crate::{Container, Method, Response};

/// Begin a resumable upload session backed by an (initially empty) object.
pub(crate) async fn begin_upload_session(
    container: &Container,
    name: &str,
) -> Result<Response, String> {
    let session = new_session_id();
    write_object(container, &upload_key(name, &session), Vec::new()).await?;

    Ok(respond_owned(
        202,
        vec![
            (
                "location".to_string(),
                format!("/v2/{name}/blobs/uploads/{session}"),
            ),
            ("docker-upload-uuid".to_string(), session),
            ("range".to_string(), "0-0".to_string()),
        ],
        Vec::new(),
    ))
}

pub(crate) async fn handle_upload(
    container: &Container,
    method: &Method,
    name: &str,
    session: &str,
    content_range: Option<&str>,
    body: &[u8],
) -> Result<Response, String> {
    let key = upload_key(name, session);

    match method {
        // Status of an in-progress upload.
        Method::Get => {
            if !has_object(container, &key).await? {
                return Ok(error_response(
                    404,
                    "BLOB_UPLOAD_UNKNOWN",
                    "upload session unknown",
                ));
            }
            let size = object_size(container, &key).await?;
            Ok(respond_owned(
                204,
                vec![
                    (
                        "location".to_string(),
                        format!("/v2/{name}/blobs/uploads/{session}"),
                    ),
                    ("docker-upload-uuid".to_string(), session.to_string()),
                    ("range".to_string(), range_header(size)),
                ],
                Vec::new(),
            ))
        }
        // Append a chunk to the session.
        Method::Patch => {
            let Some(mut buf) = read_object(container, &key).await? else {
                return Ok(error_response(
                    404,
                    "BLOB_UPLOAD_UNKNOWN",
                    "upload session unknown",
                ));
            };
            // When a Content-Range is supplied, it must start exactly where the
            // session currently ends; out-of-order or replayed chunks are
            // rejected with 416 (per the chunked-upload spec).
            if let Some(range) = content_range {
                let Some(start) = range_start(range) else {
                    return Ok(error_response(
                        400,
                        "SIZE_INVALID",
                        "malformed Content-Range header",
                    ));
                };
                if start != buf.len() as u64 {
                    return Ok(respond_owned(
                        416,
                        vec![
                            (
                                "location".to_string(),
                                format!("/v2/{name}/blobs/uploads/{session}"),
                            ),
                            ("docker-upload-uuid".to_string(), session.to_string()),
                            ("range".to_string(), range_header(buf.len() as u64)),
                        ],
                        Vec::new(),
                    ));
                }
            }
            buf.extend_from_slice(body);
            let new_len = buf.len() as u64;
            write_object(container, &key, buf).await?;

            Ok(respond_owned(
                202,
                vec![
                    (
                        "location".to_string(),
                        format!("/v2/{name}/blobs/uploads/{session}"),
                    ),
                    ("docker-upload-uuid".to_string(), session.to_string()),
                    ("range".to_string(), range_header(new_len)),
                ],
                Vec::new(),
            ))
        }
        // A PUT that finalizes an upload carries `?digest=` and is handled by the
        // streaming fast-path in `dispatch`; reaching here means it was missing.
        Method::Put => Ok(error_response(
            400,
            "DIGEST_INVALID",
            "digest query parameter is required to complete an upload",
        )),
        _ => Ok(method_not_allowed()),
    }
}

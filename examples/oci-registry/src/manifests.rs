//! Manifest push/pull/delete and the `subject`/referrer metadata extracted on push.

use crate::http::{error_response, method_not_allowed, respond, respond_owned};
use crate::storage::{
    delete_object, has_object, manifest_key, media_type_key, read_object, referrer_key, tag_key,
    write_object,
};
use crate::util::{is_digest, sha256_digest};
use crate::{Container, Method, Response};

/// Media type returned for manifests when no stored content type is available.
const DEFAULT_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

pub(crate) async fn handle_manifest(
    container: &Container,
    method: &Method,
    name: &str,
    reference: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<Response, String> {
    match method {
        Method::Put => {
            let digest = sha256_digest(body);
            let media_type = content_type
                .unwrap_or(DEFAULT_MANIFEST_MEDIA_TYPE)
                .to_string();

            let content_key = manifest_key(name, &digest);
            write_object(container, &content_key, body.to_vec()).await?;
            write_object(
                container,
                &media_type_key(&content_key),
                media_type.clone().into_bytes(),
            )
            .await?;

            // A tag reference also gets a pointer object mapping tag -> digest.
            if !is_digest(reference) {
                write_object(
                    container,
                    &tag_key(name, reference),
                    digest.clone().into_bytes(),
                )
                .await?;
            }

            // If the manifest declares a `subject`, index a referrer descriptor
            // under that subject's digest so it shows up in the referrers API.
            let meta = ManifestMetadata::parse(body);
            let mut headers = vec![
                (
                    "location".to_string(),
                    format!("/v2/{name}/manifests/{digest}"),
                ),
                ("docker-content-digest".to_string(), digest.clone()),
            ];
            if let Some(subject) = &meta.subject {
                let descriptor = meta.descriptor(&digest, &media_type, body.len() as u64);
                write_object(
                    container,
                    &referrer_key(name, subject, &digest),
                    descriptor.to_string().into_bytes(),
                )
                .await?;
                headers.push(("oci-subject".to_string(), subject.clone()));
            }

            Ok(respond_owned(201, headers, Vec::new()))
        }
        Method::Get | Method::Head => {
            let Some(digest) = resolve_reference(container, name, reference).await? else {
                return Ok(error_response(404, "MANIFEST_UNKNOWN", "manifest unknown"));
            };
            let content_key = manifest_key(name, &digest);
            let Some(data) = read_object(container, &content_key).await? else {
                return Ok(error_response(404, "MANIFEST_UNKNOWN", "manifest unknown"));
            };
            let media_type = read_object(container, &media_type_key(&content_key))
                .await?
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_else(|| DEFAULT_MANIFEST_MEDIA_TYPE.to_string());

            if matches!(method, Method::Head) {
                return Ok(respond_owned(
                    200,
                    vec![
                        ("content-type".to_string(), media_type),
                        ("docker-content-digest".to_string(), digest),
                    ],
                    Vec::new(),
                ));
            }
            let len = data.len().to_string();
            Ok(respond_owned(
                200,
                vec![
                    ("content-type".to_string(), media_type),
                    ("docker-content-digest".to_string(), digest),
                    ("content-length".to_string(), len),
                ],
                data,
            ))
        }
        Method::Delete => {
            // Deleting by digest removes the manifest content (any tags still
            // pointing at it become dangling). Deleting by tag only removes that
            // tag, leaving the shared content and other tags intact.
            if is_digest(reference) {
                let content_key = manifest_key(name, reference);
                let Some(data) = read_object(container, &content_key).await? else {
                    return Ok(error_response(404, "MANIFEST_UNKNOWN", "manifest unknown"));
                };
                // Drop the referrer entry this manifest may have registered.
                if let Some(subject) = ManifestMetadata::parse(&data).subject {
                    delete_object(container, &referrer_key(name, &subject, reference)).await?;
                }
                delete_object(container, &content_key).await?;
                delete_object(container, &media_type_key(&content_key)).await?;
            } else {
                let tag = tag_key(name, reference);
                if !has_object(container, &tag).await? {
                    return Ok(error_response(404, "MANIFEST_UNKNOWN", "manifest unknown"));
                }
                delete_object(container, &tag).await?;
            }
            Ok(respond(202, &[], Vec::new()))
        }
        _ => Ok(method_not_allowed()),
    }
}

/// Resolve a manifest reference (either a digest or a tag) to a concrete digest.
async fn resolve_reference(
    container: &Container,
    name: &str,
    reference: &str,
) -> Result<Option<String>, String> {
    if is_digest(reference) {
        return Ok(Some(reference.to_string()));
    }
    match read_object(container, &tag_key(name, reference)).await? {
        Some(bytes) => Ok(String::from_utf8(bytes).ok()),
        None => Ok(None),
    }
}

/// The subset of a manifest relevant to the referrers API.
struct ManifestMetadata {
    /// Digest from the manifest's `subject` descriptor, if any.
    subject: Option<String>,
    /// The manifest's `artifactType`, falling back to `config.mediaType`.
    artifact_type: Option<String>,
    /// The manifest's top-level `annotations`, propagated into the descriptor.
    annotations: Option<serde_json::Value>,
}

impl ManifestMetadata {
    fn parse(data: &[u8]) -> Self {
        let value: serde_json::Value =
            serde_json::from_slice(data).unwrap_or(serde_json::Value::Null);
        let subject = value
            .get("subject")
            .and_then(|s| s.get("digest"))
            .and_then(|d| d.as_str())
            .map(str::to_string);
        let artifact_type = value
            .get("artifactType")
            .and_then(|v| v.as_str())
            .or_else(|| {
                value
                    .get("config")
                    .and_then(|c| c.get("mediaType"))
                    .and_then(|v| v.as_str())
            })
            .map(str::to_string);
        let annotations = value.get("annotations").cloned();
        Self {
            subject,
            artifact_type,
            annotations,
        }
    }

    /// Build the OCI descriptor recorded in the referrers index for this manifest.
    fn descriptor(&self, digest: &str, media_type: &str, size: u64) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("mediaType".into(), media_type.into());
        map.insert("digest".into(), digest.into());
        map.insert("size".into(), size.into());
        if let Some(artifact_type) = &self.artifact_type {
            map.insert("artifactType".into(), artifact_type.clone().into());
        }
        if let Some(annotations) = &self.annotations {
            map.insert("annotations".into(), annotations.clone());
        }
        serde_json::Value::Object(map)
    }
}

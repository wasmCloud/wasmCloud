//! The referrers API: list the manifests that declare a given digest as their
//! `subject`, as an OCI image index.

use crate::http::respond_owned;
use crate::keys::referrer_prefix;
use crate::storage::{list_keys, read_object};
use crate::util::query_param;
use crate::{Container, Response};

/// Media type of the image index returned by the referrers API.
const OCI_IMAGE_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";

pub(crate) async fn handle_referrers(
    container: &Container,
    name: &str,
    subject: &str,
    query: &str,
) -> Result<Response, String> {
    let prefix = format!("{}/", referrer_prefix(name, subject));
    let filter = query_param(query, "artifactType");

    let mut manifests = Vec::new();
    for key in list_keys(container).await? {
        if !key.starts_with(&prefix) {
            continue;
        }
        let Some(bytes) = read_object(container, &key).await? else {
            continue;
        };
        let Ok(descriptor) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if let Some(want) = &filter {
            let got = descriptor.get("artifactType").and_then(|v| v.as_str());
            if got != Some(want.as_str()) {
                continue;
            }
        }
        manifests.push(descriptor);
    }

    let index = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": OCI_IMAGE_INDEX_MEDIA_TYPE,
        "manifests": manifests,
    });

    let mut headers = vec![(
        "content-type".to_string(),
        OCI_IMAGE_INDEX_MEDIA_TYPE.to_string(),
    )];
    // Signal that the artifactType filter was honored (per the referrers spec).
    if filter.is_some() {
        headers.push((
            "oci-filters-applied".to_string(),
            "artifactType".to_string(),
        ));
    }
    Ok(respond_owned(200, headers, index.to_string().into_bytes()))
}

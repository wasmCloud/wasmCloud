//! Parsing of the OCI registry URL space into typed routes.

/// A parsed OCI registry route. `name` is the (possibly multi-segment) repository
/// name, e.g. `library/nginx`.
#[derive(Clone, Copy)]
pub(crate) enum Route<'a> {
    TagsList { name: &'a str },
    Referrers { name: &'a str, digest: &'a str },
    Manifest { name: &'a str, reference: &'a str },
    UploadInit { name: &'a str },
    Upload { name: &'a str, session: &'a str },
    Blob { name: &'a str, digest: &'a str },
}

impl<'a> Route<'a> {
    /// Parse the portion of the path after the `v2/` prefix. Markers are matched
    /// from the right so repository names that contain reserved words still parse,
    /// and the more specific `blobs/uploads` cases are checked before `blobs`.
    pub(crate) fn parse(spec: &'a str) -> Option<Route<'a>> {
        if let Some(name) = spec.strip_suffix("/tags/list") {
            return Some(Route::TagsList { name });
        }
        if let Some(idx) = spec.rfind("/referrers/") {
            let name = spec.get(..idx)?;
            let digest = spec.get(idx + "/referrers/".len()..)?;
            if !name.is_empty() && !digest.is_empty() {
                return Some(Route::Referrers { name, digest });
            }
        }
        if let Some(idx) = spec.rfind("/manifests/") {
            let name = spec.get(..idx)?;
            let reference = spec.get(idx + "/manifests/".len()..)?;
            if !name.is_empty() && !reference.is_empty() {
                return Some(Route::Manifest { name, reference });
            }
        }
        if let Some(idx) = spec.rfind("/blobs/uploads/") {
            let name = spec.get(..idx)?;
            let session = spec.get(idx + "/blobs/uploads/".len()..)?;
            if !name.is_empty() && !session.is_empty() {
                return Some(Route::Upload { name, session });
            }
        }
        if let Some(name) = spec.strip_suffix("/blobs/uploads")
            && !name.is_empty()
        {
            return Some(Route::UploadInit { name });
        }
        if let Some(idx) = spec.rfind("/blobs/") {
            let name = spec.get(..idx)?;
            let digest = spec.get(idx + "/blobs/".len()..)?;
            if !name.is_empty() && !digest.is_empty() {
                return Some(Route::Blob { name, digest });
            }
        }
        None
    }
}

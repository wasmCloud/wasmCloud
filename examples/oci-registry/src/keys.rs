//! Object-key naming scheme. Everything the registry persists lives as an object
//! in a single blobstore container, keyed by repository name.

/// Digests carry a `:` separator (`sha256:<hex>`) which is not portable across
/// blobstore backends (e.g. NATS object store keys), so it is sanitized.
fn sanitize(reference: &str) -> String {
    reference.replace(':', "_")
}

pub(crate) fn blob_key(name: &str, digest: &str) -> String {
    format!("{name}/blobs/{}", sanitize(digest))
}

pub(crate) fn manifest_key(name: &str, digest: &str) -> String {
    format!("{name}/manifests/{}", sanitize(digest))
}

pub(crate) fn media_type_key(manifest_key: &str) -> String {
    format!("{manifest_key}.mediatype")
}

pub(crate) fn tag_key(name: &str, tag: &str) -> String {
    format!("{name}/tags/{tag}")
}

pub(crate) fn upload_key(name: &str, session: &str) -> String {
    format!("{name}/uploads/{session}")
}

pub(crate) fn referrer_prefix(name: &str, subject: &str) -> String {
    format!("{name}/referrers/{}", sanitize(subject))
}

pub(crate) fn referrer_key(name: &str, subject: &str, manifest_digest: &str) -> String {
    format!(
        "{}/{}",
        referrer_prefix(name, subject),
        sanitize(manifest_digest)
    )
}

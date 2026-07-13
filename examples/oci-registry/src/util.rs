//! Small pure helpers: digests, session ids, ranges, and query parsing.

use crate::bindings;
use sha2::{Digest, Sha256};

/// A reference is a digest (rather than a tag) when it carries an `algo:hex`
/// separator; tags cannot contain `:`.
pub(crate) fn is_digest(reference: &str) -> bool {
    reference.contains(':')
}

pub(crate) fn sha256_digest(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

pub(crate) fn new_session_id() -> String {
    let bytes = bindings::wasi::random::random::get_random_bytes(16);
    hex::encode(bytes)
}

pub(crate) fn range_header(len: u64) -> String {
    if len == 0 {
        "0-0".to_string()
    } else {
        format!("0-{}", len - 1)
    }
}

/// Parse the inclusive start offset from a `Content-Range: <start>-<end>` header.
pub(crate) fn range_start(range: &str) -> Option<u64> {
    let range = range.trim();
    let range = range.strip_prefix("bytes ").unwrap_or(range);
    let (start, _) = range.split_once('-')?;
    start.trim().parse::<u64>().ok()
}

/// Extract and percent-decode a query parameter value.
pub(crate) fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

/// Decode `%XX` escapes in a string. Other bytes (including `+`) are left as-is,
/// which keeps media types such as `application/vnd...v1+json` intact.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        if byte == b'%' {
            let high = bytes.get(i + 1).copied().and_then(hex_value);
            let low = bytes.get(i + 2).copied().and_then(hex_value);
            if let (Some(high), Some(low)) = (high, low) {
                out.push(high * 16 + low);
                i += 3;
                continue;
            }
        }
        out.push(byte);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

//! DPoP proofs (RFC 9449) with a key the host holds for one workload.
//!
//! A DPoP-bound token is useless without the key, and the key never leaves the
//! host, so a token copied out of a request cannot be replayed from elsewhere.
//! Each proof is signed over the request as it is finally sent.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_lc_rs::digest::{SHA256, digest};
use aws_lc_rs::encoding::{AsBigEndian, EcPublicKeyUncompressedBin};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;

/// Header carrying the proof, and the one a server returns a nonce in.
pub const DPOP: http::HeaderName = http::HeaderName::from_static("dpop");
pub const DPOP_NONCE: http::HeaderName = http::HeaderName::from_static("dpop-nonce");

/// Most origins one key remembers a nonce for.
const MAX_NONCES: usize = 64;

/// An ES256 key and the nonces servers have handed it, per origin.
pub struct DpopKey {
    pair: EcdsaKeyPair,
    jwk: String,
    thumbprint: String,
    nonces: Mutex<HashMap<String, String>>,
}

impl DpopKey {
    /// A fresh key that exists only in this host's memory.
    pub fn generate() -> anyhow::Result<Self> {
        let pair = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING)
            .map_err(|_| anyhow::anyhow!("could not generate a DPoP key"))?;
        let point: EcPublicKeyUncompressedBin<'_> = pair
            .public_key()
            .as_be_bytes()
            .map_err(|_| anyhow::anyhow!("could not encode the DPoP public key"))?;
        let (x, y) = match point.as_ref() {
            [4, coordinates @ ..] if coordinates.len() == 64 => coordinates.split_at(32),
            _ => anyhow::bail!("unexpected P-256 point"),
        };
        let (x, y) = (B64.encode(x), B64.encode(y));
        // RFC 7638: required members only, in lexicographic order, no spaces.
        let jwk = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);
        let thumbprint = B64.encode(digest(&SHA256, jwk.as_bytes()));
        Ok(Self {
            pair,
            jwk,
            thumbprint,
            nonces: Mutex::default(),
        })
    }

    /// The RFC 7638 thumbprint a bound token carries as `cnf.jkt`.
    pub fn thumbprint(&self) -> &str {
        &self.thumbprint
    }

    /// A proof for one request. `access_token` binds it to the token with
    /// `ath`; a token request has none.
    pub fn proof(
        &self,
        method: &http::Method,
        uri: &http::Uri,
        access_token: Option<&[u8]>,
    ) -> anyhow::Result<http::HeaderValue> {
        let htu = htu(uri)?;
        let iat = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let mut claims = serde_json::Map::new();
        claims.insert("jti".into(), crate::ids::now_v7().to_string().into());
        claims.insert("htm".into(), method.as_str().into());
        claims.insert("htu".into(), htu.into());
        claims.insert("iat".into(), iat.into());
        if let Some(token) = access_token {
            claims.insert("ath".into(), B64.encode(digest(&SHA256, token)).into());
        }
        if let Some(nonce) = self.nonce_for(uri) {
            claims.insert("nonce".into(), nonce.into());
        }
        let header = format!(r#"{{"typ":"dpop+jwt","alg":"ES256","jwk":{}}}"#, self.jwk);
        let signing_input = format!(
            "{}.{}",
            B64.encode(header),
            B64.encode(serde_json::to_vec(&claims)?)
        );
        let signature = self
            .pair
            .sign(&SystemRandom::new(), signing_input.as_bytes())
            .map_err(|_| anyhow::anyhow!("could not sign a DPoP proof"))?;
        let jwt = format!("{signing_input}.{}", B64.encode(signature.as_ref()));
        Ok(http::HeaderValue::try_from(jwt)?)
    }

    /// Remember the nonce `headers` carry for `uri`'s origin. Returns whether
    /// there was one.
    pub fn observe(&self, uri: &http::Uri, headers: &http::HeaderMap) -> bool {
        let Some(nonce) = headers.get(DPOP_NONCE).and_then(|v| v.to_str().ok()) else {
            return false;
        };
        let Some(origin) = origin(uri) else {
            return false;
        };
        let mut nonces = self.nonces.lock().unwrap_or_else(|e| e.into_inner());
        if nonces.len() >= MAX_NONCES && !nonces.contains_key(&origin) {
            nonces.clear();
        }
        nonces.insert(origin, nonce.to_string());
        true
    }

    fn nonce_for(&self, uri: &http::Uri) -> Option<String> {
        let origin = origin(uri)?;
        self.nonces
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&origin)
            .cloned()
    }
}

/// Whether a resource server answered that it wants a fresh nonce.
pub fn wants_nonce(status: http::StatusCode, headers: &http::HeaderMap) -> bool {
    status == http::StatusCode::UNAUTHORIZED
        && headers.contains_key(DPOP_NONCE)
        && headers
            .get_all(http::header::WWW_AUTHENTICATE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(challenges)
            .any(|(scheme, params)| {
                scheme.eq_ignore_ascii_case("dpop")
                    && params
                        .iter()
                        .any(|(k, v)| k.eq_ignore_ascii_case("error") && v == "use_dpop_nonce")
            })
}

#[derive(Debug, PartialEq)]
enum Lexeme {
    Token(String),
    Quoted(String),
    Equals,
    Comma,
}

fn lex(value: &str) -> Vec<Lexeme> {
    let mut out = Vec::new();
    let mut chars = value.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
            }
            ',' => {
                chars.next();
                out.push(Lexeme::Comma);
            }
            '=' => {
                chars.next();
                out.push(Lexeme::Equals);
            }
            '"' => {
                chars.next();
                let mut text = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => text.extend(chars.next()),
                        c => text.push(c),
                    }
                }
                out.push(Lexeme::Quoted(text));
            }
            _ => {
                let mut text = String::new();
                while let Some(&c) = chars.peek() {
                    if matches!(c, ' ' | '\t' | ',' | '=' | '"') {
                        break;
                    }
                    text.push(c);
                    chars.next();
                }
                out.push(Lexeme::Token(text));
            }
        }
    }
    out
}

type Challenge = (String, Vec<(String, String)>);

/// The challenges in one `WWW-Authenticate` value (RFC 9110 §11.6.1), each
/// a scheme and its parameters. A token68 is skipped.
fn challenges(value: &str) -> Vec<Challenge> {
    let lexemes = lex(value);
    let mut out: Vec<Challenge> = Vec::new();
    let mut i = 0;
    while let Some(lexeme) = lexemes.get(i) {
        let Lexeme::Token(name) = lexeme else {
            i += 1;
            continue;
        };
        let param = match (lexemes.get(i + 1), lexemes.get(i + 2)) {
            (Some(Lexeme::Equals), Some(Lexeme::Token(v) | Lexeme::Quoted(v))) => Some(v),
            _ => None,
        };
        match (param, out.last_mut()) {
            (Some(v), Some((_, params))) => {
                params.push((name.clone(), v.clone()));
                i += 3;
            }
            (Some(_), None) => i += 3,
            (None, _) => {
                out.push((name.clone(), Vec::new()));
                i += 1;
                while matches!(lexemes.get(i), Some(Lexeme::Equals)) {
                    i += 1;
                }
            }
        }
    }
    out
}

/// The request URI without query or fragment, as `htu` requires, in the one
/// form a server compares it in.
fn htu(uri: &http::Uri) -> anyhow::Result<String> {
    let origin =
        origin(uri).ok_or_else(|| anyhow::anyhow!("a DPoP proof needs an absolute URI"))?;
    Ok(format!("{origin}{}", uri.path()))
}

/// Lowercase scheme and host, the default port dropped, no userinfo.
fn origin(uri: &http::Uri) -> Option<String> {
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    let authority = uri.authority().filter(|a| !a.as_str().contains('@'))?;
    let host = authority.host().to_ascii_lowercase();
    let default = match scheme.as_str() {
        "https" => Some(443),
        "http" => Some(80),
        _ => None,
    };
    Some(match authority.port_u16() {
        Some(port) if Some(port) != default => format!("{scheme}://{host}:{port}"),
        _ => format!("{scheme}://{host}"),
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    fn decode(part: &str) -> serde_json::Value {
        serde_json::from_slice(&B64.decode(part).unwrap()).unwrap()
    }

    #[test]
    fn a_proof_covers_the_final_method_and_url_and_binds_the_token() {
        let key = DpopKey::generate().unwrap();
        let uri: http::Uri = "https://api.example.com/v1/items?page=2#x".parse().unwrap();
        let proof = key.proof(&http::Method::POST, &uri, Some(b"tok")).unwrap();
        let proof = proof.to_str().unwrap();
        let parts: Vec<&str> = proof.split('.').collect();
        assert_eq!(parts.len(), 3);

        let header = decode(parts[0]);
        assert_eq!(header["typ"], "dpop+jwt");
        assert_eq!(header["alg"], "ES256");
        let claims = decode(parts[1]);
        assert_eq!(claims["htm"], "POST");
        assert_eq!(claims["htu"], "https://api.example.com/v1/items");
        assert_eq!(claims["ath"], B64.encode(digest(&SHA256, b"tok")));
        assert!(claims.get("nonce").is_none());

        let jwk = &header["jwk"];
        let mut point = vec![4u8];
        point.extend(B64.decode(jwk["x"].as_str().unwrap()).unwrap());
        point.extend(B64.decode(jwk["y"].as_str().unwrap()).unwrap());
        let signed = format!("{}.{}", parts[0], parts[1]);
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &point)
            .verify(signed.as_bytes(), &B64.decode(parts[2]).unwrap())
            .unwrap();
    }

    #[test]
    fn a_nonce_is_kept_per_origin_and_sent_on_the_next_proof() {
        let key = DpopKey::generate().unwrap();
        let api: http::Uri = "https://api.example.com/a".parse().unwrap();
        let other: http::Uri = "https://other.example.com/a".parse().unwrap();
        let mut headers = http::HeaderMap::new();
        headers.insert(DPOP_NONCE, "n-1".parse().unwrap());
        assert!(key.observe(&api, &headers));

        let with = key.proof(&http::Method::GET, &api, None).unwrap();
        let claims = decode(with.to_str().unwrap().split('.').nth(1).unwrap());
        assert_eq!(claims["nonce"], "n-1");
        let without = key.proof(&http::Method::GET, &other, None).unwrap();
        let claims = decode(without.to_str().unwrap().split('.').nth(1).unwrap());
        assert!(claims.get("nonce").is_none());
    }

    #[test]
    fn recognizes_a_nonce_challenge() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::WWW_AUTHENTICATE,
            r#"DPoP error="use_dpop_nonce""#.parse().unwrap(),
        );
        assert!(!wants_nonce(http::StatusCode::UNAUTHORIZED, &headers));
        headers.insert(DPOP_NONCE, "n".parse().unwrap());
        assert!(wants_nonce(http::StatusCode::UNAUTHORIZED, &headers));
        assert!(!wants_nonce(http::StatusCode::FORBIDDEN, &headers));
    }

    #[test]
    fn challenges_are_parsed_rather_than_searched() {
        let mut headers = http::HeaderMap::new();
        headers.insert(DPOP_NONCE, "n".parse().unwrap());
        let wants = |value: &'static str| {
            let mut headers = headers.clone();
            headers.insert(http::header::WWW_AUTHENTICATE, value.parse().unwrap());
            wants_nonce(http::StatusCode::UNAUTHORIZED, &headers)
        };
        assert!(wants(r#"dpop error="use_dpop_nonce""#));
        assert!(wants(
            r#"Bearer realm="api", DPoP algs="ES256", error=use_dpop_nonce"#
        ));
        assert!(!wants(r#"Basic realm="use_dpop_nonce""#));
        assert!(!wants(
            r#"DPoP error="invalid_token", error_description="use_dpop_nonce""#
        ));
        assert!(!wants(r#"DPoP error="use_dpop_nonce_x""#));
        assert!(!wants(r#"Bearer error="use_dpop_nonce""#));
    }

    #[test]
    fn htu_and_origin_are_normalized() {
        let key = DpopKey::generate().unwrap();
        let uri: http::Uri = "HTTPS://API.Example.com:443/v1/Items".parse().unwrap();
        let proof = key.proof(&http::Method::GET, &uri, None).unwrap();
        let claims = decode(proof.to_str().unwrap().split('.').nth(1).unwrap());
        assert_eq!(claims["htu"], "https://api.example.com/v1/Items");
        let nonstandard: http::Uri = "https://api.example.com:8443/".parse().unwrap();
        assert_eq!(
            origin(&nonstandard).unwrap(),
            "https://api.example.com:8443"
        );
        let userinfo: http::Uri = "https://u@api.example.com/".parse().unwrap();
        assert!(origin(&userinfo).is_none());
    }

    #[test]
    fn the_thumbprint_is_over_the_canonical_jwk() {
        let key = DpopKey::generate().unwrap();
        assert_eq!(
            key.thumbprint,
            B64.encode(digest(&SHA256, key.jwk.as_bytes()))
        );
        assert_eq!(key.thumbprint().len(), 43);
    }
}

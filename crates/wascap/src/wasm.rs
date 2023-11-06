//! Functions for extracting and embedding claims within a WebAssembly module

use crate::{
    errors::{self, ErrorKind},
    jwt::{Actor, Claims, Token, MIN_WASCAP_INTERNAL_REVISION},
    Result,
};
use data_encoding::HEXUPPER;
use nkeys::KeyPair;
use ring::digest::{Context, Digest, SHA256};
use std::{
    io::Read,
    mem,
    time::{SystemTime, UNIX_EPOCH},
};
use wasm_encoder::ComponentSectionId;
use wasm_encoder::Encode;
use wasm_encoder::Section;
use wasmparser::Parser;
const SECS_PER_DAY: u64 = 86400;
const SECTION_JWT: &str = "jwt"; // Versions of wascap prior to 0.9 used this section
const SECTION_WC_JWT: &str = "wasmcloud_jwt";

/// Extracts a set of claims from the raw bytes of a WebAssembly module. In the case where no
/// JWT is discovered in the module, this function returns `None`.
/// If there is a token in the file with a valid hash, then you will get a `Token` back
/// containing both the raw JWT and the decoded claims.
///
/// # Errors
/// Will return an error if hash computation fails or it can't read the JWT from inside
/// a section's data, etc
pub fn extract_claims(contents: impl AsRef<[u8]>) -> Result<Option<Token<Actor>>> {
    use wasmparser::Payload::{ComponentSection, CustomSection, End, ModuleSection};

    let target_hash = compute_hash(&strip_custom_section(contents.as_ref())?)?;
    let parser = wasmparser::Parser::new(0);
    let mut depth = 0;
    for payload in parser.parse_all(contents.as_ref()) {
        let payload = payload?;
        match payload {
            ModuleSection { .. } | ComponentSection { .. } => depth += 1,
            End { .. } => depth -= 1,
            CustomSection(c)
                if (c.name() == SECTION_JWT) || (c.name() == SECTION_WC_JWT) && depth == 0 =>
            {
                let jwt = String::from_utf8(c.data().to_vec())?;
                let claims: Claims<Actor> = Claims::decode(&jwt)?;
                let Some(ref meta) = claims.metadata else {
                    return Err(errors::new(ErrorKind::InvalidAlgorithm));
                };
                if meta.module_hash != target_hash
                    && claims.wascap_revision.unwrap_or_default() >= MIN_WASCAP_INTERNAL_REVISION
                {
                    return Err(errors::new(ErrorKind::InvalidModuleHash));
                }
                return Ok(Some(Token { jwt, claims }));
            }
            _ => {}
        }
    }
    Ok(None)
}

/// This function will embed a set of claims inside the bytecode of a WebAssembly module. The claims
/// are converted into a JWT and signed using the provided `KeyPair`.
/// According to the WebAssembly [custom section](https://webassembly.github.io/spec/core/appendix/custom.html)
/// specification, arbitary sets of bytes can be stored in a WebAssembly module without impacting
/// parsers or interpreters. Returns a vector of bytes representing the new WebAssembly module which can
/// be saved to a `.wasm` file
#[allow(clippy::missing_errors_doc)] // TODO: document errors
pub fn embed_claims(orig_bytecode: &[u8], claims: &Claims<Actor>, kp: &KeyPair) -> Result<Vec<u8>> {
    let mut bytes = orig_bytecode.to_vec();
    bytes = strip_custom_section(&bytes)?;

    let hash = compute_hash(&bytes)?;
    let mut claims = (*claims).clone();
    let meta = claims.metadata.map(|md| Actor {
        module_hash: hash,
        ..md
    });
    claims.metadata = meta;

    let encoded = claims.encode(kp)?;
    let encvec = encoded.as_bytes().to_vec();
    wasm_gen::write_custom_section(&mut bytes, SECTION_WC_JWT, &encvec);

    Ok(bytes)
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::missing_errors_doc)] // TODO: document
pub fn sign_buffer_with_claims(
    name: String,
    buf: impl AsRef<[u8]>,
    mod_kp: &KeyPair,
    acct_kp: &KeyPair,
    expires_in_days: Option<u64>,
    not_before_days: Option<u64>,
    caps: Vec<String>,
    tags: Vec<String>,
    provider: bool,
    rev: Option<i32>,
    ver: Option<String>,
    call_alias: Option<String>,
) -> Result<Vec<u8>> {
    let claims = Claims::<Actor>::with_dates(
        name,
        acct_kp.public_key(),
        mod_kp.public_key(),
        Some(caps),
        Some(tags),
        days_from_now_to_jwt_time(not_before_days),
        days_from_now_to_jwt_time(expires_in_days),
        provider,
        rev,
        ver,
        call_alias,
    );
    embed_claims(buf.as_ref(), &claims, acct_kp)
}

pub(crate) fn strip_custom_section(buf: &[u8]) -> Result<Vec<u8>> {
    use wasmparser::Payload::{ComponentSection, CustomSection, End, ModuleSection, Version};

    let mut output: Vec<u8> = Vec::new();
    let mut stack = Vec::new();
    for payload in Parser::new(0).parse_all(buf) {
        let payload = payload?;
        match payload {
            Version { encoding, .. } => {
                output.extend_from_slice(match encoding {
                    wasmparser::Encoding::Component => &wasm_encoder::Component::HEADER,
                    wasmparser::Encoding::Module => &wasm_encoder::Module::HEADER,
                });
            }
            ModuleSection { .. } | ComponentSection { .. } => {
                stack.push(mem::take(&mut output));
                continue;
            }
            End { .. } => {
                let Some(mut parent) = stack.pop() else { break };
                if output.starts_with(&wasm_encoder::Component::HEADER) {
                    parent.push(ComponentSectionId::Component as u8);
                    output.encode(&mut parent);
                } else {
                    parent.push(ComponentSectionId::CoreModule as u8);
                    output.encode(&mut parent);
                }
                output = parent;
            }
            _ => {}
        }

        match payload {
            CustomSection(c) if (c.name() == SECTION_JWT) || (c.name() == SECTION_WC_JWT) => {
                // skip
            }
            _ => {
                if let Some((id, range)) = payload.as_section() {
                    if range.end <= buf.len() {
                        wasm_encoder::RawSection {
                            id,
                            data: &buf[range],
                        }
                        .append_to(&mut output);
                    } else {
                        return Err(errors::new(ErrorKind::IO(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "Invalid section range",
                        ))));
                    }
                }
            }
        }
    }

    Ok(output)
}

fn since_the_epoch() -> std::time::Duration {
    let start = SystemTime::now();
    start
        .duration_since(UNIX_EPOCH)
        .expect("A timey wimey problem has occurred!")
}

#[must_use]
pub fn days_from_now_to_jwt_time(stamp: Option<u64>) -> Option<u64> {
    stamp.map(|e| since_the_epoch().as_secs() + e * SECS_PER_DAY)
}

fn sha256_digest<R: Read>(mut reader: R) -> Result<Digest> {
    let mut context = Context::new(&SHA256);
    let mut buffer = [0; 1024];

    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        context.update(&buffer[..count]);
    }

    Ok(context.finish())
}

fn compute_hash(modbytes: &[u8]) -> Result<String> {
    let digest = sha256_digest(modbytes)?;
    Ok(HEXUPPER.encode(digest.as_ref()))
}

#[cfg(test)]
mod test {
    use std::fs::File;

    use super::*;
    use crate::{
        caps::capability_name,
        caps::{KEY_VALUE, LOGGING, MESSAGING},
        jwt::{Actor, Claims, WASCAP_INTERNAL_REVISION},
    };
    use data_encoding::BASE64;

    const WASM_BASE64: &str =
        "AGFzbQEAAAAADAZkeWxpbmuAgMACAAGKgICAAAJgAn9/AX9gAAACwYCAgAAEA2VudgptZW1vcnlCYXNl\
         A38AA2VudgZtZW1vcnkCAIACA2VudgV0YWJsZQFwAAADZW52CXRhYmxlQmFzZQN/AAOEgICAAAMAAQEGi\
         4CAgAACfwFBAAt/AUEACwejgICAAAIKX3RyYW5zZm9ybQAAEl9fcG9zdF9pbnN0YW50aWF0ZQACCYGAgI\
         AAAArpgICAAAPBgICAAAECfwJ/IABBAEoEQEEAIQIFIAAPCwNAIAEgAmoiAywAAEHpAEYEQCADQfkAOgA\
         ACyACQQFqIgIgAEcNAAsgAAsLg4CAgAAAAQuVgICAAAACQCMAJAIjAkGAgMACaiQDEAELCw==";

    #[test]
    fn strip_custom() {
        let mut f = File::open("./fixtures/guest.component.wasm").unwrap();
        let mut buffer = Vec::new();
        f.read_to_end(&mut buffer).unwrap();

        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "testing".to_string(),
                Some(vec![MESSAGING.to_string(), LOGGING.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };
        let modified_bytecode = embed_claims(&buffer, &claims, &kp).unwrap();

        super::strip_custom_section(&modified_bytecode).unwrap();
    }

    #[test]
    fn legacy_modules_still_extract() {
        // Ensure that we can still extract claims from legacy (signed prior to 0.9.0) modules without
        // a hash violation error
        let mut f = File::open("./fixtures/logger.wasm").unwrap();
        let mut buffer = Vec::new();
        f.read_to_end(&mut buffer).unwrap();

        let t = extract_claims(&buffer).unwrap();
        assert!(t.is_some());
    }

    #[test]
    fn decode_wasi_preview() {
        let mut f = File::open("./fixtures/guest.component.wasm").unwrap();
        let mut buffer = Vec::new();
        f.read_to_end(&mut buffer).unwrap();

        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "testing".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };
        let modified_bytecode = embed_claims(&buffer, &claims, &kp).unwrap();

        if let Some(token) = extract_claims(modified_bytecode).unwrap() {
            assert_eq!(claims.issuer, token.claims.issuer);
            assert_eq!(
                claims.metadata.as_ref().unwrap().caps,
                token.claims.metadata.as_ref().unwrap().caps
            );
        } else {
            unreachable!()
        }
    }

    #[test]
    fn claims_roundtrip() {
        // Serialize and de-serialize this because the module loader adds bytes to
        // the above base64 encoded module.
        let dec_module = BASE64.decode(WASM_BASE64.as_bytes()).unwrap();

        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "testing".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };
        let modified_bytecode = embed_claims(&dec_module, &claims, &kp).unwrap();

        if let Some(token) = extract_claims(modified_bytecode).unwrap() {
            assert_eq!(claims.issuer, token.claims.issuer);
            assert_eq!(
                claims.metadata.as_ref().unwrap().caps,
                token.claims.metadata.as_ref().unwrap().caps
            );
        } else {
            unreachable!()
        }
    }

    #[test]
    fn claims_doublesign_roundtrip() {
        // Verify that we can sign a previously signed module by stripping the old
        // custom JWT and maintaining valid hashes
        let dec_module = BASE64.decode(WASM_BASE64.as_bytes()).unwrap();

        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "testing".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };
        let c2 = claims.clone();
        let modified_bytecode = embed_claims(&dec_module, &claims, &kp).unwrap();

        let new_claims = Claims {
            subject: "altered.wasm".to_string(),
            ..claims
        };

        let modified_bytecode2 = embed_claims(&modified_bytecode, &new_claims, &kp).unwrap();
        if let Some(token) = extract_claims(modified_bytecode2).unwrap() {
            assert_eq!(c2.issuer, token.claims.issuer);
            assert_eq!(token.claims.subject, "altered.wasm");
        } else {
            unreachable!()
        }
    }

    #[test]
    fn claims_logging_roundtrip() {
        // Serialize and de-serialize this because the module loader adds bytes to
        // the above base64 encoded module.
        let dec_module = BASE64.decode(WASM_BASE64.as_bytes()).unwrap();

        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "testing".to_string(),
                Some(vec![capability_name(MESSAGING), capability_name(LOGGING)]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                Some("somealias".to_string()),
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };
        let modified_bytecode = embed_claims(&dec_module, &claims, &kp).unwrap();

        if let Some(token) = extract_claims(modified_bytecode).unwrap() {
            assert_eq!(claims.issuer, token.claims.issuer);
            assert_eq!(claims.subject, token.claims.subject);

            let claims_met = claims.metadata.as_ref().unwrap();
            let token_met = token.claims.metadata.as_ref().unwrap();

            assert_eq!(claims_met.caps, token_met.caps);
            assert_eq!(claims_met.call_alias, token_met.call_alias);
        } else {
            unreachable!()
        }
    }
}

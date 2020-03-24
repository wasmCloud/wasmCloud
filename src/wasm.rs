// Copyright 2015-2018 Capital One Services, LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Functions for extracting and embedding claims within a WebAssembly module

use crate::errors::{self, ErrorKind};
use crate::jwt::Token;
use crate::jwt::{Actor, Claims};
use crate::Result;
use data_encoding::HEXUPPER;
use nkeys::KeyPair;
use parity_wasm::elements::CustomSection;
use parity_wasm::{
    deserialize_buffer,
    elements::{Module, Serialize},
    serialize,
};
use ring::digest::{Context, Digest, SHA256};
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};
const SECS_PER_DAY: u64 = 86400;

/// Extracts a set of claims from the raw bytes of a WebAssembly module. In the case where no
/// JWT is discovered in the module, this function returns `None`.
/// If there is a token in the file with a valid hash, then you will get a `Token` back
/// containing both the raw JWT and the decoded claims.
///
/// # Errors
/// Will return errors if the file cannot be read, cannot be parsed, contains an improperly
/// forms JWT, or the `module_hash` claim inside the decoded JWT does not match the hash
/// of the file.
pub fn extract_claims(contents: impl AsRef<[u8]>) -> Result<Option<Token<Actor>>> {
    let module: Module = deserialize_buffer(contents.as_ref())?;

    let sections: Vec<&CustomSection> = module
        .custom_sections()
        .filter(|sect| sect.name() == "jwt")
        .collect();

    if sections.is_empty() {
        Ok(None)
    } else {
        let jwt = String::from_utf8(sections[0].payload().to_vec())?;
        let claims: Claims<Actor> = Claims::decode(&jwt)?;
        let hash = compute_hash_without_jwt(module)?;

        if let Some(ref meta) = claims.metadata {
            if meta.module_hash != hash {
                Err(errors::new(ErrorKind::InvalidModuleHash))
            } else {
                Ok(Some(Token { jwt, claims }))
            }
        } else {
            Err(errors::new(ErrorKind::InvalidAlgorithm))
        }
    }
}

/// This function will embed a set of claims inside the bytecode of a WebAssembly module. The claims
/// are converted into a JWT and signed using the provided `KeyPair`.
/// According to the WebAssembly [custom section](https://webassembly.github.io/spec/core/appendix/custom.html)
/// specification, arbitary sets of bytes can be stored in a WebAssembly module without impacting
/// parsers or interpreters. Returns a vector of bytes representing the new WebAssembly module which can
/// be saved to a `.wasm` file
pub fn embed_claims(orig_bytecode: &[u8], claims: &Claims<Actor>, kp: &KeyPair) -> Result<Vec<u8>> {
    let mut module: Module = deserialize_buffer(orig_bytecode)?;
    module.clear_custom_section("jwt");
    let cleanbytes = serialize(module)?;

    let digest = sha256_digest(cleanbytes.as_slice())?;
    let mut claims = (*claims).clone();
    let meta = claims.metadata.map(|md| Actor {
        module_hash: HEXUPPER.encode(digest.as_ref()),
        ..md
    });
    claims.metadata = meta;

    let encoded = claims.encode(&kp)?;
    let encvec = encoded.as_bytes().to_vec();
    let mut m: Module = deserialize_buffer(orig_bytecode)?;
    m.set_custom_section("jwt", encvec);
    let mut buf = Vec::new();
    m.serialize(&mut buf)?;

    Ok(buf)
}

pub fn sign_buffer_with_claims(
    name: String,
    buf: impl AsRef<[u8]>,
    mod_kp: KeyPair,
    acct_kp: KeyPair,
    expires_in_days: Option<u64>,
    not_before_days: Option<u64>,
    caps: Vec<String>,
    tags: Vec<String>,
    provider: bool,
    rev: Option<i32>,
    ver: Option<String>,
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
    );
    embed_claims(buf.as_ref(), &claims, &acct_kp)
}

fn since_the_epoch() -> std::time::Duration {
    let start = SystemTime::now();
    start
        .duration_since(UNIX_EPOCH)
        .expect("A timey wimey problem has occurred!")
}

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

fn compute_hash_without_jwt(module: Module) -> Result<String> {
    let mut refmod = module;
    refmod.clear_custom_section("jwt");
    let modbytes = serialize(refmod)?;

    let digest = sha256_digest(modbytes.as_slice())?;
    Ok(HEXUPPER.encode(digest.as_ref()))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::caps::{KEY_VALUE, MESSAGING,LOGGING};
    use crate::jwt::{Actor, Claims};
    use base64::decode;
    use parity_wasm::serialize;

    const WASM_BASE64: &str =
        "AGFzbQEAAAAADAZkeWxpbmuAgMACAAGKgICAAAJgAn9/AX9gAAACwYCAgAAEA2VudgptZW1vcnlCYXNl\
         A38AA2VudgZtZW1vcnkCAIACA2VudgV0YWJsZQFwAAADZW52CXRhYmxlQmFzZQN/AAOEgICAAAMAAQEGi\
         4CAgAACfwFBAAt/AUEACwejgICAAAIKX3RyYW5zZm9ybQAAEl9fcG9zdF9pbnN0YW50aWF0ZQACCYGAgI\
         AAAArpgICAAAPBgICAAAECfwJ/IABBAEoEQEEAIQIFIAAPCwNAIAEgAmoiAywAAEHpAEYEQCADQfkAOgA\
         ACyACQQFqIgIgAEcNAAsgAAsLg4CAgAAAAQuVgICAAAACQCMAJAIjAkGAgMACaiQDEAELCw==";

    #[test]
    fn claims_roundtrip() {
        // Serialize and de-serialize this because the module loader adds bytes to
        // the above base64 encoded module.
        let dec_module = decode(WASM_BASE64).unwrap();
        let m: Module = deserialize_buffer(&dec_module).unwrap();
        let raw_module = serialize(m).unwrap();

        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "testing".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some("".to_string()),
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
        };
        let modified_bytecode = embed_claims(&raw_module, &claims, &kp).unwrap();
        println!(
            "Added {} bytes in custom section.",
            modified_bytecode.len() - raw_module.len()
        );
        if let Some(token) = extract_claims(&modified_bytecode).unwrap() {
            assert_eq!(claims.issuer, token.claims.issuer);
        /*     assert_eq!(
            claims.metadata.as_ref().unwrap().caps,
            token.claims.metadata.as_ref().unwrap().caps
        );
        */
        /* assert_ne!(
            claims.metadata.as_ref().unwrap().module_hash,
            token.claims.metadata.as_ref().unwrap().module_hash
        );
        */
        } else {
            unreachable!()
        }
    }

    #[test]
    fn claims_logging_roundtrip() {
        // Serialize and de-serialize this because the module loader adds bytes to
        // the above base64 encoded module.
        let dec_module = decode(WASM_BASE64).unwrap();
        let m: Module = deserialize_buffer(&dec_module).unwrap();
        let raw_module = serialize(m).unwrap();

        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "testing".to_string(),
                Some(vec![MESSAGING.to_string(), LOGGING.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some("".to_string()),
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
        };
        let modified_bytecode = embed_claims(&raw_module, &claims, &kp).unwrap();
        println!(
            "Added {} bytes in custom section.",
            modified_bytecode.len() - raw_module.len()
        );
        if let Some(token) = extract_claims(&modified_bytecode).unwrap() {
            assert_eq!(claims.issuer, token.claims.issuer);
        /*     assert_eq!(
            claims.metadata.as_ref().unwrap().caps,
            token.claims.metadata.as_ref().unwrap().caps
        );
        */
        /* assert_ne!(
            claims.metadata.as_ref().unwrap().module_hash,
            token.claims.metadata.as_ref().unwrap().module_hash
        );
        */
        } else {
            unreachable!()
        }
    }
}

use std::{
    collections::HashSet,
    error::Error,
    sync::{Arc, RwLock},
};

use serde_json::Value;
use url::{ParseError, Url};
use wascap::prelude::*;
use wasmcloud_host::Authorizer;

/// The NATS account server authorizer checks the claims of actors being loaded against a specific
/// NATS account server. If the server returns the decoded claims and passes the check (unexpired token),
/// then this authorizer is satisfied that the account can load and will cache the account claims so that
/// any actor issued by a valid account will be allowed to invoke
#[derive(Clone)]
pub struct NatsAccountServerAuthorizer {
    pub root_url: Url,
    claims_cache: Arc<RwLock<HashSet<String>>>,
}

impl NatsAccountServerAuthorizer {
    /// Create a new account server with a root URL. The root URL should contain the protocol, port,
    /// and the API version, e.g. `https://my.server:8080/jwt/v1`.
    pub fn new(root_url: &str) -> std::result::Result<Self, ParseError> {
        let root_url = Url::parse(root_url)?;
        Ok(NatsAccountServerAuthorizer {
            root_url,
            claims_cache: Arc::new(RwLock::new(HashSet::new())),
        })
    }

    fn claims_for_issuer(&self, issuer: &str) -> std::result::Result<Value, Box<dyn Error>> {
        let url = self
            .root_url
            .join(&format!("accounts/{}?check=true&decode=true", issuer))?;
        let resp = reqwest::blocking::get(url.as_ref())?;
        if resp.status().is_success() {
            let claims: Value = serde_json::from_str(&resp.text()?)?;
            Ok(claims)
        } else {
            Err(format!("HTTP request failed: {:?}", resp.status()).into())
        }
    }
}

impl Default for NatsAccountServerAuthorizer {
    fn default() -> Self {
        NatsAccountServerAuthorizer {
            root_url: Url::parse("http://localhost:8080/jwt/v1").unwrap(),
            claims_cache: Arc::new(RwLock::new(HashSet::new())),
        }
    }
}

impl Authorizer for NatsAccountServerAuthorizer {
    fn can_load(&self, claims: &Claims<Actor>) -> bool {
        if let Ok(_c) = self.claims_for_issuer(&claims.issuer) {
            self.claims_cache
                .write()
                .unwrap()
                .insert(claims.issuer.to_string());
            true
        } else {
            false
        }
    }

    fn can_invoke(
        &self,
        claims: &Claims<Actor>,
        _target: &wasmcloud_host::WasmCloudEntity,
        _operation: &str,
    ) -> bool {
        self.claims_cache.read().unwrap().contains(&claims.issuer)
    }
}

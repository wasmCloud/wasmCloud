//! Lattice Cache
//! The lattice cache is a strongly-typed wrapper around the use of a `wasmcloud:keyvalue`
//! capability provider to manage the following information that needs to be replicated
//! among all members of a lattice (or simply stored in-process for a standalone host)
//!
//! * mapping of OCI image references to public keys (OCI ref -> 'Mxxx' or 'Vxxx')
//! * claims cache, a lookup table keyed by actor public key containing their Claims<Actor> values
//! * link cache, a lookup table keyed by actor public key+provider public key+provider link name, containing link configuration values

use crate::generated::core::{deserialize, serialize};
use crate::messagebus::LinkDefinition;
use crate::{Invocation, Result, WasmCloudEntity, SYSTEM_ACTOR};
use actix::Recipient;
use wasmcloud_actor_keyvalue::{
    DelArgs, GetArgs, GetResponse, SetAddArgs, SetArgs, SetQueryArgs, SetQueryResponse,
    SetRemoveArgs, OP_DEL, OP_GET, OP_SET, OP_SET_ADD, OP_SET_QUERY, OP_SET_REMOVE,
};

use serde::Deserialize;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use wascap::jwt::Claims;
use wascap::prelude::KeyPair;

pub(crate) const CACHE_PROVIDER_LINK_NAME: &str = "__wasmcloud_lattice_cache";
pub(crate) const CACHE_CONTRACT_ID: &str = "wasmcloud:keyvalue";
const CACHE_KEY_PREFIX: &str = "wclc";

/// A lattice cache client is a strongly-typed abstraction around the specific types of
/// cache operations required to operate the lattice. This client will use the message
/// bus to communicate with a key-value capability provider. There will be multiple
/// lattice cache clients throughout the wasmCloud host, but only a single capability
/// provider host actor running with the wasmCloud lattice cache link name.
#[derive(Debug, Clone)]
pub(crate) struct LatticeCacheClient {
    host: Arc<KeyPair>,
    provider: Recipient<Invocation>,
    cache_provider_id: String,
}

impl LatticeCacheClient {
    /// Creates a new lattice cache. The cache needs to be able to produce invocations
    /// to talk to the configured key-value capability provider and so needs both
    /// a host signing key pair and the provider's public key.
    pub fn new(
        host: KeyPair,
        provider: Recipient<Invocation>,
        provider_id: &str,
    ) -> LatticeCacheClient {
        LatticeCacheClient {
            host: Arc::new(host),
            provider,
            cache_provider_id: provider_id.to_string(),
        }
    }

    /// Check if a given OCI mapping is present without actually looking up the value. This
    /// function will return false if any errors happen while communicating with the message
    /// bus.
    pub async fn contains_oci_mapping(&self, oci_ref: &str) -> bool {
        let key = prefix("ocis");
        let args = SetQueryArgs { key };
        let inv = self.invocation_for_provider(OP_SET_QUERY, &serialize(&args).unwrap());
        invoke_as::<SetQueryResponse>(&self.provider, inv)
            .await
            .map(|qr| qr.values.contains(&oci_ref.to_string()))
            .unwrap_or(false)
    }

    /// Adds an OCI reference mapping to the cache. The OCI reference is an OCI compliant
    /// URL, while the public key is the public key of either a capability provider or
    /// an actor
    pub async fn put_oci_mapping(&self, oci_ref: &str, public_key: &str) -> Result<()> {
        // KV context: KEY is {prefix}:oci_{oci_ref}, VAL is public key
        // SET KEY: {prefix}:ocis, SET MEMBERS: oci_ref
        let key = oci_key(oci_ref);
        let args = SetArgs {
            key,
            value: public_key.to_string(),
            expires: 0,
        };
        let inv = self.invocation_for_provider(OP_SET, &serialize(&args)?);
        let inv_r = self.provider.send(inv).await?;
        if let Some(e) = inv_r.error {
            let s = format!("Failed to put OCI mapping: {}", e);
            error!("{}", s);
            return Err(s.into());
        }

        let key = prefix("ocis");
        let args = SetAddArgs {
            key,
            value: oci_ref.to_string(),
        };
        let inv = self.invocation_for_provider(OP_SET_ADD, &serialize(&args)?);
        let inv_r = self.provider.send(inv).await?;
        if let Some(e) = inv_r.error {
            let s = format!("Failed to put OCI mapping(set): {}", e);
            error!("{}", s);
            Err(e.into())
        } else {
            Ok(())
        }
    }

    /// Adds a call alias mapping to the cache. Call aliases are lattice-unique strings that can
    /// be used as developer-friendly handles for invoking actors, especially useful when the
    /// public key can change from one environment to another.
    pub async fn put_call_alias(&self, alias: &str, actor_key: &str) -> Result<()> {
        if self.lookup_call_alias(alias).await?.is_some() {
            let s = format!("Cannot put call alias, {} is already claimed", alias);
            error!("{}", s);
            return Err(s.into());
        }
        let key = call_alias_key(alias);
        let args = SetArgs {
            key,
            value: actor_key.to_string(),
            expires: 0,
        };
        let inv = self.invocation_for_provider(OP_SET, &serialize(&args)?);
        let inv_r = self.provider.send(inv).await?;
        if let Some(e) = inv_r.error {
            let s = format!("Failed to put call alias mapping: {}", e);
            error!("{}", s);
            return Err(s.into());
        }
        Ok(())
    }

    /// Checks if a given call alias has been claimed and returns the corresponding actor's
    /// public key if so
    pub async fn lookup_call_alias(&self, alias: &str) -> Result<Option<String>> {
        let key = call_alias_key(alias);
        let args = GetArgs { key };
        let inv = self.invocation_for_provider(OP_GET, &serialize(&args)?);
        let gr: GetResponse = invoke_as(&self.provider, inv).await?;
        if gr.exists {
            Ok(Some(gr.value))
        } else {
            Ok(None)
        }
    }

    pub async fn remove_oci_mapping(&self, oci_ref: &str) -> Result<Option<String>> {
        if let Some(s) = self.lookup_oci_mapping(oci_ref).await? {
            self.remove_oci(oci_ref).await?;
            Ok(Some(s))
        } else {
            Ok(None)
        }
    }

    /// Retrieves the public key (if one exists) corresponding to the supplied OCI image
    /// reference
    pub async fn lookup_oci_mapping(&self, oci_ref: &str) -> Result<Option<String>> {
        let key = oci_key(oci_ref);
        let args = GetArgs { key };
        let inv = self.invocation_for_provider(OP_GET, &serialize(&args)?);
        let gr: GetResponse = invoke_as(&self.provider, inv).await?;
        if gr.exists {
            Ok(Some(gr.value))
        } else {
            Ok(None)
        }
    }

    /// Retrieves the list of all currently known OCI image references. You can use this list in a loop to
    /// retrieve all of the image reference mappings.
    pub async fn get_oci_references(&self) -> Result<Vec<String>> {
        let key = prefix("ocis");
        let args = SetQueryArgs { key };
        let inv = self.invocation_for_provider(OP_SET_QUERY, &serialize(&args)?);
        let smembers: SetQueryResponse = invoke_as(&self.provider, inv).await?;
        Ok(smembers.values)
    }

    /// Produces a hashmap of OCI reference -> public key for all known references. This is equivalent
    /// to querying the references using [get_oci_references] and then performing [lookup_oci_mapping]
    /// for each item returned.
    pub async fn collect_oci_references(&self) -> HashMap<String, String> {
        let mut hm = HashMap::new();
        let keys = self.get_oci_references().await;
        if let Ok(keys) = keys {
            for key in keys {
                if let Some(pk) = self.lookup_oci_mapping(&key).await.unwrap_or(None) {
                    hm.insert(key.to_string(), pk.to_string());
                }
            }
        }

        hm
    }

    /// Sets the claims that correspond to a given actor ID.
    pub async fn put_claims(
        &self,
        actor_id: &str,
        claims: Claims<wascap::jwt::Actor>,
    ) -> Result<()> {
        // KV context: KEY is {prefix}:claims_{actor_id}
        // value is claims JSON
        let key = prefix(&format!("claims_{}", actor_id));
        let args = SetArgs {
            key,
            value: serde_json::to_string(&claims)?,
            expires: 0,
        };
        let inv = self.invocation_for_provider(OP_SET, &serialize(&args)?);
        let inv_r = self.provider.send(inv).await?;
        if let Some(e) = inv_r.error {
            let s = format!("Failed to put claims {}", e);
            error!("{}", e);
            return Err(s.into());
        }

        let key = prefix("claims");
        let args = SetAddArgs {
            key,
            value: actor_id.to_string(),
        };
        let inv = self.invocation_for_provider(OP_SET_ADD, &serialize(&args)?);
        let inv_r = self.provider.send(inv).await?;
        if let Some(e) = inv_r.error {
            let s = format!("Failed to add claims to set: {}", e);
            error!("{}", s);
            Err(s.into())
        } else {
            Ok(())
        }
    }

    /// Retrieves the claims, if they exist, belonging to a given actor ID
    pub async fn get_claims(&self, actor_id: &str) -> Result<Option<Claims<wascap::jwt::Actor>>> {
        // KV context: KEY is {prefix}:claims_{actor_id}
        // value is claims JSON
        let key = prefix(&format!("claims_{}", actor_id));
        let args = GetArgs { key };
        let inv = self.invocation_for_provider(OP_GET, &serialize(&args)?);
        let get_r: GetResponse = invoke_as(&self.provider, inv).await?;

        if get_r.exists {
            let c: Claims<wascap::jwt::Actor> = serde_json::from_str(&get_r.value)?;
            Ok(Some(c))
        } else {
            Ok(None)
        }
    }

    pub async fn has_actor(&self, actor_id: &str) -> bool {
        match self.get_actors().await {
            Ok(t) => t.contains(&actor_id.to_string()),
            Err(_) => false,
        }
    }

    /// Retrieves the list of all known actor public keys within the claims cache
    pub async fn get_actors(&self) -> Result<Vec<String>> {
        let key = prefix("claims");
        let args = SetQueryArgs { key };
        let inv = self.invocation_for_provider(OP_SET_QUERY, &serialize(&args)?);
        let smembers: SetQueryResponse = invoke_as(&self.provider, inv).await?;
        Ok(smembers.values)
    }

    /// Retrieves the entire claims cache as a mapping between actor public keys and their associated claims
    pub async fn get_all_claims(&self) -> Result<HashMap<String, Claims<wascap::jwt::Actor>>> {
        let mut hm = HashMap::new();
        let actors = self.get_actors().await?;
        for actor in actors {
            let claims = self.get_claims(&actor).await?;
            if let Some(c) = claims {
                hm.insert(actor.to_string(), c);
            }
        }
        Ok(hm)
    }

    /// Stores the link configuration values and provider public key associated with the
    /// link definition key - actor, contract, link name
    /// A link definition key can be expressed verbally as "actor (key) is linked to a provider named (link name) supporting (contract)"
    pub async fn put_link(
        &self,
        actor_id: &str,
        provider_id: &str,
        contract_id: &str,
        link_name: &str,
        values: HashMap<String, String>,
    ) -> Result<()> {
        // KV context: {prefix}:link_{u64 hash of actor+contract+link_name}
        // value: JSON string for LinkDefinition
        // SET key {prefix}:links
        let key = link_key(actor_id, contract_id, link_name);
        let ld = LinkDefinition {
            actor_id: actor_id.to_string(),
            provider_id: provider_id.to_string(),
            contract_id: contract_id.to_string(),
            link_name: link_name.to_string(),
            values,
        };
        let args = SetArgs {
            key,
            value: serde_json::to_string(&ld)?,
            expires: 0,
        };
        let inv = self.invocation_for_provider(OP_SET, &serialize(&args)?);
        let inv_r = self.provider.send(inv).await?;
        if let Some(e) = inv_r.error {
            let s = format!("Failed to put link: {}", e);
            error!("{}", s);
            return Err(s.into());
        }

        let key = prefix("links");
        let args = SetAddArgs {
            key,
            value: format!("{}", hash_link_key(actor_id, contract_id, link_name)),
        };
        let inv = self.invocation_for_provider(OP_SET_ADD, &serialize(&args)?);
        let inv_r = self.provider.send(inv).await?;
        if let Some(e) = inv_r.error {
            let s = format!("Failed to add link to set: {}", e);
            error!("{}", s);
            return Err(s.into());
        }

        Ok(())
    }

    /// Retrieves the relevant link definition, if one exists, for a given actor, contract ID, and link
    /// name.
    pub async fn lookup_link(
        &self,
        actor_id: &str,
        contract_id: &str,
        link_name: &str,
    ) -> Result<Option<LinkDefinition>> {
        self.lookup_link_by_hash(hash_link_key(actor_id, contract_id, link_name))
            .await
    }

    /// Retrieves a link definition by its hashed key (which you get when you query the known link definitions)
    pub async fn lookup_link_by_hash(&self, hash: u64) -> Result<Option<LinkDefinition>> {
        let key = prefix(&format!("link_{}", hash));
        let args = GetArgs { key };
        let inv = self.invocation_for_provider(OP_GET, &serialize(&args)?);
        let res: GetResponse = invoke_as(&self.provider, inv).await?;
        if res.exists {
            let ld: LinkDefinition = serde_json::from_str(&res.value)?;
            Ok(Some(ld))
        } else {
            Ok(None)
        }
    }

    /// Retrieves the list of link definition keys as u64 hashes that you can then use to retrieve
    /// the link definition details via [lookup_link_by_hash].
    pub async fn get_links(&self) -> Result<Vec<u64>> {
        let key = prefix("links");
        let args = SetQueryArgs { key };
        let inv = self.invocation_for_provider(OP_SET_QUERY, &serialize(&args)?);
        Ok(invoke_as::<SetQueryResponse>(&self.provider, inv)
            .await?
            .values
            .iter()
            .map(|v| v.parse::<u64>().unwrap_or(0))
            .collect())
    }

    pub async fn collect_links(&self) -> Vec<LinkDefinition> {
        let mut res = Vec::new();

        for hash in self.get_links().await.unwrap_or_default() {
            if let Some(item) = self.lookup_link_by_hash(hash).await.unwrap_or(None) {
                res.push(item);
            }
        }

        res
    }

    /// Removes an OCI mapping from the cache
    async fn remove_oci(&self, oci_ref: &str) -> Result<()> {
        let key = prefix("ocis");
        let args = SetRemoveArgs {
            key,
            value: oci_ref.to_string(),
        };
        let inv = self.invocation_for_provider(OP_SET_REMOVE, &serialize(&args)?);
        let inv_r = self.provider.send(inv).await?;
        if let Some(e) = inv_r.error {
            let s = format!("Failed to remove OCI reference from set: {}", e);
            error!("{}", s);
            return Err(s.into());
        }

        let args = DelArgs {
            key: oci_key(oci_ref),
        };
        let inv = self.invocation_for_provider(OP_DEL, &serialize(&args)?);
        let inv_r = self.provider.send(inv).await?;
        if let Some(e) = inv_r.error {
            let s = format!("Failed to delete OCI mapping: {}", e);
            error!("{}", s);
            Err(s.into())
        } else {
            Ok(())
        }
    }

    fn invocation_for_provider(&self, op: &str, payload: &[u8]) -> Invocation {
        let target = WasmCloudEntity::Capability {
            id: self.cache_provider_id.to_string(),
            contract_id: CACHE_CONTRACT_ID.to_string(),
            link_name: CACHE_PROVIDER_LINK_NAME.to_string(),
        };
        Invocation::new(
            &self.host,
            WasmCloudEntity::Actor(SYSTEM_ACTOR.to_string()),
            target,
            op,
            payload.to_vec(),
        )
    }
}

fn call_alias_key(alias: &str) -> String {
    prefix(&format!("call_{}", alias))
}

fn oci_key(oci_ref: &str) -> String {
    prefix(&format!("oci_{}", oci_ref))
}

fn hash_link_key(actor: &str, contract_id: &str, link_name: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    actor.hash(&mut hasher);
    contract_id.hash(&mut hasher);
    link_name.hash(&mut hasher);
    hasher.finish()
}

fn link_key(actor: &str, contract_id: &str, link_name: &str) -> String {
    prefix(&format!(
        "link_{}",
        hash_link_key(actor, contract_id, link_name)
    ))
}

async fn invoke_as<'de, T: Deserialize<'de>>(
    target: &Recipient<Invocation>,
    inv: Invocation,
) -> Result<T> {
    let inv_r = target.send(inv).await?;
    if let Some(e) = inv_r.error {
        let s = format!("Lattice cache client invocation failure: {}", e);
        error!("{}", s);
        Err(s.into())
    } else {
        let t: T = deserialize(&inv_r.msg)?;
        Ok(t)
    }
}

fn prefix(input: &str) -> String {
    format!("{}:{}", CACHE_KEY_PREFIX, input)
}

pub(crate) fn get_claims() -> Claims<wascap::jwt::CapabilityProvider> {
    Claims::<wascap::jwt::CapabilityProvider>::decode(CACHE_JWT).unwrap()
}

pub(crate) const CACHE_JWT: &str = "eyJ0eXAiOiJqd3QiLCJhbGciOiJFZDI1NTE5In0.eyJqdGkiOiJncVJBY0ZIb0lKdW55TFpiMXozenNnIiwiaWF0IjoxNjA5OTQzMTkzLCJpc3MiOiJBQ09KSk42V1VQNE9ERDc1WEVCS0tUQ0NVSkpDWTVaS1E1NlhWS1lLNEJFSldHVkFPT1FIWk1DVyIsInN1YiI6IlZBSE5NMzdHNEFSSFozQ1lIQjNMMzRNNlRZUVdRUjZJWjRRVllDNE5ZWldUSkNKMkxXUDdTNloyIiwid2FzY2FwIjp7Im5hbWUiOiJOQVRTIFJlcGxpY2F0ZWQgSW4tTWVtb3J5IEtleS1WYWx1ZSBTdG9yZSIsImNhcGlkIjoid2FzbWNsb3VkOmtleXZhbHVlIiwidmVuZG9yIjoid2FzbUNsb3VkIiwicmV2IjoxLCJ2ZXIiOiIwLjEuMCIsInRhcmdldF9oYXNoZXMiOnt9fX0.NwDUQMl08RRMjcNrvKiTNKYyLahYrYtcIUIQiHzElOq7SJqtYh_YGVKN-64YYGHdSfwK1OK89arS9DGmW7YuCQ";

#[cfg(test)]
mod test {
    use crate::messagebus::latticecache_client::hash_link_key;

    #[test]
    fn hasher() {
        let a = hash_link_key("Mxxxx", "wasmcloud:keyvalue", "default");
        let b = hash_link_key("Mxxxx", "wasmcloud:keyvalue", "default");
        assert_eq!(a, b);
    }
}

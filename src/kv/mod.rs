use std::collections::HashMap;
use std::io::Write;

use async_nats::jetstream::kv::Store;
use async_nats::Client;
use data_encoding::HEXUPPER;
use ring::digest::{digest, SHA256};

use crate::LinkDefinition;
use crate::Result;

mod cached;
mod direct;

pub use cached::*;
pub use direct::*;

const LINKDEF_PREFIX: &str = "LINKDEF_";
const CLAIMS_PREFIX: &str = "CLAIMS_";
const BUCKET_PREFIX: &str = "LATTICEDATA_";
const SUBJECT_KEY: &str = "sub";

#[async_trait::async_trait]
pub trait KvStore {
    /// Returns all links in the store
    async fn get_links(&self) -> Result<Vec<LinkDefinition>>;

    /// Returns all claims in the store
    async fn get_all_claims(&self) -> Result<Vec<HashMap<String, String>>>;

    /// Returns all provider claims in the store
    async fn get_provider_claims(&self) -> Result<Vec<HashMap<String, String>>>;

    /// Returns all actor claims in the store
    async fn get_actor_claims(&self) -> Result<Vec<HashMap<String, String>>>;

    /// Returns all links in the store that match the provided filter function. For some
    /// implementations, this could be more efficient than fetching all links and filtering them in
    /// memory.
    async fn get_filtered_links<F>(&self, filter_fn: F) -> Result<Vec<LinkDefinition>>
    where
        F: FnMut(&LinkDefinition) -> bool + Send;

    /// Returns the link definition for the given actor, link name, and contract ID, if it exists.
    async fn get_link(
        &self,
        actor_id: &str,
        link_name: &str,
        contract_id: &str,
    ) -> Result<Option<LinkDefinition>>;

    /// Returns the claim for the given ID, if it exists.
    async fn get_claims(&self, id: &str) -> Result<Option<HashMap<String, String>>>;

    /// Adds a link definition to the store
    async fn put_link(&self, ld: LinkDefinition) -> Result<()>;

    /// Deletes a link definition from the store
    async fn delete_link(&self, actor_id: &str, contract_id: &str, link_name: &str) -> Result<()>;
}

/// A helper that creates a filter function for [`get_filtered_links`](KvStore::get_filtered_links)
/// to fetch links between a specific actor and provider
pub fn actor_and_provider_filter<'a>(
    actor_id: &'a str,
    provider_id: &'a str,
) -> impl FnMut(&'a LinkDefinition) -> bool {
    move |ld| ld.actor_id == actor_id && ld.provider_id == provider_id
}

pub(crate) fn ld_hash(ld: &LinkDefinition) -> String {
    ld_hash_raw(&ld.actor_id, &ld.contract_id, &ld.link_name)
}

// Performs a hash function against the link definition key fields.
pub(crate) fn ld_hash_raw(actor_id: &str, contract_id: &str, link_name: &str) -> String {
    let mut cleanbytes: Vec<u8> = Vec::new();
    cleanbytes.write_all(actor_id.as_bytes()).unwrap();
    cleanbytes.write_all(contract_id.as_bytes()).unwrap();
    cleanbytes.write_all(link_name.as_bytes()).unwrap();

    let digest = digest(&SHA256, &cleanbytes);
    HEXUPPER.encode(digest.as_ref())
}

pub(crate) fn ld_key(hash: &str) -> String {
    format!("{LINKDEF_PREFIX}{hash}")
}

async fn get_kv_store(
    nc: Client,
    lattice_prefix: &str,
    js_domain: Option<String>,
) -> Result<Store> {
    let jetstream = if let Some(domain) = js_domain {
        async_nats::jetstream::with_domain(nc, domain)
    } else {
        async_nats::jetstream::new(nc)
    };
    let bucket = format!("{}{}", BUCKET_PREFIX, lattice_prefix);
    jetstream.get_key_value(bucket).await.map_err(|e| e.into())
}

async fn put_link(store: &Store, ld: &LinkDefinition) -> Result<()> {
    store
        .put(ld_key(&ld_hash(ld)), serde_json::to_vec(&ld)?.into())
        .await
        .map(|_| ())
        .map_err(|e| e.into())
}

async fn delete_link(
    store: &Store,
    actor_id: &str,
    contract_id: &str,
    link_name: &str,
) -> Result<()> {
    store
        .delete(ld_key(&ld_hash_raw(actor_id, contract_id, link_name)))
        .await
        .map(|_| ())
        .map_err(|e| e.into())
}

// NOTE: these tests require nats to be running with JS enabled.
#[cfg(test)]
mod test {
    use std::future::Future;

    use rstest::rstest;

    use crate::kv::{ld_hash, CachedKvStore, DirectKvStore, KvStore};
    use crate::types::LinkDefinition;

    use super::BUCKET_PREFIX;

    const CLAIMS_1: &str = r#"{"call_alias":"","caps":"wasmcloud:httpserver","iss":"ABRIBHH54GM7QIEJBYYGZJUSDAMO34YM4SKWUQJGIILRB7JYGXEPWUVT","name":"kvcounter","rev":"1631624220","sub":"MBW3UGAIONCX3RIDDUGDCQIRGBQQOWS643CVICQ5EZ7SWNQPZLZTSQKU","tags":"","version":"0.3.0"}"#;
    const CLAIMS_2: &str = r#"{"call_alias":"","caps":"","iss":"ACOJJN6WUP4ODD75XEBKKTCCUJJCY5ZKQ56XVKYK4BEJWGVAOOQHZMCW","name":"HTTP Server","rev":"1644594344","sub":"VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M","tags":"","version":"0.14.10"}"#;

    const LINK_1: &str = r#"{"actor_id":"MBW3UGAIONCX3RIDDUGDCQIRGBQQOWS643CVICQ5EZ7SWNQPZLZTSQKU","contract_id":"wasmcloud:httpserver","id":"fb30deff-bbe7-4a28-a525-e53ebd4e8228","link_name":"default","provider_id":"VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M","values":{"PORT":"8082"}}"#;
    const LINK_2: &str = r#"{"actor_id":"MBW3UGAIONCX3RIDDUGDCQIRGBQQOWS643CVICQ5EZ7SWNQPZLZTSQKU","contract_id":"wasmcloud:keyvalue","id":"ff140106-dd0d-44ee-8241-a2158a528b1d","link_name":"default","provider_id":"VAZVC4RX54J2NVCMCW7BPCAHGGG5XZXDBXFUMDUXGESTMQEJLC3YVZWB","values":{"URL":"redis://127.0.0.1:6379"}}"#;

    #[test]
    fn test_hash_compatibility() {
        let mut ld = LinkDefinition::default();
        // generated by sha_binary = :crypto.hash_final(sha), sha_hex = sha_binary |> Base.encode16() |> String.upcase
        const ELIXIR_HASH: &str =
            "B40411AD09B70A2C83D59923584F66BA2C5A3C274DC4F19416DA49CCD6531F9C";

        ld.actor_id = "Mbob".to_string();
        ld.provider_id = "Valice".to_string();
        ld.link_name = "default".to_string();
        ld.contract_id = "wasmcloud:testy".to_string();

        let h1 = ld_hash(&ld);
        assert_eq!(h1, ELIXIR_HASH);
    }

    #[tokio::test]
    async fn test_get_returns_none_for_nonexistent_store() {
        let client = async_nats::connect("127.0.0.1:4222").await.unwrap();

        CachedKvStore::new(client, "this-lattice-shall-never-existeth", None)
            .await
            .expect_err("Should not be able to get a store for a non-existent lattice");
    }

    #[rstest]
    #[case(DirectKvStore::new, "mylattice1direct")]
    #[case(CachedKvStore::new, "mylattice1cached")]
    #[tokio::test]
    async fn test_get_claims_returns_response<'a, F, U, T>(
        #[case] new_store: F,
        #[case] lattice_name: &'static str,
    ) where
        F: Fn(async_nats::Client, &'a str, Option<String>) -> U,
        U: Future<Output = crate::Result<T>>,
        T: KvStore,
    {
        let client = async_nats::connect("127.0.0.1:4222").await.unwrap();
        let js = async_nats::jetstream::new(client.clone());
        let bucket_name = format!("{BUCKET_PREFIX}{lattice_name}");
        // Always try to delete the bucket before recreating so that we start fresh. We can't
        // cleanup on drop because async
        let _ = js.delete_key_value(&bucket_name).await;
        let kv = js
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket: bucket_name.clone(),
                ..Default::default()
            })
            .await
            .unwrap();

        kv.put(
            "CLAIMS_VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M",
            CLAIMS_2.into(),
        )
        .await
        .unwrap();
        kv.put(
            "CLAIMS_MBW3UGAIONCX3RIDDUGDCQIRGBQQOWS643CVICQ5EZ7SWNQPZLZTSQKU",
            CLAIMS_1.into(),
        )
        .await
        .unwrap();

        let store = new_store(client, lattice_name, None).await.unwrap();
        let claims = store.get_all_claims().await.expect("Should get claims");

        js.delete_key_value(bucket_name).await.unwrap();

        assert_eq!(claims.len(), 2);
        assert!(claims[0].contains_key("name"));
        assert!(claims[0].contains_key("rev"));
        assert!(claims[0].contains_key("sub"));
        assert!(claims[1].contains_key("call_alias"));
    }

    #[rstest]
    #[case(DirectKvStore::new, "mylattice2direct")]
    #[case(CachedKvStore::new, "mylattice2cached")]
    #[tokio::test]
    async fn test_get_links_returns_response<'a, F, U, T>(
        #[case] new_store: F,
        #[case] lattice_name: &'static str,
    ) where
        F: Fn(async_nats::Client, &'a str, Option<String>) -> U,
        U: Future<Output = crate::Result<T>>,
        T: KvStore,
    {
        let client = async_nats::connect("127.0.0.1:4222").await.unwrap();
        let js = async_nats::jetstream::new(client.clone());
        let bucket_name = format!("LATTICEDATA_{lattice_name}");
        // Always try to delete the bucket before recreating so that we start fresh. We can't
        // cleanup on drop because async
        let _ = js.delete_key_value(&bucket_name).await;
        let kv = js
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket: bucket_name.clone(),
                ..Default::default()
            })
            .await
            .unwrap();

        kv.put(
            "LINKDEF_ff140106-dd0d-44ee-8241-a2158a528b1d",
            LINK_2.into(),
        )
        .await
        .unwrap();
        kv.put("LINKDEF_fb30deff-bbe7-4a28-a525-e53ebd4e822", LINK_1.into())
            .await
            .unwrap();

        let store = new_store(client, lattice_name, None).await.unwrap();
        let links = store.get_links().await.expect("Should get links");

        js.delete_key_value(bucket_name).await.unwrap();

        assert_eq!(links.len(), 2);
    }

    #[rstest]
    #[case(DirectKvStore::new, "mylattice3direct")]
    #[case(CachedKvStore::new, "mylattice3cached")]
    #[tokio::test]
    async fn test_put_and_del_link<'a, F, U, T>(
        #[case] new_store: F,
        #[case] lattice_name: &'static str,
    ) where
        F: Fn(async_nats::Client, &'a str, Option<String>) -> U,
        U: Future<Output = crate::Result<T>>,
        T: KvStore,
    {
        let client = async_nats::connect("127.0.0.1:4222").await.unwrap();
        let js = async_nats::jetstream::new(client.clone());
        let bucket_name = format!("LATTICEDATA_{lattice_name}");
        // Always try to delete the bucket before recreating so that we start fresh. We can't
        // cleanup on drop because async
        let _ = js.delete_key_value(&bucket_name).await;
        js.create_key_value(async_nats::jetstream::kv::Config {
            bucket: bucket_name.clone(),
            ..Default::default()
        })
        .await
        .unwrap();

        let store = new_store(client, lattice_name, None).await.unwrap();
        let ld = LinkDefinition {
            actor_id: "Mbob".to_string(),
            provider_id: "Valice".to_string(),
            contract_id: "wasmcloud:testy".to_string(),
            link_name: "default".to_string(),
            ..Default::default()
        };
        store.put_link(ld).await.unwrap();

        let ld2 = LinkDefinition {
            actor_id: "Msteve".to_string(),
            provider_id: "Valice".to_string(),
            contract_id: "wasmcloud:testy".to_string(),
            link_name: "default".to_string(),
            ..Default::default()
        };
        store.put_link(ld2).await.unwrap();

        store
            .delete_link("Mbob", "wasmcloud:testy", "default")
            .await
            .unwrap();

        let links = store.get_links().await.expect("Should get links");

        js.delete_key_value(bucket_name).await.unwrap();

        assert_eq!(links.len(), 1); // 1 left after delete
    }

    #[tokio::test]
    async fn test_cache_updates() {
        let client = async_nats::connect("127.0.0.1:4222").await.unwrap();
        let js = async_nats::jetstream::new(client.clone());
        let bucket_name = "LATTICEDATA_cachetest";
        // Always try to delete the bucket before recreating so that we start fresh. We can't
        // cleanup on drop because async
        let _ = js.delete_key_value(&bucket_name).await;
        let kv = js
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket: bucket_name.to_string(),
                ..Default::default()
            })
            .await
            .unwrap();

        // Set up different stores so that we can test that the cache is updated correctly
        let insert_store = CachedKvStore::new(client.clone(), "cachetest", None)
            .await
            .unwrap();
        let read_store = CachedKvStore::new(client, "cachetest", None).await.unwrap();
        let ld = LinkDefinition {
            actor_id: "Mbob".to_string(),
            provider_id: "Valice".to_string(),
            contract_id: "wasmcloud:testy".to_string(),
            link_name: "default".to_string(),
            ..Default::default()
        };
        insert_store.put_link(ld).await.unwrap();

        let ld2 = LinkDefinition {
            actor_id: "Msteve".to_string(),
            provider_id: "Valice".to_string(),
            contract_id: "wasmcloud:testy".to_string(),
            link_name: "default".to_string(),
            ..Default::default()
        };
        insert_store.put_link(ld2).await.unwrap();

        kv.put(
            "CLAIMS_VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M",
            CLAIMS_2.into(),
        )
        .await
        .unwrap();

        // Give time for events to be handled
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let links = read_store.get_links().await.expect("Should get links");
        assert_eq!(links.len(), 2);
        assert!(
            links.iter().any(|ld| ld.actor_id == "Mbob"),
            "Should have the correct linkdefs"
        );
        assert!(
            links.iter().any(|ld| ld.actor_id == "Msteve"),
            "Should have the correct linkdefs"
        );

        let claims = read_store
            .get_all_claims()
            .await
            .expect("Should get claims");
        assert_eq!(claims.len(), 1);
        assert_eq!(
            claims[0].get("name").expect("Should have a `name` value"),
            "HTTP Server",
            "Should have the correct claims info"
        );

        insert_store
            .delete_link("Mbob", "wasmcloud:testy", "default")
            .await
            .unwrap();
        kv.delete("CLAIMS_VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M")
            .await
            .unwrap();

        // Give time for events to be handled
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let links = read_store.get_links().await.expect("Should get links");
        let claims = read_store
            .get_all_claims()
            .await
            .expect("Should get claims");
        js.delete_key_value(bucket_name).await.unwrap();

        assert_eq!(links.len(), 1);
        assert_eq!(claims.len(), 0);
    }
}

use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// When an an actor binds to a capability provider, it does so with a contract ID
/// (e.g. 'wascc:messaging') and a link name (e.g. `default`). The triplet of
/// the actor, contract_id, and the link name is the unique or primary key for
/// the link. That triplet can only ever be bound to a single provider ID.
#[derive(Default, Eq, Clone, PartialEq, Hash, Debug, Serialize, Deserialize)]
struct LinkKey {
    actor: String,
    contract_id: String,
    link_name: String,
}

impl LinkKey {
    fn new(actor: &str, contract_id: &str, link_name: &str) -> LinkKey {
        LinkKey {
            actor: actor.to_string(),
            contract_id: contract_id.to_string(),
            link_name: link_name.to_string(),
        }
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
struct LinkValues {
    provider_id: String,
    values: HashMap<String, String>,
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LinkCache {
    link_config: HashMap<LinkKey, LinkValues>,
}

impl LinkCache {
    pub fn add_link(
        &mut self,
        actor: &str,
        contract_id: &str,
        link_name: &str,
        provider_id: &str,
        values: HashMap<String, String>,
    ) {
        self.link_config.insert(
            LinkKey::new(actor, contract_id, link_name),
            LinkValues {
                provider_id: provider_id.to_string(),
                values,
            },
        );
    }

    pub fn len(&self) -> usize {
        self.link_config.len()
    }

    pub fn find_provider_id(
        &self,
        actor: &str,
        contract_id: &str,
        link_name: &str,
    ) -> Option<String> {
        let key = LinkKey::new(actor, contract_id, link_name);
        self.link_config
            .get(&key)
            .cloned()
            .map(|bv| bv.provider_id.to_string())
    }

    pub fn remove_link(&mut self, actor: &str, contract_id: &str, link_name: &str) {
        self.link_config
            .remove(&LinkKey::new(actor, contract_id, link_name));
    }

    /// Retrieves the list of all actor links that pertain to a specific capability provider. Does not return an error,
    /// will return an empty vector if no links are found. Each item in the returned vector is a tuple consisting of the
    /// actor's public key and the config values hash map.
    pub fn find_links(
        &self,
        link_name: &str,
        provider_id: &str,
    ) -> Vec<(String, HashMap<String, String>)> {
        let mut res = Vec::new();
        for (key, val) in &self.link_config {
            if key.link_name == link_name && val.provider_id == provider_id {
                res.push((key.actor.to_string(), val.values.clone()));
            }
        }
        res
    }
}

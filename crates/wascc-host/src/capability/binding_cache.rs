use crate::Result;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// When an an actor binds to a capability provider, it does so with a contract ID
/// (e.g. 'wascc:messaging') and a binding name (e.g. `default`). The triplet of
/// the actor, contract_id, and the binding name is the unique or primary key for
/// the binding. That triplet can only ever be bound to a single provider ID.
#[derive(Default, Eq, Clone, PartialEq, Hash, Debug, Serialize, Deserialize)]
struct BindingKey {
    actor: String,
    contract_id: String,
    binding_name: String,
}

impl BindingKey {
    fn new(actor: &str, contract_id: &str, binding_name: &str) -> BindingKey {
        BindingKey {
            actor: actor.to_string(),
            contract_id: contract_id.to_string(),
            binding_name: binding_name.to_string(),
        }
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
struct BindingValues {
    provider_id: String,
    values: HashMap<String, String>,
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub(crate) struct BindingCache {
    binding_config: HashMap<BindingKey, BindingValues>,
}

impl BindingCache {
    pub fn add_binding(
        &mut self,
        actor: &str,
        contract_id: &str,
        binding_name: &str,
        provider_id: &str,
        values: HashMap<String, String>,
    ) {
        self.binding_config.insert(
            BindingKey::new(actor, contract_id, binding_name),
            BindingValues {
                provider_id: provider_id.to_string(),
                values,
            },
        );
    }

    pub fn len(&self) -> usize {
        self.binding_config.len()
    }

    pub fn find_provider_id(
        &self,
        actor: &str,
        contract_id: &str,
        binding_name: &str,
    ) -> Option<String> {
        let key = BindingKey::new(actor, contract_id, binding_name);
        self.binding_config
            .get(&key)
            .cloned()
            .map(|bv| bv.provider_id.to_string())
    }

    pub fn remove_binding(&mut self, actor: &str, contract_id: &str, binding_name: &str) {
        self.binding_config
            .remove(&BindingKey::new(actor, contract_id, binding_name));
    }

    /// Retrieves the list of all actor bindings that pertain to a specific capability provider. Does not return an error,
    /// will return an empty vector if no bindings are found. Each item in the returned vector is a tuple consisting of the
    /// actor's public key and the config values hash map.
    pub fn find_bindings(
        &self,
        binding_name: &str,
        provider_id: &str,
    ) -> Vec<(String, HashMap<String, String>)> {
        let mut res = Vec::new();
        for (key, val) in &self.binding_config {
            if key.binding_name == binding_name && val.provider_id == provider_id {
                res.push((key.actor.to_string(), val.values.clone()));
            }
        }
        res
    }
}

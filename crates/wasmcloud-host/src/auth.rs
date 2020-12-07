use crate::dispatch::WasccEntity;
use crate::Result;
use crate::{Invocation, SYSTEM_ACTOR};
use std::collections::HashMap;
use wascap::jwt::{Actor, Claims};

/// An authorizer is responsible for determining whether an actor can be loaded as well as
/// whether an actor can invoke another entity. For invocation checks, the authorizer is only ever invoked _after_
/// an initial capability attestation check has been performed and _passed_. This has the net effect of making it
/// impossible to override the base behavior of checking that an actor's embedded JWT contains the right
/// capability attestations.
pub trait Authorizer: CloneAuthorizer + Sync + Send {
    /// This check is performed during the `[start_actor](crate::Host::start_actor)` call, allowing the custom authorizer to do things
    /// like verify a provenance chain, make external calls, etc.
    fn can_load(&self, claims: &Claims<Actor>) -> bool;
    /// This check will be performed for _every_ invocation that has passed the base capability check,
    /// including the operation that occurs during `bind_actor`. Developers should be aware of this because
    /// if `set_authorizer` is done _after_ actor link, it could potentially allow an unauthorized link.
    fn can_invoke(&self, claims: &Claims<Actor>, target: &WasccEntity, operation: &str) -> bool;
}

#[doc(hidden)]
pub trait CloneAuthorizer {
    fn clone_authorizer(&self) -> Box<dyn Authorizer>;
}

impl<T> CloneAuthorizer for T
where
    T: Authorizer + Clone + 'static,
{
    fn clone_authorizer(&self) -> Box<dyn Authorizer> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn Authorizer> {
    fn clone(&self) -> Self {
        self.clone_authorizer()
    }
}

#[derive(Clone)]
pub(crate) struct DefaultAuthorizer {}

impl DefaultAuthorizer {
    pub fn new() -> DefaultAuthorizer {
        DefaultAuthorizer {}
    }
}

impl Authorizer for DefaultAuthorizer {
    fn can_load(&self, _claims: &Claims<Actor>) -> bool {
        true
    }

    // This doesn't actually mean everyone can invoke everything. Remember that the host itself
    // will _always_ enforce the claims check on an actor having the required capability
    // attestation
    fn can_invoke(&self, _claims: &Claims<Actor>, target: &WasccEntity, _operation: &str) -> bool {
        match target {
            WasccEntity::Actor(_a) => true,
            WasccEntity::Capability { .. } => true,
        }
    }
}

pub(crate) fn authorize_invocation(
    inv: &Invocation,
    authorizer: Box<dyn Authorizer>,
    claims_cache: &HashMap<String, Claims<wascap::jwt::Actor>>,
) -> Result<()> {
    let _ = inv.validate_antiforgery()?; // Fail authorization if the invocation isn't properly signed

    if let WasccEntity::Actor(ref actor_key) = &inv.origin {
        if let Some(c) = claims_cache.get(actor_key) {
            if let Some(ref caps) = c.metadata.as_ref().unwrap().caps {
                let allowed = if let WasccEntity::Capability { contract_id, .. } = &inv.target {
                    caps.contains(contract_id)
                } else {
                    true
                };
                if allowed {
                    if authorizer.can_invoke(&c, &inv.target, &inv.operation) {
                        Ok(())
                    } else {
                        Err("Authorization denied - authorizer rejected invocation".into())
                    }
                } else {
                    Err("Authorization denied - Actor does not have required claims".into())
                }
            } else {
                Err("This actor has no embedded claims. Authorization denied".into())
            }
        } else {
            if actor_key == SYSTEM_ACTOR {
                // system actor can call other actors
                Ok(())
            } else {
                Err(format!(
                    "No claims found for actor '{}'. Has it been started?",
                    actor_key
                )
                .into())
            }
        }
    } else {
        Ok(()) // Allow cap->actor calls without checking
    }
}

#[cfg(test)]
mod test {
    use crate::auth::{authorize_invocation, Authorizer, DefaultAuthorizer};
    use crate::{Invocation, WasccEntity};
    use std::collections::HashMap;
    use wascap::jwt::{Actor, Claims, ClaimsBuilder};
    use wascap::prelude::KeyPair;

    #[test]
    fn actor_to_actor_allowed() {
        let inv = gen_invocation(
            WasccEntity::Actor("A".to_string()),
            WasccEntity::Actor("B".to_string()),
            "test",
        );
        let mut cache = HashMap::new();
        cache.insert(
            "A".to_string(),
            ClaimsBuilder::new()
                .with_metadata(wascap::jwt::Actor::new(
                    "A".to_string(),
                    Some(vec!["wascc:messaging".to_string()]),
                    None,
                    false,
                    None,
                    None,
                ))
                .build(),
        );
        let auth = Box::new(DefaultAuthorizer::new());
        assert!(authorize_invocation(&inv, auth, &cache).is_ok());
    }

    #[test]
    fn block_actor_with_no_claims() {
        let inv = gen_invocation(
            WasccEntity::Actor("A".to_string()),
            WasccEntity::Actor("B".to_string()),
            "test",
        );
        let cache = HashMap::new();
        let auth = Box::new(DefaultAuthorizer::new());
        let res = authorize_invocation(&inv, auth, &cache);
        assert!(res.is_err());
        assert_eq!(
            res.err().unwrap().to_string(),
            "No claims found for actor 'A'. Has it been started?"
        );
    }

    #[test]
    fn block_actor_with_insufficient_claims() {
        let target = WasccEntity::Capability {
            contract_id: "wascc:keyvalue".to_string(),
            id: "Vxxx".to_string(),
            link_name: "default".to_string(),
        };
        let inv = gen_invocation(WasccEntity::Actor("A".to_string()), target, "test");
        let mut cache = HashMap::new();
        cache.insert(
            "A".to_string(),
            ClaimsBuilder::new()
                .with_metadata(wascap::jwt::Actor::new(
                    "A".to_string(),
                    Some(vec!["wascc:messaging".to_string()]),
                    None,
                    false,
                    None,
                    None,
                ))
                .build(),
        );
        let auth = Box::new(DefaultAuthorizer::new());
        let res = authorize_invocation(&inv, auth, &cache);
        assert_eq!(
            res.err().unwrap().to_string(),
            "Authorization denied - Actor does not have required claims"
        );
    }

    #[test]
    fn invoke_authorizer_when_initial_check_passes() {
        let target = WasccEntity::Capability {
            contract_id: "wascc:keyvalue".to_string(),
            id: "Vxxx".to_string(),
            link_name: "default".to_string(),
        };
        let inv = gen_invocation(WasccEntity::Actor("A".to_string()), target, "test");
        let mut cache = HashMap::new();
        cache.insert(
            "A".to_string(),
            ClaimsBuilder::new()
                .with_metadata(wascap::jwt::Actor::new(
                    "A".to_string(),
                    Some(vec!["wascc:keyvalue".to_string()]),
                    None,
                    false,
                    None,
                    None,
                ))
                .build(),
        );
        let auth = Box::new(CrankyAuthorizer::new());
        let res = authorize_invocation(&inv, auth, &cache);
        assert_eq!(
            res.err().unwrap().to_string(),
            "Authorization denied - authorizer rejected invocation"
        );
    }

    fn gen_invocation(source: WasccEntity, target: WasccEntity, op: &str) -> Invocation {
        let hk = KeyPair::new_server();
        Invocation::new(&hk, source, target, op, vec![])
    }

    #[derive(Clone)]
    struct CrankyAuthorizer;
    impl CrankyAuthorizer {
        pub fn new() -> CrankyAuthorizer {
            CrankyAuthorizer
        }
    }
    impl Authorizer for CrankyAuthorizer {
        fn can_load(&self, _claims: &Claims<Actor>) -> bool {
            false
        }

        fn can_invoke(
            &self,
            _claims: &Claims<Actor>,
            _target: &WasccEntity,
            _operation: &str,
        ) -> bool {
            false
        }
    }
}

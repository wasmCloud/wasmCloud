use crate::dispatch::WasccEntity;
use wascap::jwt::{Actor, Claims};

/// An authorizer is responsible for determining whether an actor can be loaded as well as
/// whether an actor can invoke another entity. For invocation checks, the authorizer is only ever invoked _after_
/// an initial capability attestation check has been performed and _passed_. This has the net effect of making it
/// impossible to override the base behavior of checking that an actor's embedded JWT contains the right
/// capability attestations.
pub trait Authorizer: Sync + Send {
    /// This check is performed during the `add_actor` call, allowing the custom authorizer to do things
    /// like verify a provenance chain, make external calls, etc.
    fn can_load(&self, claims: &Claims<Actor>) -> bool;
    /// This check will be performed for _every_ invocation that has passed the base capability check,
    /// including the operation that occurs during `bind_actor`. Developers should be aware of this because
    /// if `set_authorizer` is done _after_ actor binding, it could potentially allow an unauthorized binding.
    fn can_invoke(&self, claims: &Claims<Actor>, target: &WasccEntity, operation: &str) -> bool;
}

pub(crate) struct DefaultAuthorizer {}

impl DefaultAuthorizer {
    pub fn new() -> impl Authorizer {
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

mod runner;

use crate::dispatch::{Invocation, InvocationResponse};
use crate::Result;

pub(crate) use runner::*;

/// The trait that must be implemented by all middleware. Each time an actor or
/// capability provider is invoked, each middleware in the chain will get a chance to
/// react to the input Invocation. Middleware is cloned on a 1:1 basis with each
/// potential target. Because each of the target types invokes in single-threaded fashion,
/// middleware authors can maintain state inside the middleware object between pre and post
/// calls, using the invocation's ID for correlation.
pub trait Middleware: CloneMiddleware + Send + Sync + 'static {
    /// Called prior to the actual invocation of an actor
    fn actor_pre_invoke(&self, inv: &Invocation) -> Result<()>;
    /// Called after an actor's invocation, _only if_ that call was successful.
    fn actor_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse>;

    /// Invoked prior to a capability provider's invocation
    fn capability_pre_invoke(&self, inv: &Invocation) -> Result<()>;
    /// Invoked after a capability provider's invocation, _only if_ that call was successful
    fn capability_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse>;
}

// ---
// Make middleware require clone without preventing it from being a trait object.
// Caution: Shenanigans below

pub trait CloneMiddleware {
    fn clone_middleware(&self) -> Box<dyn Middleware>;
}

impl<T> CloneMiddleware for T
where
    T: Middleware + Clone + 'static,
{
    fn clone_middleware(&self) -> Box<dyn Middleware> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn Middleware> {
    fn clone(&self) -> Self {
        self.clone_middleware()
    }
}

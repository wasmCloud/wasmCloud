//! # Common types used for managing native capability providers

use std::error::Error;

use std::any::Any;

/// The dispatcher is used by a native capability provider to send commands to an actor, expecting
/// a result containing a byte array in return
pub trait Dispatcher: Any + Send + Sync {
    fn dispatch(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>>;
}

/// The NullDispatcher is as its name implies--a dispatcher that does nothing. This is convenient for
/// initializing a capability provider with a null dispatcher, and then swapping it for a real dispatcher
/// when the host runtime provides one configured with the appropriate channels
#[derive(Default)]
pub struct NullDispatcher {}

impl NullDispatcher {
    pub fn new() -> NullDispatcher {
        NullDispatcher {}
    }
}

impl Dispatcher for NullDispatcher {
    fn dispatch(
        &self,
        _actor: &str,
        _op: &str,
        _msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
        unimplemented!()
    }
}

/// Every native capability provider must implement this trait. Both portable and native capability providers
/// must respond to the following operations: `OP_BIND_ACTOR`, `OP_REMOVE_ACTOR`
pub trait CapabilityProvider: CloneProvider + Send + Sync {
    /// This function will be called on the provider when the host runtime is ready and has configured a dispatcher. This function is only ever
    /// called _once_ for a capability provider, regardless of the number of actors being managed in the host
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn Dispatcher>,
    ) -> Result<(), Box<dyn Error + Send + Sync>>;
    /// Invoked when an actor has requested that a provider perform a given operation
    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>>;
    /// This function is called to let the capability provider know that it is being removed
    /// from the host runtime. This gives the provider an opportunity to clean up any
    /// resources and stop any running threads.
    /// WARNING: do not do anything in this function that can
    /// cause a panic, including attempting to write to STDOUT while the host process is terminating
    fn stop(&self);
}

#[doc(hidden)]
pub trait CloneProvider {
    fn clone_provider(&self) -> Box<dyn CapabilityProvider>;
}

impl<T> CloneProvider for T
where
    T: CapabilityProvider + Clone + 'static,
{
    fn clone_provider(&self) -> Box<dyn CapabilityProvider> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn CapabilityProvider> {
    fn clone(&self) -> Self {
        self.clone_provider()
    }
}

/// Wraps a constructor inside an FFI function to allow the `CapabilityProvider` trait implementation
/// to be instantiated and used by the host runtime
#[macro_export]
macro_rules! capability_provider {
    ($provider_type:ty, $constructor:path) => {
        #[no_mangle]
        pub extern "C" fn __capability_provider_create(
        ) -> *mut $crate::capabilities::CapabilityProvider {
            let constructor: fn() -> $provider_type = $constructor;
            let object = constructor();
            let boxed: Box<$crate::capabilities::CapabilityProvider> = Box::new(object);
            Box::into_raw(boxed)
        }
    };
}

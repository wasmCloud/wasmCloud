use core::fmt::Debug;

/// External capability provider
pub trait Provider {
    /// Error returned by handling capability provider invocations
    type Error: ToString + Debug;

    /// Handle a capability provider invocation
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the operation fails
    fn handle(
        &self,
        binding: String,
        namespace: String,
        operation: String,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error>;
}

impl Provider for () {
    type Error = &'static str;

    fn handle(&self, _: String, _: String, _: String, _: Vec<u8>) -> Result<Vec<u8>, Self::Error> {
        Err("not supported")
    }
}

impl<T, E, F> Provider for F
where
    T: Into<Vec<u8>>,
    E: ToString + Debug,
    F: Fn(String, String, String, Vec<u8>) -> Result<T, E>,
{
    type Error = E;

    fn handle(
        &self,
        binding: String,
        namespace: String,
        operation: String,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        self(binding, namespace, operation, payload).map(Into::into)
    }
}

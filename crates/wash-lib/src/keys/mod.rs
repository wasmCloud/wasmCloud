//! A common set of types and traits for managing collections of nkeys used for wasmCloud

use anyhow::Result;
use nkeys::KeyPair;

/// Convenience re-export of nkeys to make key functionality easier to manage
pub use nkeys;

pub mod fs;

/// A trait that can be implemented by anything that needs to manage nkeys
pub trait KeyManager {
    /// Returns the named keypair. Returns None if the key doesn't exist in the manager
    fn get(&self, name: &str) -> Result<Option<KeyPair>>;

    /// List all key names available
    fn list_names(&self) -> Result<Vec<String>>;

    /// Retrieves all keys. Note that this could be an expensive operation depending on the
    /// implementation
    fn list(&self) -> Result<Vec<KeyPair>>;

    /// Deletes a named keypair
    fn delete(&self, name: &str) -> Result<()>;

    /// Saves the given keypair with the given name
    fn save(&self, name: &str, key: &KeyPair) -> Result<()>;
}

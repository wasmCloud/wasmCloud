//! A filesystem directory based implementation of a `KeyManager`

use std::{
    ops::Deref,
    path::{Path, PathBuf},
};

use anyhow::Result;
use nkeys::KeyPair;

use super::KeyManager;

pub const KEY_FILE_EXTENSION: &str = "nk";

pub struct KeyDir(PathBuf);

impl AsRef<Path> for KeyDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Deref for KeyDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl KeyDir {
    /// Creates a new `KeyDir`, erroring if it is unable to access or create the given directory.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let p = path.as_ref();
        let exists = p.exists();
        if exists && !p.is_dir() {
            anyhow::bail!("{} is not a directory (or cannot be accessed)", p.display())
        } else if !exists {
            std::fs::create_dir_all(p)?;
        }
        // Always ensure the directory has the proper permissions, even if it exists
        set_permissions_keys(p)?;
        // Make sure we have the fully qualified path at this point
        Ok(Self(p.canonicalize()?))
    }

    /// Returns a list of paths to all keyfiles in the directory
    pub fn list_paths(&self) -> Result<Vec<PathBuf>> {
        let paths = std::fs::read_dir(&self.0)?;

        Ok(paths
            .filter_map(|p| {
                if let Ok(entry) = p {
                    let path = entry.path();
                    match path.extension().map(|os| os.to_str()).unwrap_or_default() {
                        Some(KEY_FILE_EXTENSION) => Some(path),
                        _ => None,
                    }
                } else {
                    None
                }
            })
            .collect())
    }

    fn generate_file_path(&self, name: &str) -> PathBuf {
        self.0.join(format!("{name}.{KEY_FILE_EXTENSION}"))
    }
}

impl KeyManager for KeyDir {
    fn get(&self, name: &str) -> Result<Option<KeyPair>> {
        let path = self.generate_file_path(name);
        match read_key(path) {
            Ok(k) => Ok(Some(k)),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("Unable to load key from disk: {}", e)),
        }
    }

    fn list_names(&self) -> Result<Vec<String>> {
        Ok(self
            .list_paths()?
            .into_iter()
            .filter_map(|p| {
                p.file_stem()
                    .unwrap_or_default()
                    .to_os_string()
                    .into_string()
                    .ok()
            })
            .collect())
    }

    fn list(&self) -> Result<Vec<KeyPair>> {
        self.list_paths()?
            .into_iter()
            .map(|p| {
                read_key(p).map_err(|e| anyhow::anyhow!("Unable to load key from disk: {}", e))
            })
            .collect()
    }

    fn delete(&self, name: &str) -> Result<()> {
        match std::fs::remove_file(self.generate_file_path(name)) {
            Ok(()) => Ok(()),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("Unable to delete key from disk: {}", e)),
        }
    }

    fn save(&self, name: &str, key: &KeyPair) -> Result<()> {
        let path = self.generate_file_path(name);
        std::fs::write(&path, key.seed()?.as_bytes())
            .map_err(|e| anyhow::anyhow!("Unable to write key to disk: {}", e))?;
        set_permissions_keys(path)
    }
}

/// Helper function for reading a key from disk
pub fn read_key(p: impl AsRef<Path>) -> std::io::Result<KeyPair> {
    let raw = std::fs::read_to_string(p)?;

    KeyPair::from_seed(&raw).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
}

#[cfg(unix)]
/// Set file and folder permissions for keys.
fn set_permissions_keys(path: impl AsRef<Path>) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = path.as_ref().metadata()?;
    match metadata.file_type().is_dir() {
        true => std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?,
        false => std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?,
    };
    Ok(())
}

#[cfg(target_os = "windows")]
fn set_permissions_keys(_path: impl AsRef<Path>) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod test {
    use nkeys::KeyPairType;

    use super::*;

    const TEST_KEY: &str = "SMAAGJ4DY4FNV4VJWA6QU7UQIL7DKJR4Z3UH7NBMNTH22V6VEIJGJUBQN4";

    #[test]
    fn round_trip_happy_path() {
        let tempdir = tempfile::tempdir().expect("Unable to create temp dir");
        let key_dir = KeyDir::new(&tempdir).expect("Should be able to create key dir");

        let key1 = KeyPair::new(KeyPairType::Account);
        let key2 = KeyPair::new(KeyPairType::Module);

        key_dir
            .save("foobar_account", &key1)
            .expect("Should be able to save key");
        key_dir
            .save("foobar_module", &key2)
            .expect("Should be able to save key");

        assert_eq!(
            tempdir.path().read_dir().unwrap().count(),
            2,
            "Directory should have 2 entries"
        );

        let names = key_dir.list_names().expect("Should be able to list names");
        assert_eq!(names.len(), 2, "Should have listed 2 names");
        for name in names {
            assert!(
                name == "foobar_account" || name == "foobar_module",
                "Should only have the newly created keys in the list"
            );
        }

        let key = key_dir
            .get("foobar_module")
            .expect("Shouldn't error while reading key")
            .expect("Key should exist");
        assert_eq!(
            key.public_key(),
            key2.public_key(),
            "Should have fetched the right key from disk"
        );

        assert_eq!(
            key_dir
                .list()
                .expect("Should be able to load all keys")
                .len(),
            2,
            "Should have loaded 2 keys from disk"
        );

        key_dir
            .delete("foobar_account")
            .expect("Should be able to delete key");
        assert_eq!(
            tempdir.path().read_dir().unwrap().count(),
            1,
            "Directory should have 1 entry"
        );
    }

    #[test]
    fn can_read_existing() {
        let tempdir = tempfile::tempdir().expect("Unable to create temp dir");
        std::fs::write(tempdir.path().join("foobar_module.nk"), TEST_KEY)
            .expect("Unable to write test file");
        // Write a file that should be skipped
        std::fs::write(tempdir.path().join("blah"), TEST_KEY).expect("Unable to write test file");

        let key_dir = KeyDir::new(&tempdir).expect("Should be able to create key dir");

        assert_eq!(
            key_dir
                .list_names()
                .expect("Should be able to list existing keys")
                .len(),
            1,
            "Should only have 1 key on disk"
        );

        let key = key_dir
            .get("foobar_module")
            .expect("Should be able to load key from disk")
            .expect("Key should exist");
        assert_eq!(
            key.seed().unwrap(),
            TEST_KEY,
            "Should load the correct key from disk"
        );
    }

    #[test]
    fn delete_of_nonexistent_key_should_succeed() {
        let tempdir = tempfile::tempdir().expect("Unable to create temp dir");
        let key_dir = KeyDir::new(&tempdir).expect("Should be able to create key dir");

        key_dir
            .delete("foobar")
            .expect("Non-existent key shouldn't error");
    }
}

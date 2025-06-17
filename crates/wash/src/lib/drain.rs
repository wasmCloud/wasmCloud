//! Remove cached wasmCloud files like OCI artifacts or downloaded binaries

use std::{env, fs, io::Result, path::PathBuf};

use crate::lib::config::WASH_DIRECTORIES;

/// A type that allows you to clean up (i.e. drain) a set of caches and folders used by wasmcloud
#[derive(Debug, Clone)]
#[cfg_attr(feature = "cli", derive(clap::Subcommand))]
pub enum Drain {
    /// Remove all cached files created by wasmcloud
    All,
    /// Remove cached files downloaded from OCI registries by wasmCloud
    Oci,
    /// Remove cached binaries extracted from provider archives
    Lib,
    /// Remove files and logs from wash dev sessions
    Dev,
    /// Remove downloaded and generated files from launching wasmCloud hosts
    Downloads,
}

impl IntoIterator for &Drain {
    type Item = PathBuf;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        let paths = match self {
            Drain::All => vec![
                /* Lib    */ env::temp_dir().join("wasmcloudcache"),
                /* Oci    */ env::temp_dir().join("wasmcloud_ocicache"),
                /* Downloads */ WASH_DIRECTORIES.downloads_dir(),
            ],
            Drain::Lib => vec![env::temp_dir().join("wasmcloudcache")],
            Drain::Oci => vec![env::temp_dir().join("wasmcloud_ocicache")],
            Drain::Dev => vec![WASH_DIRECTORIES.dev_dir()],
            Drain::Downloads => vec![WASH_DIRECTORIES.downloads_dir()],
        };
        paths.into_iter()
    }
}

impl Drain {
    /// Cleans up all data based on the type of Drain requested. Returns a list of paths that were
    /// cleaned
    pub fn drain(self) -> Result<Vec<PathBuf>> {
        self.into_iter()
            .filter(|path| path.exists())
            .map(remove_dir_contents)
            .collect::<Result<Vec<PathBuf>>>()
    }
}

fn remove_dir_contents(path: PathBuf) -> Result<PathBuf> {
    for entry in fs::read_dir(&path)? {
        let path = entry?.path();
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else if path.is_file() {
            fs::remove_file(&path)?;
        }
    }
    Ok(path)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_dir_clean() {
        let tempdir = tempfile::tempdir().expect("Unable to create tempdir");

        let subdir = tempdir.path().join("foobar");
        fs::create_dir(&subdir).unwrap();

        // Create the files and drop the handles
        {
            fs::File::create(subdir.join("baz")).unwrap();
            fs::File::create(tempdir.path().join("baz")).unwrap();
        }

        remove_dir_contents(tempdir.path().to_owned())
            .expect("Shouldn't get an error when cleaning files");
        assert!(
            tempdir
                .path()
                .read_dir()
                .expect("Top level dir should still exist")
                .next()
                .is_none(),
            "Directory should be empty"
        );
    }
}

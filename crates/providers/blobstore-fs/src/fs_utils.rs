use std::path::{Path, PathBuf};
use std::vec::Vec;

/// Traverses a file system starting at location `root` and returning a list of all directories
/// contained in that directory, recursively, relative to the original root at level 0.
pub fn all_dirs(root: &Path, prefix: &Path, depth: u32) -> Vec<PathBuf> {
    if depth > 1000 {
        return vec![];
    }
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(root) {
        Ok(rd) => rd
            .filter(|e| match e {
                Ok(entry) => match entry.file_type() {
                    Ok(ft) => ft.is_dir(),
                    _ => false,
                },
                _ => false,
            })
            .map(|e| PathBuf::from(e.unwrap().path().as_path().strip_prefix(prefix).unwrap()))
            .collect(),
        Err(e) => {
            panic!("Could not read directories at {:?}: {}", root, e)
        }
    };

    // Now recursively go in all directories and collect all sub-directories
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for dir in &dirs {
        let mut local_subdirs = all_dirs(prefix.join(dir.as_path()).as_path(), prefix, depth + 1);
        subdirs.append(&mut local_subdirs);
    }
    dirs.append(&mut subdirs);
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::fs::{create_dir_all, remove_dir_all};

    fn clear_state(r: &Path) {
        if let Err(e) = remove_dir_all(r) {
            println!("Error in remove_dir_all: {}", e);
        }
    }

    #[test]
    fn one_dir() {
        // give each test a different root otherwise they can't run in parallel
        let root = Path::new("/tmp/rust_test/test1");
        if let Err(e) = create_dir_all(root.join("dir1").as_path()) {
            panic!(
                "Error in create_dir_all({:?}): {}",
                root.join("dir1").as_path(),
                e
            );
        }

        let dirs = all_dirs(root, root, 0);

        clear_state(root);

        assert_eq!(dirs, vec![PathBuf::from(r"dir1")]);
    }

    #[test]
    fn many_dirs() {
        // give each test a different root otherwise they can't run in parallel
        let root = Path::new("/tmp/rust_test/test2");
        if let Err(e) = create_dir_all(root.join("dir1").as_path()) {
            panic!("Error in create_dir_all: {}", e);
        }
        if let Err(e) = create_dir_all(root.join("dir2/dir3").as_path()) {
            panic!("Error in create_dir_all: {}", e);
        }

        let dirs = all_dirs(root, root, 0);

        clear_state(root);

        assert!(dirs.contains(&PathBuf::from(r"dir1")));
        assert!(dirs.contains(&PathBuf::from(r"dir2")));
        assert!(dirs.contains(&PathBuf::from(r"dir2/dir3")));
    }

    #[test]
    fn many_dirs_with_files() {
        // give each test a different root otherwise they can't run in parallel
        let root = Path::new("/tmp/rust_test/test3");
        if let Err(e) = create_dir_all(root.join("dir1").as_path()) {
            panic!("Error in create_dir_all: {}", e);
        }
        if let Err(e) = create_dir_all(root.join("dir2/dir3").as_path()) {
            panic!("Error in create_dir_all: {}", e);
        }

        File::create(root.join("dir2/foo.txt").as_path()).unwrap();

        let dirs = all_dirs(root, root, 0);

        clear_state(root);

        assert!(dirs.contains(&PathBuf::from(r"dir1")));
        assert!(dirs.contains(&PathBuf::from(r"dir2")));
        assert!(!dirs.contains(&PathBuf::from(r"foo.txt")));
        assert!(dirs.contains(&PathBuf::from(r"dir2/dir3")));
    }
}

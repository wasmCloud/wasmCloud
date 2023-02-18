use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use atelier_core::model::Model;
use reqwest::Url;
use rustc_hash::FxHasher;

use crate::{
    config::ModelSource,
    error::{Error, Result},
};

/// maximum number of parallel downloader threads
const MAX_PARALLEL_DOWNLOADS: u16 = 8;
/// how long cached smithy file can be used before we attempt to download another
const CACHED_FILE_MAX_AGE: Duration = Duration::from_secs(60 * 60 * 24); // one day
const SMITHY_CACHE_ENV_VAR: &str = "SMITHY_CACHE";
const SMITHY_CACHE_NO_EXPIRE: &str = "NO_EXPIRE";

/// Load all model sources and merge into single model.
/// - Sources may be a combination of files, directories, and urls.
/// - Model files may be .smithy or .json
/// See the codegen.toml documentation on `[[models]]` for
/// a description of valid ModelSources.
/// If `relative_dir` is provided, all relative paths read will be made relative to that folder,
/// (Relative paths in codegen.toml are relative to the file codegen.toml, not
/// necessarily the current directory of the OS process)
/// Returns single merged model.
pub fn sources_to_model(sources: &[ModelSource], base_dir: &Path, verbose: u8) -> Result<Model> {
    let paths = sources_to_paths(sources, base_dir, verbose)?;
    let mut assembler = atelier_assembler::ModelAssembler::default();
    for path in paths.iter() {
        if !path.exists() {
            return Err(Error::MissingFile(format!(
                "'{}' is not a valid path to a file or directory",
                path.display(),
            )));
        }
        let _ = assembler.push(path);
    }
    let model: Model = assembler
        .try_into()
        .map_err(|e| Error::Model(format!("assembling model: {e:#?}")))?;
    Ok(model)
}

/// Flatten source lists and collect list of paths to local files.
/// All returned paths that were relative have been joined to base_dir.
/// Download any urls to cache dir if they aren't already cached.
/// If any of the source paths are local directories, they are passed
/// to the result and the caller is expected to traverse them
/// or pass them to an Assembler for traversal.
#[doc(hidden)]
pub(crate) fn sources_to_paths(
    sources: &[ModelSource],
    base_dir: &Path,
    verbose: u8,
) -> Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    let mut urls = Vec::new();

    for source in sources.iter() {
        match source {
            ModelSource::Path { path, files } => {
                let prefix = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    base_dir.join(path)
                };
                if files.is_empty() {
                    // If path is a file, it will be added; if a directory, and source.files is empty,
                    // the directory will be traversed to find model files
                    if verbose > 0 {
                        println!("DEBUG: adding path: {}", &prefix.display());
                    }
                    results.push(prefix)
                } else {
                    for file in files.iter() {
                        let path = prefix.join(file);
                        if verbose > 0 {
                            println!("DEBUG: adding path: {}", &path.display());
                        }
                        results.push(path);
                    }
                }
            }
            ModelSource::Url { url, files } => {
                if files.is_empty() {
                    if verbose > 0 {
                        println!("DEBUG: adding url: {url}");
                    }
                    urls.push(url.to_string());
                } else {
                    for file in files.iter() {
                        let url = format!(
                            "{}{}{}",
                            url,
                            if !url.ends_with('/') && !file.starts_with('/') { "/" } else { "" },
                            file
                        );
                        if verbose > 0 {
                            println!("DEBUG: adding url: {}", &url);
                        }
                        urls.push(url);
                    }
                }
            }
        }
    }
    if !urls.is_empty() {
        let cached = urls_to_cached_files(urls)?;
        results.extend_from_slice(&cached);
    }
    Ok(results)
}

/// Returns cache_path, relative to download directory
/// format: host_dir/file_stem.HASH.ext
fn url_to_cache_path(url: &str) -> Result<PathBuf> {
    let origin = url.parse::<Url>().map_err(|e| bad_url(url, e))?;
    let host_dir = origin.host_str().ok_or_else(|| bad_url(url, "no-host"))?;
    let file_name = PathBuf::from(
        origin
            .path_segments()
            .ok_or_else(|| bad_url(url, "path"))?
            .last()
            .map(|s| s.to_string())
            .ok_or_else(|| bad_url(url, "last-path"))?,
    );
    let file_stem = file_name
        .file_stem()
        .map(|s| s.to_str())
        .unwrap_or_default()
        .unwrap_or("index");
    let file_ext = file_name
        .extension()
        .map(|s| s.to_str())
        .unwrap_or_default()
        .unwrap_or("raw");
    let new_file_name = format!("{}.{:x}.{}", file_stem, hash(origin.path()), file_ext);
    let path = PathBuf::from(host_dir).join(new_file_name);
    Ok(path)
}

/// Locate the weld cache directory
#[doc(hidden)]
pub fn weld_cache_dir() -> Result<PathBuf> {
    let dirs = directories::BaseDirs::new()
        .ok_or_else(|| Error::Other("invalid home directory".to_string()))?;
    let weld_cache = dirs.cache_dir().join("smithy");
    Ok(weld_cache)
}

/// Returns true if the file is older than the specified cache age.
/// If the environment contains SMITHY_CACHE=NO_EXPIRE, the file age is ignored and false is returned.
pub fn cache_expired(path: &Path) -> bool {
    if let Ok(cache_flag) = std::env::var(SMITHY_CACHE_ENV_VAR) {
        if cache_flag == SMITHY_CACHE_NO_EXPIRE {
            return false;
        }
    }
    if let Ok(md) = std::fs::metadata(path) {
        if let Ok(modified) = md.modified() {
            if let Ok(age) = modified.elapsed() {
                return age >= CACHED_FILE_MAX_AGE;
            }
        }
    }
    // If the OS can't read the file timestamp, assume it's expired and return true.
    true
}

/// Returns a list of cached files for a list of urls. Files that are not present in the cache are fetched
/// with a parallel downloader. This function fails if any file cannot be retrieved.
/// Files are downloaded into a temp dir, so that if there's a download error they don't overwrite
/// any cached values
fn urls_to_cached_files(urls: Vec<String>) -> Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    let mut to_download = Vec::new();

    let weld_cache = weld_cache_dir()?;

    let tmpdir =
        tempfile::tempdir().map_err(|e| Error::Io(format!("creating temp folder: {e}")))?;
    for url in urls.iter() {
        let rel_path = url_to_cache_path(url)?;
        let cache_path = weld_cache.join(&rel_path);
        if cache_path.is_file() && !cache_expired(&cache_path) {
            // found cached file
            results.push(cache_path);
        } else {
            // no cache file (or expired), download to temp dir
            let temp_path = tmpdir.path().join(&rel_path);
            std::fs::create_dir_all(temp_path.parent().unwrap()).map_err(|e| {
                crate::Error::Io(format!(
                    "creating folder {}: {}",
                    &temp_path.parent().unwrap().display(),
                    e,
                ))
            })?;
            let dl = downloader::Download::new(url).file_name(&temp_path);
            to_download.push(dl);
        }
    }

    if !to_download.is_empty() {
        let mut downloader = downloader::Downloader::builder()
            .download_folder(tmpdir.path())
            .parallel_requests(MAX_PARALLEL_DOWNLOADS)
            .build()
            .map_err(|e| Error::Other(format!("internal error: download failure: {e}")))?;
        // invoke parallel downloader, returns when all have been read
        let result = downloader
            .download(&to_download)
            .map_err(|e| Error::Other(format!("download error: {e}")))?;

        for r in result.iter() {
            match r {
                Err(e) => {
                    println!("Failure downloading: {e}");
                }
                Ok(summary) => {
                    for status in summary.status.iter() {
                        if (200..300).contains(&status.1) {
                            // take first with status ok
                            let downloaded_file = &summary.file_name;
                            let rel_path = downloaded_file.strip_prefix(&tmpdir).map_err(|e| {
                                Error::Other(format!("internal download error {e}"))
                            })?;
                            let cache_file = weld_cache.join(rel_path);
                            std::fs::create_dir_all(cache_file.parent().unwrap()).map_err(|e| {
                                Error::Io(format!(
                                    "creating folder {}: {}",
                                    &cache_file.parent().unwrap().display(),
                                    e
                                ))
                            })?;
                            std::fs::copy(downloaded_file, &cache_file).map_err(|e| {
                                Error::Other(format!(
                                    "writing cache file {}: {}",
                                    &cache_file.display(),
                                    e
                                ))
                            })?;
                            results.push(cache_file);
                            break;
                        } else {
                            println!("Warning: url '{}' got status {}", status.0, status.1);
                        }
                    }
                }
            };
        }
    }
    if results.len() != urls.len() {
        Err(Error::Other(format!(
            "Quitting - {} model files could not be downloaded and were not found in the cache. \
             If you have previously built this project and are working \"offline\", try setting \
             SMITHY_CACHE=NO_EXPIRE in the environment",
            urls.len() - results.len()
        )))
    } else {
        Ok(results)
    }
}

fn bad_url<E: std::fmt::Display>(s: &str, e: E) -> Error {
    Error::Other(format!("bad url {s}: {e}"))
}

#[cfg(test)]
type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn test_cache_path() -> TestResult {
    assert_eq!(
        "localhost/file.1dc75e4e94bec8fd.smithy",
        url_to_cache_path("http://localhost/path/file.smithy")
            .unwrap()
            .to_str()
            .unwrap()
    );

    assert_eq!(
        "localhost/file.cd93a55565eb790a.smithy",
        url_to_cache_path("http://localhost/path/to/file.smithy")
            .unwrap()
            .to_str()
            .unwrap(),
        "hash changes with path"
    );

    assert_eq!(
        "localhost/file.1dc75e4e94bec8fd.smithy",
        url_to_cache_path("http://localhost:8080/path/file.smithy")
            .unwrap()
            .to_str()
            .unwrap(),
        "hash is not dependent on port",
    );

    assert_eq!(
        "127.0.0.1/file.1dc75e4e94bec8fd.smithy",
        url_to_cache_path("http://127.0.0.1/path/file.smithy")
            .unwrap()
            .to_str()
            .unwrap(),
        "hash is not dependent on host",
    );

    assert_eq!(
        "127.0.0.1/foo.3f066558cb61d00f.raw",
        url_to_cache_path("http://127.0.0.1/path/foo").unwrap().to_str().unwrap(),
        "generate .raw for missing extension",
    );

    assert_eq!(
        "127.0.0.1/index.ce34ccb3ff9b34cd.raw",
        url_to_cache_path("http://127.0.0.1/dir/").unwrap().to_str().unwrap(),
        "generate index.raw for missing filename",
    );

    Ok(())
}

fn hash(s: &str) -> u64 {
    use std::hash::Hasher;
    let mut hasher = FxHasher::default();
    hasher.write(s.as_bytes());
    hasher.finish()
}

#[test]
fn test_hash() {
    assert_eq!(0, hash(""));
    assert_eq!(18099358241699475913, hash("hello"));
}

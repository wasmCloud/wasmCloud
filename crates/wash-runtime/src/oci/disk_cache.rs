use super::{AuthId, TrustId};
use anyhow::{Context, Result, bail};
use hmac::{Hmac, Mac};
use oci_client::secrets::RegistryAuth;
use sha2::{Digest as _, Sha256};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::AsyncReadExt as _;
use tracing::debug;

/// The random secret entry names are keyed with, in the cache root.
const KEY_FILE: &str = "cache.key";
/// Prefix of every entry this version reads and writes.
const ENTRY_PREFIX: &str = "v3-";

/// Who an entry was fetched as: the registry user, not the password that
/// proved it, so rotating a password keeps the user's entries. `Never` also
/// needs the password that last authenticated them: see
/// [`CacheManager::credential_matches`].
/// A different user, or anonymous access, has its own entries. A bearer
/// token names no user, so the token is its own scope.
struct Principal([u8; 32]);

impl Principal {
    fn of(auth: &RegistryAuth) -> Self {
        let mut hasher = Sha256::new();
        // Length-prefixed, so no two different principals hash the same bytes.
        let mut field = |bytes: &[u8]| {
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        };
        match auth {
            RegistryAuth::Anonymous => field(b"anonymous"),
            RegistryAuth::Basic(user, _password) => {
                field(b"basic");
                field(user.as_bytes());
            }
            RegistryAuth::Bearer(token) => {
                field(b"bearer");
                field(token.as_bytes());
            }
        }
        Self(hasher.finalize().into())
    }
}

/// Cache manager for OCI artifacts
pub(super) struct CacheManager {
    cache_dir: PathBuf,
    principal: Principal,
    /// The full credentials, password included, for [`Self::credential_proof`].
    credential: AuthId,
    insecure: bool,
    trust: TrustId,
    /// HMAC-SHA256 keyed with the cache root's secret, cloned per entry name.
    mac: Hmac<Sha256>,
}

impl CacheManager {
    /// Offline reuse requires the registry user, transport and extra roots that
    /// populated the entry. Older layouts cannot establish that provenance.
    ///
    /// Creates the cache root's key on first use.
    pub(super) async fn open(
        cache_dir: PathBuf,
        auth: &RegistryAuth,
        insecure: bool,
        trust: TrustId,
    ) -> Result<Self> {
        let key = tokio::task::spawn_blocking({
            let cache_dir = cache_dir.clone();
            move || load_or_create_key(&cache_dir)
        })
        .await
        .context("OCI cache key task failed")??;
        let mac = Hmac::<Sha256>::new_from_slice(&key)
            .map_err(|err| anyhow::anyhow!("invalid OCI cache key: {err}"))?;
        Ok(Self {
            cache_dir,
            principal: Principal::of(auth),
            credential: AuthId::of(auth),
            insecure,
            trust,
            mac,
        })
    }

    /// Get the cache directory for a given OCI reference
    ///
    /// Named by an HMAC keyed with the cache root's secret: the name derives
    /// from the registry user or bearer token, and an unkeyed hash would let
    /// anyone who can list the directory test guesses at them offline.
    pub(super) fn get_cache_dir(&self, reference: &str) -> PathBuf {
        let mut mac = self.mac.clone();
        mac.update(b"wash-oci-cache-v3\0");
        mac.update(&self.principal.0);
        mac.update(&[u8::from(self.insecure)]);
        mac.update(&self.trust.0);
        mac.update(reference.as_bytes());
        let hash = mac.finalize().into_bytes();
        self.cache_dir.join(format!("{ENTRY_PREFIX}{hash:x}"))
    }

    /// The digest and component share one file, published by one rename, so
    /// a reader cannot pair different pulls. Older two-file entries are ignored.
    pub(super) fn get_entry_path(&self, reference: &str) -> PathBuf {
        self.get_cache_dir(reference).join("entry")
    }

    /// Keyed like entry names, so the file holding it does not let anyone who
    /// can read the cache test password guesses offline.
    fn credential_proof(&self) -> String {
        let mut mac = self.mac.clone();
        mac.update(b"wash-oci-cache-credential-v1\0");
        mac.update(&self.credential.0);
        format!("{:x}", mac.finalize().into_bytes())
    }

    fn get_credential_path(&self, reference: &str) -> PathBuf {
        self.get_cache_dir(reference).join("credential")
    }

    /// Whether these credentials, password included, are the ones that last
    /// fetched or revalidated the entry. Entries are shared by everyone
    /// presenting the same user, so `Never`, which cannot ask the registry,
    /// needs this to refuse a wrong password. A rotated password passes once a
    /// pull that asks the registry has revalidated the entry with it.
    pub(super) async fn credential_matches(&self, reference: &str) -> bool {
        tokio::fs::read_to_string(self.get_credential_path(reference))
            .await
            .is_ok_and(|proof| proof == self.credential_proof())
    }

    /// Record that these credentials have just been accepted by the registry
    /// for the entry, without rewriting its bytes.
    pub(super) async fn record_credential(&self, reference: &str) -> Result<()> {
        write_replacing(
            &self.get_cache_dir(reference),
            "credential",
            self.credential_proof().into_bytes(),
        )
        .await
    }

    /// Check if an artifact is cached.
    pub(super) async fn is_cached(&self, reference: &str) -> bool {
        tokio::fs::metadata(self.get_entry_path(reference))
            .await
            .is_ok()
    }

    /// Read cached artifact, returning (component_data, digest)
    pub(super) async fn read_cached(&self, reference: &str) -> Result<(Vec<u8>, String)> {
        let entry_path = self.get_entry_path(reference);
        debug!(entry_path = %entry_path.display(), "reading cached artifact");
        let file = tokio::fs::File::open(&entry_path).await.with_context(|| {
            format!("failed to open cached artifact at {}", entry_path.display())
        })?;
        read_entry(file)
            .await
            .with_context(|| format!("invalid cached artifact at {}", entry_path.display()))
    }

    /// Write artifact and digest to cache
    ///
    /// The whole entry is written beside its final name and renamed into
    /// place. Concurrent writers each publish a complete pair; the last wins.
    pub(super) async fn write_to_cache(
        &self,
        reference: &str,
        data: &[u8],
        digest: &str,
    ) -> Result<()> {
        if digest.contains('\n') {
            bail!("digest {digest:?} contains a newline");
        }
        let cache_dir = self.get_cache_dir(reference);

        debug!(cache_dir = %cache_dir.display(), "writing to cache");

        tokio::fs::create_dir_all(&cache_dir)
            .await
            .with_context(|| format!("failed to create cache directory {}", cache_dir.display()))?;

        let mut entry = Vec::with_capacity(digest.len() + 1 + data.len());
        entry.extend_from_slice(digest.as_bytes());
        entry.push(b'\n');
        entry.extend_from_slice(data);
        write_replacing(&cache_dir, "entry", entry).await?;
        self.record_credential(reference).await
    }
}

async fn read_entry(mut file: tokio::fs::File) -> Result<(Vec<u8>, String)> {
    let capacity = usize::try_from(file.metadata().await?.len())
        .context("cached artifact is too large for this platform")?;
    let mut entry = Vec::with_capacity(capacity);
    file.read_to_end(&mut entry).await?;
    let newline = entry
        .iter()
        .position(|b| *b == b'\n')
        .context("cached artifact has no digest")?;
    let digest = std::str::from_utf8(
        entry
            .get(..newline)
            .context("invalid cached digest length")?,
    )
    .context("cached artifact has an invalid digest")?
    .to_owned();
    // Keep the large allocation with the component, not the short digest.
    drop(entry.drain(..=newline));
    Ok((entry, digest))
}

/// Expire old artifacts from the cache, and remove entries written before
/// entries were scoped by credentials: they are never read again.
pub(super) async fn expire_artifacts(cache_dir: &Path, age: Duration) -> Result<()> {
    // walk the cache directory, looking for artifact dirs
    // and remove those older than the specified age
    let mut dir_entries = match tokio::fs::read_dir(cache_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).context("failed to read cache directory"),
    };
    while let Some(entry) = dir_entries
        .next_entry()
        .await
        .context("failed to read cache entry")?
    {
        let metadata = entry
            .metadata()
            .await
            .context("failed to read cache entry metadata")?;
        if metadata.is_dir() && is_legacy_entry(&entry.path(), &entry.file_name()).await {
            debug!(path = %entry.path().display(), "removing unscoped legacy cache entry");
            tokio::fs::remove_dir_all(entry.path())
                .await
                .context("failed to remove legacy cache entry")?;
            continue;
        }
        // The cache root may be a directory the operator also uses, so only
        // what this cache writes is expired. The key itself never is: that
        // would orphan every entry keyed with it.
        let ours = if metadata.is_dir() {
            is_entry_name(&entry.file_name())
        } else {
            is_key_temp_name(&entry.file_name())
        };
        if !ours {
            continue;
        }
        if let Ok(modified) = metadata.modified() {
            let modified_duration = modified
                .elapsed()
                .context("failed to compute modified duration")?;
            if modified_duration > age {
                debug!(path = %entry.path().display(), "expiring cached artifact");
                if metadata.is_dir() {
                    tokio::fs::remove_dir_all(entry.path())
                        .await
                        .context("failed to remove expired cache entry")?;
                } else {
                    // A key file left half-published by a crash.
                    tokio::fs::remove_file(entry.path())
                        .await
                        .context("failed to remove expired cache file")?;
                }
            }
        }
    }

    Ok(())
}

/// A directory [`CacheManager::get_cache_dir`] names: the prefix and a
/// lowercase hex HMAC-SHA256.
fn is_entry_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix(ENTRY_PREFIX))
        .is_some_and(|hash| hash.len() == 64 && is_lower_hex(hash))
}

/// A key file [`load_or_create_key`] left behind, e.g. after a crash.
fn is_key_temp_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.strip_prefix(KEY_FILE)?.strip_prefix('.'))
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .is_some_and(|uuid| uuid.len() == 32 && is_lower_hex(uuid))
}

fn is_lower_hex(s: &str) -> bool {
    s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// An entry in the layout before entries were scoped by credentials:
/// `<sanitized reference>_<hash8>/` holding a `digest`. Matched narrowly, since
/// the cache root may be a directory the operator also uses for other things.
async fn is_legacy_entry(path: &Path, name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    !name.starts_with(ENTRY_PREFIX)
        && name
            .rsplit_once('_')
            .is_some_and(|(_, hash)| hash.len() == 8 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        && tokio::fs::metadata(path.join("digest")).await.is_ok()
}

/// Replace `dir/name` with `data` without a reader ever seeing it partly
/// written.
async fn write_replacing(dir: &Path, name: &str, data: Vec<u8>) -> Result<()> {
    let path = dir.join(name);
    let temp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    if let Err(err) = tokio::fs::write(&temp, data).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(err).with_context(|| format!("failed to write cache file {}", temp.display()));
    }
    if let Err(err) = tokio::fs::rename(&temp, &path).await {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(err).with_context(|| format!("failed to write cache file {}", path.display()));
    }
    Ok(())
}

/// The cache root's secret, created on first use and readable only by its
/// owner on Unix. It is never rotated: entries keyed with an earlier secret
/// would simply be fetched again.
fn load_or_create_key(cache_dir: &Path) -> Result<[u8; 32]> {
    let path = cache_dir.join(KEY_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => return parse_key(&path, &bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", path.display()));
        }
    }
    std::fs::create_dir_all(cache_dir)
        .with_context(|| format!("failed to create cache directory {}", cache_dir.display()))?;

    // Two v4 UUIDs: 244 bits from the OS random source.
    let mut key = Vec::with_capacity(32);
    key.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    key.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    let temp = cache_dir.join(format!("{KEY_FILE}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let written = options.open(&temp).and_then(|mut file| {
        file.write_all(&key)?;
        file.sync_all()
    });
    if let Err(err) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(err).with_context(|| format!("failed to write {}", temp.display()));
    }
    // Published whole, and only if no other process got there first, so every
    // process sharing the root agrees on one key.
    let published = match std::fs::hard_link(&temp, &path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        // No hard links on this filesystem. A racing process may replace the
        // key, which only costs the entries it keyed.
        Err(_) => std::fs::rename(&temp, &path),
    };
    let _ = std::fs::remove_file(&temp);
    published.with_context(|| format!("failed to create {}", path.display()))?;
    let bytes =
        std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    parse_key(&path, &bytes)
}

fn parse_key(path: &Path, bytes: &[u8]) -> Result<[u8; 32]> {
    match <[u8; 32]>::try_from(bytes) {
        Ok(key) => Ok(key),
        Err(_) => bail!(
            "OCI cache key {} is not 32 bytes; remove it to start a new cache",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn anonymous_cache(dir: &Path) -> CacheManager {
        CacheManager::open(
            dir.to_path_buf(),
            &RegistryAuth::Anonymous,
            false,
            TrustId([0; 32]),
        )
        .await
        .unwrap()
    }

    /// Writers racing on one reference each publish a whole entry, so every
    /// read returns a digest with the bytes that were written with it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_writes_keep_digest_with_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = std::sync::Arc::new(anonymous_cache(dir.path()).await);
        let reference = "localhost:5000/test:moving";
        let versions: [(&str, Vec<u8>); 2] = [
            ("sha256:v1", vec![1u8; 256 * 1024]),
            ("sha256:v2", vec![2u8; 256 * 1024]),
        ];

        let mut writers = tokio::task::JoinSet::new();
        for round in 0..64 {
            let cache = std::sync::Arc::clone(&cache);
            let (digest, data) = versions.get(round % 2).unwrap().clone();
            writers.spawn(async move {
                cache
                    .write_to_cache(reference, &data, digest)
                    .await
                    .unwrap();
            });
        }
        while !writers.is_empty() {
            if cache.is_cached(reference).await {
                let (data, digest) = cache.read_cached(reference).await.unwrap();
                let expected = versions.iter().find(|(d, _)| *d == digest).unwrap();
                assert!(
                    data == expected.1,
                    "{digest} was read with another pull's bytes"
                );
            }
            if let Some(joined) = writers.try_join_next() {
                joined.unwrap();
            } else {
                tokio::task::yield_now().await;
            }
        }
        let (data, digest) = cache.read_cached(reference).await.unwrap();
        let expected = versions.iter().find(|(d, _)| *d == digest).unwrap();
        assert!(data == expected.1);
        let leftovers: Vec<_> = std::fs::read_dir(cache.get_cache_dir(reference))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name != "entry" && name != "credential")
            .collect();
        assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");
    }

    /// Force publication between the reader's open and its data/digest read.
    /// The production reader must finish the original pair through that handle.
    #[tokio::test]
    async fn open_reader_keeps_its_pair_when_another_pull_publishes() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = anonymous_cache(dir.path()).await;
        let reference = "localhost:5000/test:moving";
        cache
            .write_to_cache(reference, b"component A", "sha256:A")
            .await
            .unwrap();
        let reader = tokio::fs::File::open(cache.get_entry_path(reference))
            .await
            .unwrap();

        cache
            .write_to_cache(reference, b"component B", "sha256:B")
            .await
            .unwrap();

        let (data, digest) = read_entry(reader).await.unwrap();
        assert_eq!(data, b"component A");
        assert_eq!(digest, "sha256:A");
        let (data, digest) = cache.read_cached(reference).await.unwrap();
        assert_eq!(data, b"component B");
        assert_eq!(digest, "sha256:B");
    }

    #[tokio::test]
    async fn digest_does_not_retain_the_component_allocation() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = anonymous_cache(dir.path()).await;
        let reference = "localhost:5000/test:large";
        let data = vec![42; 1024 * 1024];
        cache
            .write_to_cache(reference, &data, "sha256:large")
            .await
            .unwrap();

        let (cached_data, digest) = cache.read_cached(reference).await.unwrap();
        assert_eq!(cached_data, data);
        assert_eq!(digest, "sha256:large");
        assert!(digest.capacity() <= 1024, "capacity: {}", digest.capacity());
    }

    /// The old files can hold different pulls, even inside a scoped directory.
    #[tokio::test]
    async fn ignores_the_two_file_layout() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = anonymous_cache(dir.path()).await;
        let reference = "localhost:5000/test:v1.0.0";
        let path = cache.get_cache_dir(reference);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("component.wasm"), b"old bytes").unwrap();
        std::fs::write(path.join("digest"), "sha256:other").unwrap();

        assert!(!cache.is_cached(reference).await);
        assert!(cache.read_cached(reference).await.is_err());
    }

    #[tokio::test]
    async fn failed_publication_removes_the_temporary_entry() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache = anonymous_cache(dir.path()).await;
        let reference = "localhost:5000/test:blocked";
        // A directory cannot be replaced by the entry file on Unix or Windows.
        let path = cache.get_entry_path(reference);
        std::fs::create_dir_all(&path).unwrap();
        assert!(
            cache
                .write_to_cache(reference, b"bytes", "sha256:blocked")
                .await
                .is_err()
        );
        let files: Vec<_> = std::fs::read_dir(cache.get_cache_dir(reference))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, [std::ffi::OsString::from("entry")]);
    }

    /// A known answer for the whole disk-key message, generated independently
    /// with OpenSSL. Opening the cache also exercises HMAC setup
    /// with the dependency's default features disabled, as in normal builds.
    #[tokio::test]
    async fn cache_names_match_the_keyed_v3_scope() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(KEY_FILE), [0x0b; 32]).unwrap();
        let cache = CacheManager::open(
            dir.path().to_path_buf(),
            &RegistryAuth::Basic("user".into(), "secret".into()),
            true,
            TrustId([0x11; 32]),
        )
        .await
        .unwrap();
        assert_eq!(
            cache.get_cache_dir("registry.invalid/ns/component:v1"),
            dir.path()
                .join("v3-932821c7eb947a9643db96c9af9777d2429bbb1586b727d77bda39c0c45672cd")
        );
    }

    /// A password rotation keeps the user's entries; another user, or
    /// anonymous access, never reads them.
    #[tokio::test]
    async fn entries_are_scoped_by_user_not_password() {
        let dir = tempfile::TempDir::new().unwrap();
        let open = |auth: RegistryAuth| {
            let dir = dir.path().to_path_buf();
            async move {
                CacheManager::open(dir, &auth, false, TrustId([0; 32]))
                    .await
                    .unwrap()
            }
        };
        let basic = |user: &str, password: &str| RegistryAuth::Basic(user.into(), password.into());
        let reference = "registry/ns/c:1";
        open(basic("alice", "old"))
            .await
            .write_to_cache(reference, b"bytes", "sha256:aa")
            .await
            .unwrap();

        let rotated = open(basic("alice", "rotated")).await;
        assert!(rotated.is_cached(reference).await);
        // Only the password the registry last accepted passes offline.
        assert!(!rotated.credential_matches(reference).await);
        rotated.record_credential(reference).await.unwrap();
        assert!(rotated.credential_matches(reference).await);
        assert!(
            !open(basic("alice", "old"))
                .await
                .credential_matches(reference)
                .await
        );
        let proof = std::fs::read_to_string(rotated.get_credential_path(reference)).unwrap();
        assert!(!proof.contains("rotated"));

        assert!(!open(basic("bob", "old")).await.is_cached(reference).await);
        assert!(
            !open(RegistryAuth::Anonymous)
                .await
                .is_cached(reference)
                .await
        );
        assert!(
            !open(RegistryAuth::Bearer("alice".into()))
                .await
                .is_cached(reference)
                .await
        );
    }

    #[tokio::test]
    async fn the_key_is_created_once_and_reused() {
        let dir = tempfile::TempDir::new().unwrap();
        let auth = RegistryAuth::Anonymous;
        let first = CacheManager::open(dir.path().to_path_buf(), &auth, false, TrustId([0; 32]))
            .await
            .unwrap();
        let second = CacheManager::open(dir.path().to_path_buf(), &auth, false, TrustId([0; 32]))
            .await
            .unwrap();
        assert_eq!(first.get_cache_dir("r"), second.get_cache_dir("r"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.path().join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "key file mode {mode:o}");
        }

        // Another cache root has its own key, so the same reference and
        // credentials name a different entry.
        let other = tempfile::TempDir::new().unwrap();
        let other = CacheManager::open(other.path().to_path_buf(), &auth, false, TrustId([0; 32]))
            .await
            .unwrap();
        assert_ne!(
            first.get_cache_dir("r").file_name(),
            other.get_cache_dir("r").file_name()
        );
    }

    #[tokio::test]
    async fn cleanup_removes_legacy_entries_and_keeps_the_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let manager = CacheManager::open(
            dir.path().to_path_buf(),
            &RegistryAuth::Anonymous,
            false,
            TrustId([0; 32]),
        )
        .await
        .unwrap();
        manager
            .write_to_cache("registry/ns/c:1", b"bytes", "sha256:aa")
            .await
            .unwrap();
        let legacy = dir.path().join("registry_ns_c_1_0123abcd");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("digest"), "sha256:aa").unwrap();
        let unrelated = dir.path().join("not_a_cache_entry");
        std::fs::create_dir_all(&unrelated).unwrap();

        expire_artifacts(dir.path(), Duration::from_secs(3600))
            .await
            .unwrap();
        assert!(!legacy.exists());
        assert!(unrelated.exists());
        assert!(dir.path().join(KEY_FILE).exists());
        assert!(manager.is_cached("registry/ns/c:1").await);
    }

    /// Expiry removes only what this cache writes, however old the operator's
    /// own files in the same directory are.
    #[tokio::test]
    async fn expiry_leaves_files_the_cache_did_not_write() {
        let dir = tempfile::TempDir::new().unwrap();
        let manager = anonymous_cache(dir.path()).await;
        manager
            .write_to_cache("registry/ns/c:1", b"bytes", "sha256:aa")
            .await
            .unwrap();
        let entry = manager.get_cache_dir("registry/ns/c:1");
        let key_temp = dir
            .path()
            .join(format!("{KEY_FILE}.{}.tmp", uuid::Uuid::new_v4().simple()));
        std::fs::write(&key_temp, b"partial").unwrap();
        let operator_dir = dir.path().join("operator-data");
        std::fs::create_dir_all(&operator_dir).unwrap();
        let operator_file = dir.path().join("notes.txt");
        std::fs::write(&operator_file, b"keep me").unwrap();
        let lookalike = dir.path().join(format!("{ENTRY_PREFIX}not-a-hash"));
        std::fs::create_dir_all(&lookalike).unwrap();

        // With a zero age everything counts as old once its timestamp is in
        // the past, which a coarse filesystem clock may take a moment to show.
        let paths = [&entry, &key_temp, &operator_dir, &operator_file, &lookalike];
        tokio::time::timeout(Duration::from_secs(10), async {
            while paths.iter().any(|path| {
                std::fs::metadata(path)
                    .and_then(|m| m.modified())
                    .is_ok_and(|modified| modified.elapsed().unwrap_or_default().is_zero())
            }) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("timestamps fall into the past");

        expire_artifacts(dir.path(), Duration::ZERO).await.unwrap();
        assert!(!entry.exists());
        assert!(!key_temp.exists());
        assert!(operator_dir.exists());
        assert!(operator_file.exists());
        assert!(lookalike.exists());
        assert!(dir.path().join(KEY_FILE).exists());
    }
}

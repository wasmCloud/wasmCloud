# Runtime scaling changes

These changes affect embedders and operators upgrading the runtime.

## OCI pulls and caching

- OCI disk entries are scoped by the resolved credentials. Existing unscoped
  entries are ignored and will need to be fetched again. `cleanup_cache` (the
  washlet's periodic cache cleanup) removes them; `wash dev` and `wash oci`
  never clean their cache, so delete the old `<sanitized reference>_<hash>`
  directories under `<wash cache dir>/oci` by hand.
- Entry names are keyed with a random secret the cache creates once as
  `cache.key` in the cache root, readable only by its owner on Unix. Keep it
  with the cache: removing it, or copying a cache without it, means every
  entry is fetched again.
- `Never` no longer contacts the registry on a cache hit, so it now works
  offline, but only within the same credential context. An air-gapped host
  upgrading with a pre-populated cache fails every `Never` start until it has
  re-pulled once with `IfNotPresent`, under the credentials it will use,
  before switching back to `Never`.
- A credential rotation (a new image pull secret, a refreshed ECR token) is a
  new credential context: images are fetched again after each rotation, and
  `Never` workloads that restart after it fail until they are re-pulled.
  Entries for the old credentials stay on disk until age-based cleanup
  removes them.
- Concurrent pulls of the same reference with the same credentials and
  settings are folded into one. A finished `IfNotPresent` or `Never` result
  answers identical requests for 5 seconds without checking the tag's digest
  again. `Always` resolves the registry afresh after any prior pull has
  completed; only pulls genuinely in flight together are coalesced.
- Changes to `SSL_CERT_FILE` / `SSL_CERT_DIR` and their bundles apply to
  subsequent pulls and invalidate client and finished-result reuse.
- Docker credential helper results are reused for 30 seconds per registry. A
  helper lookup that takes longer than 30 seconds fails, and the pull proceeds
  anonymously.

The OCI implementation separates disk-cache policy, credential resolution,
client/trust snapshots, and coalescing into `src/oci/`.

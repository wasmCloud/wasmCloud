# Runtime scaling changes

## OCI pulls and caches

Disk entries now use a `v3-` layout scoped by resolved credentials, HTTP/HTTPS
transport and the trust snapshot used to fetch or push the artifact. Older
unscoped and credential-only entries are ignored and must be fetched again.
Removing a CA or requiring HTTPS prevents reuse of an incompatible disk entry,
including a reference pinned by digest. `cleanup_cache` removes the old
`<sanitized reference>_<hash>` entries; the washlet calls it periodically.
`wash dev` and `wash oci` do not clean their cache, so remove old directories
under `<wash cache dir>/oci` manually when no longer needed.

Entry names are keyed with a random secret created once as `cache.key` in the
cache root, readable only by its owner on Unix. Keep it with the cache: removing
it, or copying a cache without it, orphans its entries. A credential rotation is
a new scope: fetch the image under the new credentials before using `Never`.
Entries for the old credentials remain until age-based cleanup removes them.

`Never` remains offline: it returns a cached artifact only inside the same
credential, transport and trust scope, and otherwise returns a cache-miss error.
Populate that scope before switching to `Never`. It cannot check whether the
registry has subsequently revoked credentials.

`Always` resolves the registry afresh after a prior pull has completed;
concurrent identical requests can share an active pull. Other policies reuse a
successful identical result for five seconds, so tag updates may take that long
to appear. Failed results are retried by the next caller.

Changes to `SSL_CERT_FILE`, `SSL_CERT_DIR` and their loaded certificate contents
apply to subsequent pulls, separating client, completed-result and disk reuse.
Explicit credentials bypass Docker credential-helper caching; successful helper
results, including anonymous resolution, can be reused for 30 seconds. Helper
errors are not cached. A helper lookup exceeding 30 seconds fails and the pull
proceeds anonymously, as for other helper failures.

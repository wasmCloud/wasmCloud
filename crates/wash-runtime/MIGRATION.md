# Runtime scaling changes

## OCI pulls and caches

Disk entries now use a `v3-` layout scoped by resolved credentials, HTTP/HTTPS
transport and the configured extra-root snapshot used to fetch or push the
artifact. Older unscoped and credential-only entries are ignored and must be
fetched again. Removing an extra CA or requiring HTTPS prevents reuse of an
incompatible disk entry,
including a reference pinned by digest. `cleanup_cache` removes the old
`<sanitized reference>_<hash>` entries; the washlet calls it periodically.
`wash dev` and `wash oci` do not clean their cache, so remove old directories
under `<wash cache dir>/oci` manually when no longer needed.

Each scoped directory stores the digest and component together in one `entry`
file, published by an atomic rename. Older `component.wasm`/`digest` pairs are
ignored because concurrent pulls could have mixed their versions. Fetch them
again before using `Never`, even if they already have a `v3-` directory name.
Old files remain until age-based cleanup removes the directory, or can be
removed manually. Cancelling a write can leave a temporary file in the directory;
temporary files are never read as entries and expire with that directory.

Entry names are keyed with a random secret created once as `cache.key` in the
cache root, readable only by its owner on Unix. Keep it with the cache: removing
it, or copying a cache without it, orphans its entries. A credential rotation is
a new scope: fetch the image under the new credentials before using `Never`.
Entries for the old credentials remain until age-based cleanup removes them.

`Never` remains offline: it returns a cached artifact only inside the same
credential, transport and trust scope, and otherwise returns a cache-miss error.
Populate that scope before switching to `Never`. It cannot check whether the
registry has subsequently revoked credentials or whether native OS trust policy
has changed; the disk scope cannot represent every platform trust decision.

`IfNotPresent` revalidates a disk entry's manifest digest with the registry,
including references pinned by digest. This applies the caller's TLS and registry
authorization policy instead of treating persistent bytes as fresh authorization.

`Always` resolves the registry afresh after a prior pull has completed;
concurrent identical requests can share an active pull. Other policies reuse a
successful identical result for five seconds, so tag updates may take that long
to appear. Failed results are retried by the next caller.

Changes to `SSL_CERT_FILE`, `SSL_CERT_DIR` and their loaded certificate contents
apply to subsequent pulls, separating client, completed-result and disk reuse.
On Linux and other Unix platforms using PEM native roots, clients use the
captured environment root set even if a bundle rotates while construction is
queued. This preserves the native loader's fallback when no certificate paths
are configured; macOS and Windows retain their platform verification policies.
Environment-scoped entries from the earlier merged-root layout are fetched again.
Explicit credentials bypass Docker credential-helper caching; successful helper
results, including anonymous resolution, can be reused for 30 seconds. Helper
errors are not cached. A helper lookup exceeding 30 seconds fails and the pull
proceeds anonymously, as for other helper failures.

Shared clients are retained for at most five minutes in a 256-entry cache.
Each entry serves one registry/repository and security scope, including its
authentication-token state. Tags and digests in that repository share a client;
different repositories do not. Active callers may hold an evicted client until
their requests finish.

Native OS trust-store changes are not part of the extra-root snapshot. In a
running process, successful-result reuse may delay revalidation by five seconds,
and a reused client's platform verifier may retain its native roots for up to
five minutes. Restart the process to apply native trust-store changes immediately;
requests already in flight can continue with the client they acquired.

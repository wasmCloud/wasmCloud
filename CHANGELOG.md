# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

<csr-id-5957fce86a928c7398370547d0f43c9498185441/>

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### Bug Fixes

 - <csr-id-5d645087bc73a3a000fa4184ea768527ca90acda/> add OTEL for messaging kafka provider
   This commit ensures OTEL is working for the messaging-kafka provider.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 2 commits contributed to the release over the course of 11 calendar days.
 - 11 days passed between releases.
 - 2 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Add OTEL for messaging kafka provider ([`5d64508`](https://github.com/wasmCloud/wasmCloud/commit/5d645087bc73a3a000fa4184ea768527ca90acda))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
</details>

## v1.0.0 (2024-04-17)

## v1.0.0-rc.2 (2024-04-13)

<csr-id-346753ab823f911b12de763225dfd154272f1d3a/>
<csr-id-9886a34a239d06b8b6c8375bb0cd0f97e3a188c3/>

### Chore

 - <csr-id-346753ab823f911b12de763225dfd154272f1d3a/> Bumps host version to rc.2
   While I was here, I fixed the issue where we were using the host crate
   version instead of the top level binary host version in our events and
   ctl API responses
 - <csr-id-9886a34a239d06b8b6c8375bb0cd0f97e3a188c3/> rename provider binaries with -provider suffix
   This commit updates the binaries for providers with a
   suffix (`-provider`) so that it's clear they're provider binaries as
   we add more binaries to the project.

### New Features

 - <csr-id-9cd2b4034f8d5688ce250429dc14120eaf61b483/> update `wrpc:keyvalue` in providers
   part of this process is adopting `wit-bindgen-wrpc` in the host

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 3 commits contributed to the release over the course of 3 calendar days.
 - 4 days passed between releases.
 - 3 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bumps host version to rc.2 ([`346753a`](https://github.com/wasmCloud/wasmCloud/commit/346753ab823f911b12de763225dfd154272f1d3a))
    - Update `wrpc:keyvalue` in providers ([`9cd2b40`](https://github.com/wasmCloud/wasmCloud/commit/9cd2b4034f8d5688ce250429dc14120eaf61b483))
    - Rename provider binaries with -provider suffix ([`9886a34`](https://github.com/wasmCloud/wasmCloud/commit/9886a34a239d06b8b6c8375bb0cd0f97e3a188c3))
</details>

## v1.0.0-rc.1 (2024-04-09)

<csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>

### New Features

 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`

### Refactor

 - <csr-id-8082135282f66b5d56fe6d14bb5ce6dc510d4b63/> remove `ProviderHandler`

### Chore (BREAKING)

 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 3 commits contributed to the release.
 - 4 days passed between releases.
 - 3 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Remove cluster_seed/cluster_issuers ([`bc5d296`](https://github.com/wasmCloud/wasmCloud/commit/bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f))
    - Switch to `wit-bindgen-wrpc` ([`322f471`](https://github.com/wasmCloud/wasmCloud/commit/322f471f9a8154224a50ec33517c9f5b1716d2d5))
    - Remove `ProviderHandler` ([`8082135`](https://github.com/wasmCloud/wasmCloud/commit/8082135282f66b5d56fe6d14bb5ce6dc510d4b63))
</details>

## v1.0.0-prealpha.1 (2024-03-13)

<csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/>
<csr-id-b882f8f76973ea6afd24dbce7a123619f0a2da25/>
<csr-id-9014806ae104a162edd1b6e8acab92d079d73cef/>

### Chore

 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-b882f8f76973ea6afd24dbce7a123619f0a2da25/> Mark enable-observability conflicting with individiual enable-<signal> flags
 - <csr-id-9014806ae104a162edd1b6e8acab92d079d73cef/> prefix NATS_PORT and NATS_URL with WASMCLOUD
   This adds a `WASMCLOUD` prefix to both the `NATS_PORT` and `NATS_URL`
   env vars, since there are potentially conficts with those variables in other
   systems. Specifically this would play badly deploying in Kubernetes with
   a service named `nats` running in the same namespace, since a
   `NATS_PORT` env var is automatically injected in that case which stops
   the wasmcloud binary from starting. It is also a good practice to have
   all of our environment variables using the same prefix.
   
   This is a breaking change to wasmCloud CLI args for the host.

### New Features

<csr-id-17648fedc2a1907b2f0c6d053b9747e72999addb/>

 - <csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/> Add flags for overriding the default OpenTelemetry endpoint
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
 - <csr-id-82c249b15dba4dbe4c14a6afd2b52c7d3dc99985/> Glues in named config to actors
   This introduces a new config bundle that can watch for config changes. There
   is probably a way to reduce the number of allocations here, but it is good
   enough for now.
   
   Also, sorry for the new file. I renamed `config.rs` to `host_config.rs` so
   I could reuse the `config.rs` file, but I forgot to git mv. So that file
   hasn't changed
 - <csr-id-7d51408440509c687b01e00b77a3672a8e8c30c9/> add invocation and error counts for actor invocations
   Add two new metrics for actors:
   * the count of the number of invocations (`wasmcloud_host.actor.invocations`)
* the count of errors (`wasmcloud_host.actor.invocation.errors`)
* the lattice ID
* the host ID
* provider information if a provider invoked the actor: ** the contract ID
   ** the provider ID
   ** the name of the linkdef

### New Features (BREAKING)

 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 9 commits contributed to the release over the course of 26 calendar days.
 - 28 days passed between releases.
 - 9 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Use traces instead of tracing user-facing language to align with OTEL signal names ([`d65512b`](https://github.com/wasmCloud/wasmCloud/commit/d65512b5e86eb4d13e64cffa220a5a842c7bb72b))
    - Mark enable-observability conflicting with individiual enable-<signal> flags ([`b882f8f`](https://github.com/wasmCloud/wasmCloud/commit/b882f8f76973ea6afd24dbce7a123619f0a2da25))
    - Add flags for overriding the default OpenTelemetry endpoint ([`6fe14b8`](https://github.com/wasmCloud/wasmCloud/commit/6fe14b89d4c26e5c01e54773268c6d0f04236e71))
    - Switch to using --enable-observability and --enable-<signal> flags ([`868570b`](https://github.com/wasmCloud/wasmCloud/commit/868570be8d94a6d73608c7cde5d2422e15f9eb0c))
    - Glues in named config to actors ([`82c249b`](https://github.com/wasmCloud/wasmCloud/commit/82c249b15dba4dbe4c14a6afd2b52c7d3dc99985))
    - Prefix NATS_PORT and NATS_URL with WASMCLOUD ([`9014806`](https://github.com/wasmCloud/wasmCloud/commit/9014806ae104a162edd1b6e8acab92d079d73cef))
    - Add invocation and error counts for actor invocations ([`7d51408`](https://github.com/wasmCloud/wasmCloud/commit/7d51408440509c687b01e00b77a3672a8e8c30c9))
    - Updates topics to the new standard ([`42d069e`](https://github.com/wasmCloud/wasmCloud/commit/42d069eee87d1b5befff1a95b49973064f1a1d1b))
    - Add initial support for metrics ([`17648fe`](https://github.com/wasmCloud/wasmCloud/commit/17648fedc2a1907b2f0c6d053b9747e72999addb))
</details>

<csr-unknown>
This also adds a bunch of new attributes to the existing actor metrics so that they make sense in an environment with multiple hosts. Specifically this adds:For actor to actor calls, instead of having the provider metadata it instead has the public key of the invoking actor.An example of what this looks like as an exported Prometheus metric:wasmcloud_host_actor_invocations_total{actor_ref="wasmcloud.azurecr.io/echo:0.3.8", caller_provider_contract_id="wasmcloud:httpserver", caller_provider_id="VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M", caller_provider_link_name="default", host="ND7L3RZ6NLYJGN25E6DKYS665ITWXAPXZXGZXLCUQEDDU65RB5TVUHEN", job="wasmcloud-host", lattice="default", operation="HttpServer.HandleRequest"}
Provider metrics will likely need to wait until the wRPC work is finished. Add initial support for metrics<csr-unknown/>

## v1.0.0-alpha.5 (2024-04-04)

<csr-id-44a7d3a0484efe5137eaaf7755a5483437c7a251/>

### Chore

 - <csr-id-44a7d3a0484efe5137eaaf7755a5483437c7a251/> document environment var for labels

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release over the course of 2 calendar days.
 - 5 days passed between releases.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Document environment var for labels ([`44a7d3a`](https://github.com/wasmCloud/wasmCloud/commit/44a7d3a0484efe5137eaaf7755a5483437c7a251))
</details>

## v1.0.0-alpha.4 (2024-03-29)

<csr-id-073b3c21581632f135d47b14b6b13ad13d7d7592/>
<csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/>

### New Features

 - <csr-id-8ce845bec7ca3f50e211d36e62fffbb0f36a0b37/> introduce interface provider running utilities
 - <csr-id-a84492d15d154a272de33680f6338379fc036a3a/> introduce provider interface sdk
 - <csr-id-f56492ac6b5e6f1274a1f11b061c42cace372122/> migrate to `wrpc:keyvalue`

### Other

 - <csr-id-073b3c21581632f135d47b14b6b13ad13d7d7592/> sync with `capability-providers`

### New Features (BREAKING)

 - <csr-id-91874e9f4bf2b37b895a4654250203144e12815c/> convert to `wrpc:blobstore`

### Refactor (BREAKING)

 - <csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/> make providers part of the workspace

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 6 commits contributed to the release over the course of 10 calendar days.
 - 10 days passed between releases.
 - 6 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Introduce interface provider running utilities ([`8ce845b`](https://github.com/wasmCloud/wasmCloud/commit/8ce845bec7ca3f50e211d36e62fffbb0f36a0b37))
    - Introduce provider interface sdk ([`a84492d`](https://github.com/wasmCloud/wasmCloud/commit/a84492d15d154a272de33680f6338379fc036a3a))
    - Migrate to `wrpc:keyvalue` ([`f56492a`](https://github.com/wasmCloud/wasmCloud/commit/f56492ac6b5e6f1274a1f11b061c42cace372122))
    - Convert to `wrpc:blobstore` ([`91874e9`](https://github.com/wasmCloud/wasmCloud/commit/91874e9f4bf2b37b895a4654250203144e12815c))
    - Sync with `capability-providers` ([`073b3c2`](https://github.com/wasmCloud/wasmCloud/commit/073b3c21581632f135d47b14b6b13ad13d7d7592))
    - Make providers part of the workspace ([`005b707`](https://github.com/wasmCloud/wasmCloud/commit/005b7073e6896f68aa64348fef44ae69305acaf7))
</details>

## v1.0.0-alpha.3 (2024-03-19)

## v1.0.0-alpha.2 (2024-03-17)

## v1.0.0-alpha.1 (2024-03-13)

## v0.82.0-rc1 (2024-02-13)

<csr-id-8e8f6d29518ec6d986fad9426fbe8224171660ab/>
<csr-id-1855d1e241f7856a0b1daaf2b79c5ccf3e823023/>
<csr-id-fe7592b1a5501f3faa8bcf3bf45edf4032e92f0b/>
<csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/>

### Chore

 - <csr-id-8e8f6d29518ec6d986fad9426fbe8224171660ab/> remove ineffective ENV aliases
   This commit removes what were supposed to be ENV aliases that don't
   work, which were introduced by https://github.com/wasmCloud/wasmCloud/pull/1243
 - <csr-id-1855d1e241f7856a0b1daaf2b79c5ccf3e823023/> Normalize wasmCloud Host as wasmcloud-host for tracing

### New Features

 - <csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/> enable OTEL logs

### Refactor

 - <csr-id-fe7592b1a5501f3faa8bcf3bf45edf4032e92f0b/> move label parsing out of host library

### Refactor (BREAKING)

 - <csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/> rename lattice prefix to just lattice

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 5 commits contributed to the release over the course of 27 calendar days.
 - 47 days passed between releases.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Enable OTEL logs ([`3602bdf`](https://github.com/wasmCloud/wasmCloud/commit/3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3))
    - Move label parsing out of host library ([`fe7592b`](https://github.com/wasmCloud/wasmCloud/commit/fe7592b1a5501f3faa8bcf3bf45edf4032e92f0b))
    - Remove ineffective ENV aliases ([`8e8f6d2`](https://github.com/wasmCloud/wasmCloud/commit/8e8f6d29518ec6d986fad9426fbe8224171660ab))
    - Rename lattice prefix to just lattice ([`6e8faab`](https://github.com/wasmCloud/wasmCloud/commit/6e8faab6a6e9f9bb7327ffb71ded2a83718920f7))
    - Normalize wasmCloud Host as wasmcloud-host for tracing ([`1855d1e`](https://github.com/wasmCloud/wasmCloud/commit/1855d1e241f7856a0b1daaf2b79c5ccf3e823023))
</details>

## v0.81.0 (2023-12-28)

### New Features

 - <csr-id-715e94e7f1a35da002769a0a25d531606f003d49/> consistently prefix cli flags

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release over the course of 1 calendar day.
 - 14 days passed between releases.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Consistently prefix cli flags ([`715e94e`](https://github.com/wasmCloud/wasmCloud/commit/715e94e7f1a35da002769a0a25d531606f003d49))
</details>

## v0.81.0-rc1 (2023-12-14)

### Bug Fixes (BREAKING)

 - <csr-id-545c21cedd1475def0648e3e700bcdd15f800c2a/> remove support for prov_rpc NATS connection

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release over the course of 28 calendar days.
 - 41 days passed between releases.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Remove support for prov_rpc NATS connection ([`545c21c`](https://github.com/wasmCloud/wasmCloud/commit/545c21cedd1475def0648e3e700bcdd15f800c2a))
</details>

## v0.81.0-alpha1 (2023-11-03)

<csr-id-8805873d6f556e1581b47846836aa249efd41caa/>
<csr-id-5af1c68bf86b62b4e2f81cbf1cc9ca1d5542ac37/>

### Chore

 - <csr-id-8805873d6f556e1581b47846836aa249efd41caa/> un-hide policy options

### Bug Fixes

 - <csr-id-ef3e4e584fef4d597cab0215fdf3cfe864f701e9/> Configure signing keys directory for build cmd
   The keys directory can be specified via wasmcloud.toml, CLI arguments (`--keys-directory`), or environment variable (`WASH_KEYS`).

### Refactor

 - <csr-id-5af1c68bf86b62b4e2f81cbf1cc9ca1d5542ac37/> `Err(anyhow!(...))` -> `bail!`, err msg capitals
   `return Err(anyhow!(...))` has been used all over the codebase over
   time, and can be comfortably converted to anyhow::bail!, which is
   easier to read and usually takes less space.
   
   In addition, for passing errors through layers of Rust code/libs,
   capitals should be avoided in error messages as the later messages may
   be wrapped (and may not be the start of the sentence), which is also
   done periodically through out the codebase.
   
   This commit converts the usages of the patterns above to be more
   consistent over time.
   
   There is a small concern here, because some of the capitalized error
   messages are now lower-cased -- this could present an issue to
   end-users but this is unlikely to be a breaking/major issue.

### New Features (BREAKING)

 - <csr-id-e394b271c13d33f24e9c6a302d17d91b3100e903/> Unifies all calls behind the ctl interface topics
   This essentially backs out all of the changes I added to 0.30 of the
   control interface. There are two main reasons behind this:
   
   First, and most important, a NATS KV bucket and a NATS topic are two
   different security contexts. In multitenant environments (like Cosmonic,
   which is where we ran into this), it is perfectly reasonable for a
   platform operator to have access to the "API" of the thing they are
   running (the ctl interface in this case), but not access to the underlying
   datastore. In order to give access to buckets, you'd have to explicitly
   grant permissions and resign tokens for each new lattice or NATS account
   (depending on your security boundary) that you spin up.
   
   Second, this unifies everything behind a single API - the NATS topics.
   Although NATS KV is still NATS, it is a weird API to essentially switch
   from requests to a "please connect to the database." It also means that
   _all_ clients have to know about how we encode and store the data, as
   well as other implementation details. Seeing as that might change in the
   future (with things like linkdefs 2.0), it seems better to have this
   behind an API.
   
   You might be saying at this point, "but wait Taylor! Didn't we do this to
   fix some issues?" Yes, we did. However, we are actually ending up with the
   same behavior as currently exists in the client. The Rust host just proxies
   through the linkdef put information and puts the key in the database. It
   doesn't update its local cache until it receives the value update from the
   bucket. It also returns its cached claims on a get links command, which is
   exactly what happens with the caching as it worked before in the client. So
   we should be in the exact state we were before, but with a cleaner API.
   
   Also, while I was here, I cleaned up some more of the code to be more concise
   and removed behavior from starting a provider that tried to automatically
   auction on a host. None of the other functions attempted to do that, nor
   should they IMO. That behavior is nice to have in something like wash, but
   an API client represents lower level API interactions. So I removed that
   from the `start_provider` function.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 5 commits contributed to the release over the course of 9 calendar days.
 - 13 days passed between releases.
 - 4 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Un-hide policy options ([`8805873`](https://github.com/wasmCloud/wasmCloud/commit/8805873d6f556e1581b47846836aa249efd41caa))
    - Configure signing keys directory for build cmd ([`ef3e4e5`](https://github.com/wasmCloud/wasmCloud/commit/ef3e4e584fef4d597cab0215fdf3cfe864f701e9))
    - `Err(anyhow!(...))` -> `bail!`, err msg capitals ([`5af1c68`](https://github.com/wasmCloud/wasmCloud/commit/5af1c68bf86b62b4e2f81cbf1cc9ca1d5542ac37))
    - Merge pull request #64 from thomastaylor312/feat/unified_api ([`e2f3142`](https://github.com/wasmCloud/wasmCloud/commit/e2f3142da686ddae0ca28a1b6533b319e3544488))
    - Unifies all calls behind the ctl interface topics ([`e394b27`](https://github.com/wasmCloud/wasmCloud/commit/e394b271c13d33f24e9c6a302d17d91b3100e903))
</details>

## v0.79.1 (2023-10-21)

<csr-id-2b0c1a82f4354dabed8b1dc538162e486734279d/>
<csr-id-8a6780ab5b348e812470996ac5d98910b90b4910/>
<csr-id-70b20a12553e84697ffe9f8dbf32219162bdf946/>
<csr-id-372e81e2da3a60ee8cbf3f2525bf27284dc62332/>
<csr-id-571a25ddb7d8f18b2bb1d3f6b22401503d31f719/>
<csr-id-d53bf1b5e3be1cd8d076939cc80460305e30d8c5/>
<csr-id-13cb1907d4f70235059899fb329287d1b44736e5/>

### Chore

 - <csr-id-2b0c1a82f4354dabed8b1dc538162e486734279d/> bump wasmcloud 0.79.0 wadm 0.7.1
 - <csr-id-8a6780ab5b348e812470996ac5d98910b90b4910/> add extra space to fix alignment
 - <csr-id-70b20a12553e84697ffe9f8dbf32219162bdf946/> update async_nats,ctl,wasmbus_rpc to latest

### New Features

 - <csr-id-5c0ccc5f872ad42b6152c66c34ab73f855f82832/> query all host inventories
 - <csr-id-c418ccd7e4163e7d9695c87adf6f516a68334af0/> add friendly name to host table output

### Refactor

 - <csr-id-372e81e2da3a60ee8cbf3f2525bf27284dc62332/> various fixes to testing code
   This commit refactors some of the testing code to:
   
   - ensure we always print integration test output (save time root
   causing in CI and elsewhere)
   - consistent use of TARGET to choose which test to run
   - use system provided randomized ports (port 0)
   - fix some uses of context
   - remove some process scanning that was never used
   
   This commit also includes changes test flake fixes from
   https://github.com/wasmCloud/wash/pull/921
 - <csr-id-571a25ddb7d8f18b2bb1d3f6b22401503d31f719/> add manifest source type to use with app manifest loader.

### Chore (BREAKING)

 - <csr-id-d53bf1b5e3be1cd8d076939cc80460305e30d8c5/> remove prov_rpc options
 - <csr-id-13cb1907d4f70235059899fb329287d1b44736e5/> remove prov_rpc_host

### New Features (BREAKING)

 - <csr-id-7851a53ab31273b04df8372662198ac6dc70f78e/> add scale and update cmds
 - <csr-id-bb69ea644d95517bfdc38779c2060096f1cec30f/> update to start/stop/scale for concurrent instances
 - <csr-id-fcd166dcdff05ae78a5f8e4ecd516a57e1fd15ea/> remove manifest apply command

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 16 commits contributed to the release over the course of 7 calendar days.
 - 8 days passed between releases.
 - 12 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Remove prov_rpc options ([`d53bf1b`](https://github.com/wasmCloud/wasmCloud/commit/d53bf1b5e3be1cd8d076939cc80460305e30d8c5))
    - Remove prov_rpc_host ([`13cb190`](https://github.com/wasmCloud/wasmCloud/commit/13cb1907d4f70235059899fb329287d1b44736e5))
    - Merge pull request #922 from vados-cosmonic/refactor/light-testing-code-refactor ([`0b9e1ca`](https://github.com/wasmCloud/wasmCloud/commit/0b9e1caf8143fd7688f7658db76f01b6bd4a6c5f))
    - Various fixes to testing code ([`372e81e`](https://github.com/wasmCloud/wasmCloud/commit/372e81e2da3a60ee8cbf3f2525bf27284dc62332))
    - Merge pull request #874 from connorsmith256/feat/add-friendly-name ([`208e4d1`](https://github.com/wasmCloud/wasmCloud/commit/208e4d10354b76de87b2edf0cbe0329a439a7cac))
    - Bump wasmcloud 0.79.0 wadm 0.7.1 ([`2b0c1a8`](https://github.com/wasmCloud/wasmCloud/commit/2b0c1a82f4354dabed8b1dc538162e486734279d))
    - Merge pull request #873 from connorsmith256/feat/get-all-inventories ([`3b58fc7`](https://github.com/wasmCloud/wasmCloud/commit/3b58fc739b5ee6a8609e3d2501abfbdf604fe897))
    - Query all host inventories ([`5c0ccc5`](https://github.com/wasmCloud/wasmCloud/commit/5c0ccc5f872ad42b6152c66c34ab73f855f82832))
    - Add friendly name to host table output ([`c418ccd`](https://github.com/wasmCloud/wasmCloud/commit/c418ccd7e4163e7d9695c87adf6f516a68334af0))
    - Merge pull request #875 from ahmedtadde/feat/expand-manifest-input-sources-clean ([`c25352b`](https://github.com/wasmCloud/wasmCloud/commit/c25352bb21e7ec0f733317f2e13d3e183149e679))
    - Add manifest source type to use with app manifest loader. ([`571a25d`](https://github.com/wasmCloud/wasmCloud/commit/571a25ddb7d8f18b2bb1d3f6b22401503d31f719))
    - Add extra space to fix alignment ([`8a6780a`](https://github.com/wasmCloud/wasmCloud/commit/8a6780ab5b348e812470996ac5d98910b90b4910))
    - Add scale and update cmds ([`7851a53`](https://github.com/wasmCloud/wasmCloud/commit/7851a53ab31273b04df8372662198ac6dc70f78e))
    - Update to start/stop/scale for concurrent instances ([`bb69ea6`](https://github.com/wasmCloud/wasmCloud/commit/bb69ea644d95517bfdc38779c2060096f1cec30f))
    - Remove manifest apply command ([`fcd166d`](https://github.com/wasmCloud/wasmCloud/commit/fcd166dcdff05ae78a5f8e4ecd516a57e1fd15ea))
    - Update async_nats,ctl,wasmbus_rpc to latest ([`70b20a1`](https://github.com/wasmCloud/wasmCloud/commit/70b20a12553e84697ffe9f8dbf32219162bdf946))
</details>

## v0.79.0 (2023-10-10)

### New Features

 - <csr-id-32ea9f9eb8ba63118dfd23084d413aae23226124/> polishing app manifest loader
 - <csr-id-6907c8012fd59bbcaa6234c533b62ba997b86139/> http & stdin manifest input sources support for put & deploy cmds

### Bug Fixes

 - <csr-id-0eb5a7cade13a87e59c27c7f6faa89234d07863d/> some cleanup relevant to app manifest input sources
 - <csr-id-588c41338126978e2a7a2689e2b660b089799a28/> add friendly_name to Host
 - <csr-id-1bf2a2042cd3ef180ca465cb5dd3faf3db647d10/> update imports after package update and move to root

### Bug Fixes (BREAKING)

 - <csr-id-bf796201bc686c757ece81964cb3af877a6b344f/> make max optional, change meaning of 0

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 9 commits contributed to the release over the course of 4 calendar days.
 - 5 days passed between releases.
 - 6 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #876 from WallysFerreira/feat/866-add-alias-to-delete ([`119fc5b`](https://github.com/wasmCloud/wasmCloud/commit/119fc5bdc1d06bc3defe4b34e765240b4e98ef76))
    - Change name and add alias ([`41cac7d`](https://github.com/wasmCloud/wasmCloud/commit/41cac7d0ad8b3a4993b60f818c21a51dbffff0cb))
    - Some cleanup relevant to app manifest input sources ([`0eb5a7c`](https://github.com/wasmCloud/wasmCloud/commit/0eb5a7cade13a87e59c27c7f6faa89234d07863d))
    - Polishing app manifest loader ([`32ea9f9`](https://github.com/wasmCloud/wasmCloud/commit/32ea9f9eb8ba63118dfd23084d413aae23226124))
    - Http & stdin manifest input sources support for put & deploy cmds ([`6907c80`](https://github.com/wasmCloud/wasmCloud/commit/6907c8012fd59bbcaa6234c533b62ba997b86139))
    - Merge pull request #59 from connorsmith256/fix/add-friendly-name ([`0b81489`](https://github.com/wasmCloud/wasmCloud/commit/0b81489f154250193b10aa772e0e51925534fc87))
    - Add friendly_name to Host ([`588c413`](https://github.com/wasmCloud/wasmCloud/commit/588c41338126978e2a7a2689e2b660b089799a28))
    - Make max optional, change meaning of 0 ([`bf79620`](https://github.com/wasmCloud/wasmCloud/commit/bf796201bc686c757ece81964cb3af877a6b344f))
    - Update imports after package update and move to root ([`1bf2a20`](https://github.com/wasmCloud/wasmCloud/commit/1bf2a2042cd3ef180ca465cb5dd3faf3db647d10))
</details>

## v0.79.0-rc4 (2023-10-13)

### Bug Fixes

 - <csr-id-50950e44d693c487ca25827a5e2d84de18e41e6b/> Fixes issue with cached links not populating
   The initial implementation of the cached KV client had issues due
   to the drop implementation for the caching thread. It was only behind
   and arc, so any time a clone was dropped, the thread would stop polling.
   
   This also adds some additional logging around the caching and fixes the
   client builder based off of real life usage in wadm

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 3 commits contributed to the release over the course of 1 calendar day.
 - 2 days passed between releases.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #60 from thomastaylor312/fix/y_no_linkz ([`62fb192`](https://github.com/wasmCloud/wasmCloud/commit/62fb192cde9bd27bc99e55abaa14b5b11dfd3280))
    - Fixes issue with cached links not populating ([`50950e4`](https://github.com/wasmCloud/wasmCloud/commit/50950e44d693c487ca25827a5e2d84de18e41e6b))
    - Merge pull request #867 from lachieh/fix-wash-build-after-upgrade ([`dcfbaf3`](https://github.com/wasmCloud/wasmCloud/commit/dcfbaf37cbcff146a7471828abe9c4baf7c06a93))
</details>

## v0.79.0-rc3 (2023-10-05)

<csr-id-f057b40eb847467d5874e2d14b90fa6b2687f53c/>

### Chore

 - <csr-id-f057b40eb847467d5874e2d14b90fa6b2687f53c/> suppress deprecated messages within this library

### New Features

 - <csr-id-3c8aee31c83b8087451a9bc6557320d8493f3743/> add image reference and max to actor instance

### Bug Fixes

 - <csr-id-072c6a0aca9eab02616721c6c8bc7342a3d5440d/> deprecate rpc_timeout in favor of timeout

### New Features (BREAKING)

 - <csr-id-5155f5e1a7da11400bcc72f6f3398de7acc123ab/> remove start actor in favor of scale

### Bug Fixes (BREAKING)

 - <csr-id-d3b89a59f51fa9dec3496f73bdf0cc5b92e83c0e/> add constraints to provider auction ack

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 6 commits contributed to the release.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #56 from wasmCloud/feat/scale-actor-changes ([`bf37726`](https://github.com/wasmCloud/wasmCloud/commit/bf377264914b38f14afd13690151526c61b4b6bb))
    - Add constraints to provider auction ack ([`d3b89a5`](https://github.com/wasmCloud/wasmCloud/commit/d3b89a59f51fa9dec3496f73bdf0cc5b92e83c0e))
    - Deprecate rpc_timeout in favor of timeout ([`072c6a0`](https://github.com/wasmCloud/wasmCloud/commit/072c6a0aca9eab02616721c6c8bc7342a3d5440d))
    - Add image reference and max to actor instance ([`3c8aee3`](https://github.com/wasmCloud/wasmCloud/commit/3c8aee31c83b8087451a9bc6557320d8493f3743))
    - Suppress deprecated messages within this library ([`f057b40`](https://github.com/wasmCloud/wasmCloud/commit/f057b40eb847467d5874e2d14b90fa6b2687f53c))
    - Remove start actor in favor of scale ([`5155f5e`](https://github.com/wasmCloud/wasmCloud/commit/5155f5e1a7da11400bcc72f6f3398de7acc123ab))
</details>

## v0.79.0-rc2 (2023-10-04)

<csr-id-016c37812b8cf95615a6ad34ee49de669c66886b/>

### Chore

 - <csr-id-016c37812b8cf95615a6ad34ee49de669c66886b/> fix lint

### New Features

 - <csr-id-977feaa1bca1ae4df625c8061f2f5330029739b4/> parse labels from args

### New Features (BREAKING)

 - <csr-id-90f79447bc0b1dc7efbef2b13af9cf715e1ea1f0/> add par command support to wash-lib
   * Added par support to wash-lib

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 6 commits contributed to the release over the course of 5 calendar days.
 - 6 days passed between releases.
 - 3 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #849 from vados-cosmonic/chore/fix-lint ([`894329f`](https://github.com/wasmCloud/wasmCloud/commit/894329fca42ff4e58dbdffe9a39bc90147c63727))
    - Fix lint ([`016c378`](https://github.com/wasmCloud/wasmCloud/commit/016c37812b8cf95615a6ad34ee49de669c66886b))
    - Parse labels from args ([`977feaa`](https://github.com/wasmCloud/wasmCloud/commit/977feaa1bca1ae4df625c8061f2f5330029739b4))
    - Add par command support to wash-lib ([`90f7944`](https://github.com/wasmCloud/wasmCloud/commit/90f79447bc0b1dc7efbef2b13af9cf715e1ea1f0))
    - Merge pull request #839 from aish-where-ya/fix/update-actor ([`6d98a6d`](https://github.com/wasmCloud/wasmCloud/commit/6d98a6d2608333661254c184d6aba8e6b81fd145))
    - Minor fix to update actor in wash-lib ([`3dbbc03`](https://github.com/wasmCloud/wasmCloud/commit/3dbbc03c22e983a0b89a681a4645ad04a0a4b7d2))
</details>

## v0.79.0-rc1 (2023-09-28)

<csr-id-7f87247071ddf99ea2912504b2701e6eea0e9ad3/>

### New Features

 - <csr-id-99262d8b1c0bdb09657407663e2d5d4a3fb7651c/> move update-actor for wash ctl update to wash-lib.

### Bug Fixes

<csr-id-6dd214c2ea3befb5170d5a711a2eef0f3d14cc09/>

 - <csr-id-2314f5f4d49c5b98949fe5d4a1eb692f1fad92b7/> rework host shutdown
   - Always include a timeout for graceful shutdown (e.g. if NATS
   connection dies, it will never finish)
- Stop if one of the core wasmbus tasks dies
- Flush NATS queues concurrently on shutdown
- Handle `stopped` method errors

### Other

 - <csr-id-7f87247071ddf99ea2912504b2701e6eea0e9ad3/> use data-encoding for base64 encode/decoding
   * Use `data-encoding` for base64 encode/decoding
   
   The `data-encoding` crate is already in use and provides base64.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release over the course of 1 calendar day.
 - 6 days passed between releases.
 - 4 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: [#43](https://github.com/wasmCloud/wasmCloud/issues/43)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#43](https://github.com/wasmCloud/wasmCloud/issues/43)**
    - Use data-encoding for base64 encode/decoding ([`7f87247`](https://github.com/wasmCloud/wasmCloud/commit/7f87247071ddf99ea2912504b2701e6eea0e9ad3))
 * **Uncategorized**
    - Rework host shutdown ([`2314f5f`](https://github.com/wasmCloud/wasmCloud/commit/2314f5f4d49c5b98949fe5d4a1eb692f1fad92b7))
    - Validate jwt has 3 segments ([`6dd214c`](https://github.com/wasmCloud/wasmCloud/commit/6dd214c2ea3befb5170d5a711a2eef0f3d14cc09))
    - Move update-actor for wash ctl update to wash-lib. ([`99262d8`](https://github.com/wasmCloud/wasmCloud/commit/99262d8b1c0bdb09657407663e2d5d4a3fb7651c))
</details>

<csr-unknown>
 validate jwt has 3 segments<csr-unknown/>

## v0.78.0-rc9 (2023-09-21)

## v0.78.0-rc8 (2023-09-20)

## v0.78.0-rc7 (2023-09-19)

### Bug Fixes

 - <csr-id-263823e7469cd1b22c241903e5e565f7daae4cbe/> correct error message for missing file deploy
 - <csr-id-c19d88762c2a50c2e4afd12e85a6b86b775b0801/> fix description on app deploy
 - <csr-id-70785b53e1c45817a00d69cd3831871cdd2b40fe/> display deployed version in app list

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 6 commits contributed to the release over the course of 11 calendar days.
 - 11 days passed between releases.
 - 3 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #55 from thomastaylor312/feat/cache ([`8ecb129`](https://github.com/wasmCloud/wasmCloud/commit/8ecb1295ac2f82a06c270c5301770d57401d17ec))
    - Feat!(kv): Adds caching option for the interface client ([`b72dcb4`](https://github.com/wasmCloud/wasmCloud/commit/b72dcb4522c8ebf4850050d6de7587bfde5c9450))
    - Correct error message for missing file deploy ([`263823e`](https://github.com/wasmCloud/wasmCloud/commit/263823e7469cd1b22c241903e5e565f7daae4cbe))
    - Fix description on app deploy ([`c19d887`](https://github.com/wasmCloud/wasmCloud/commit/c19d88762c2a50c2e4afd12e85a6b86b775b0801))
    - Display deployed version in app list ([`70785b5`](https://github.com/wasmCloud/wasmCloud/commit/70785b53e1c45817a00d69cd3831871cdd2b40fe))
    - Merge pull request #782 from wasmCloud/chore/wasmcloud-0.78.0-rc.6-bump ([`cfab765`](https://github.com/wasmCloud/wasmCloud/commit/cfab7655770ff414d57c6531db56e587465ee738))
</details>

## v0.78.0-rc6 (2023-09-07)

<csr-id-892d6dd777ed6fe2998afcd37fe7add8b751b012/>

### Chore

 - <csr-id-892d6dd777ed6fe2998afcd37fe7add8b751b012/> run wasmcloud 0.78.0-rc6

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Run wasmcloud 0.78.0-rc6 ([`892d6dd`](https://github.com/wasmCloud/wasmCloud/commit/892d6dd777ed6fe2998afcd37fe7add8b751b012))
</details>

## v0.78.0-rc5 (2023-09-07)

<csr-id-5befc1bf86d022bb6ecb2885ac746d777d49dc94/>
<csr-id-eab7a7b18a626bd790c42ab23a5ce571baf0c56b/>
<csr-id-bbf0b1a6074108a96d9534500c97c8ad5ed13dd6/>

### Chore

 - <csr-id-5befc1bf86d022bb6ecb2885ac746d777d49dc94/> update dashboard message
 - <csr-id-eab7a7b18a626bd790c42ab23a5ce571baf0c56b/> bump wadm 0.6.0
 - <csr-id-bbf0b1a6074108a96d9534500c97c8ad5ed13dd6/> remove references to DASHBOARD_PORT

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 3 commits contributed to the release over the course of 1 calendar day.
 - 1 day passed between releases.
 - 3 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Update dashboard message ([`5befc1b`](https://github.com/wasmCloud/wasmCloud/commit/5befc1bf86d022bb6ecb2885ac746d777d49dc94))
    - Bump wadm 0.6.0 ([`eab7a7b`](https://github.com/wasmCloud/wasmCloud/commit/eab7a7b18a626bd790c42ab23a5ce571baf0c56b))
    - Remove references to DASHBOARD_PORT ([`bbf0b1a`](https://github.com/wasmCloud/wasmCloud/commit/bbf0b1a6074108a96d9534500c97c8ad5ed13dd6))
</details>

## v0.78.0-rc4 (2023-09-05)

## v0.78.0-rc3 (2023-09-03)

<csr-id-f4a9cd6d2f1c29b0cc7eb4c3509114ed81eb7983/>

### New Features

 - <csr-id-78b99fde8606febf59e30f1d12ac558b29d425bf/> set default to Rust host
   - update paths to release binary
- allow-file-upload default bug
- mention dashboard ui cmd

### Other

 - <csr-id-f4a9cd6d2f1c29b0cc7eb4c3509114ed81eb7983/> use rc2

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release over the course of 1 calendar day.
 - 1 day passed between releases.
 - 2 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #759 from wasmCloud/rust-host-default ([`6be0162`](https://github.com/wasmCloud/wasmCloud/commit/6be0162cb89a6d030270d616bc4667c2c5cc7186))
    - Use rc2 ([`f4a9cd6`](https://github.com/wasmCloud/wasmCloud/commit/f4a9cd6d2f1c29b0cc7eb4c3509114ed81eb7983))
    - Removes unused param and unneeded wash cycle ([`776afd3`](https://github.com/wasmCloud/wasmCloud/commit/776afd3e40270fd294ad8071012adc5251d89cd5))
    - Set default to Rust host ([`78b99fd`](https://github.com/wasmCloud/wasmCloud/commit/78b99fde8606febf59e30f1d12ac558b29d425bf))
</details>

## v0.78.0-rc2 (2023-09-02)

<csr-id-e15878948673f9ad1cfbbafdc01c48c2d2678955/>
<csr-id-e459a7a2434ce926d211ff37c1f6ebef2b5faef5/>
<csr-id-c2f765d5c25a18e5f79379955cd77ed4858954bd/>
<csr-id-c9fef73977b86172afc5d3f2e8c4830c0277aff3/>
<csr-id-abb2de11f159191357f1676b11fe07bd39c5573c/>
<csr-id-710ab08386bd57da080c8a207ed18ea7f9ed217f/>
<csr-id-c8a2d0b99bed8c92cd51d95c8a62addb67f2bb1d/>
<csr-id-9d94dccea42c486c95e9fa497c1d1e7cf7cd5a0b/>
<csr-id-22d374a1d750c0803a52bd93bb057018576e804d/>
<csr-id-c51fa5e51115ccf001d916b3882e819d4ec7cea8/>
<csr-id-315c808777c4800dbbd52efacf7bf36b2b245f5a/>
<csr-id-bc35010697e1eca3eb2093ee6aa5302a9bd1d437/>
<csr-id-ed4282c9ea1bb95e346c9a981acdc264b0fc9d3f/>
<csr-id-77ed1441bdd1da15e13ce9196138cfe7c037f6ba/>
<csr-id-82915861e422c845d97b3a8680738d55bd9bfce2/>
<csr-id-088cbe0a20c7486bfaa80ec0d69e18ab2a2a6902/>
<csr-id-48eafc861099b08a531b7eeb033802ab8a215baf/>
<csr-id-0dca3ef4bb3db38aae6dbea57b520a36ef058e2f/>
<csr-id-52ef5b6b1b6b01bc5e7a2c8fe3cbb2a08d4ad864/>
<csr-id-7d0e031a57564cd550e7e2db48f939403ad22cd8/>
<csr-id-1f2ad935620548acc4f2a51a4956056fa99e2e93/>
<csr-id-0bbdd4032b3bc1b63df6724b0d636176c2d49226/>
<csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-0db5a5ba5b20535e16af46fd92f7040c8174d636/>
<csr-id-8806ef1ff0afeb42986639d254e8d9cb918e1633/>
<csr-id-fa064101e82385c2fb9c9cd0ce958d800a65d660/>
<csr-id-010129a272ce327cbb251b874f6f4cf57a950f91/>
<csr-id-a4902e25212f7261ed444f7483a07aa210283a16/>
<csr-id-909e04f06139de52304babfeef6839e172aac5c2/>
<csr-id-f3f6c21f25632940a6cc1d5290f8e84271609496/>
<csr-id-c6acc2c6f6183515441d1dcaca073ba1df109df2/>
<csr-id-8cff2e5b65fbb8b5e0578d1ce5ccb892e14caba7/>

### Chore

 - <csr-id-e15878948673f9ad1cfbbafdc01c48c2d2678955/> remove irrelevant ipv6 flag
 - <csr-id-e459a7a2434ce926d211ff37c1f6ebef2b5faef5/> refactor args parsing
 - <csr-id-c2f765d5c25a18e5f79379955cd77ed4858954bd/> unhide implemented CLI args
 - <csr-id-c9fef73977b86172afc5d3f2e8c4830c0277aff3/> ignore .DS_Store
 - <csr-id-abb2de11f159191357f1676b11fe07bd39c5573c/> scaffold cli args
 - <csr-id-710ab08386bd57da080c8a207ed18ea7f9ed217f/> bump to 0.28, async-nats 0.31
 - <csr-id-c8a2d0b99bed8c92cd51d95c8a62addb67f2bb1d/> delete useless src/start.rs
 - <csr-id-9d94dccea42c486c95e9fa497c1d1e7cf7cd5a0b/> Fixes a bunch of clippy lints
   Pretty sure the new lints were from the latest version of clippy in
   Rust 1.65. This fixes all of them
 - <csr-id-22d374a1d750c0803a52bd93bb057018576e804d/> update clap to v4
 - <csr-id-c51fa5e51115ccf001d916b3882e819d4ec7cea8/> adopt common naming convention with OTP host
 - <csr-id-315c808777c4800dbbd52efacf7bf36b2b245f5a/> Bumps version to 0.6.6
   Also, I couldn't help myself and fixed the remaining clippy warnings

### Documentation

 - <csr-id-b7b43385ef52c0026b65d6eefe85d7bd12d15682/> add `wash get` to help text
 - <csr-id-b7016b648d5f7f1d3605e6dff933d1e58c8a797c/> add top level commands to help text, restructure

### New Features

<csr-id-e9fe020a0906cb377f6ea8bd3a9879e5bad877b7/>
<csr-id-8c96789f1c793c5565715080b84fecfbe0653b43/>
<csr-id-e58c6a60928a7157ffbbc95f9eabcc9cae3db2a7/>
<csr-id-d2bc21681306ef2251be4347224249e2ce8c4c18/>
<csr-id-6923ce7efb721f8678c33f42647b87ea33a7653a/>
<csr-id-4daf51be422d395bc0142d62b8d59060b89feafa/>
<csr-id-128f7603c67443f23e76c3cb4bd1468ffd8f5462/>
<csr-id-2a6c401834b4cb55ef420538e15503b98281eaf1/>
<csr-id-24bba484009be9e87bfcbd926a731534e936c339/>
<csr-id-caa965ac17eeda67c35f41b38a236f1b682cf462/>
<csr-id-7cca2e76f0048bd37a50960c8df5b40ed0e16d7d/>
<csr-id-79e66a64a8d20926a18967e8efb970d2104e6596/>
<csr-id-d8900ccc62f1383ed231bee1b6a28fd434f74c5a/>
<csr-id-bb89f2c516339e155a6c942871907a2c044ee014/>
<csr-id-4db7517586aec531137e7f83836da3fcd684d18e/>
<csr-id-fc65c2cb27ad15e0ef27fa45e61a3d62c2d0c033/>
<csr-id-fc6620a5ba92b1e6fce4e16c21cc4b6cb5ccae0d/>
<csr-id-3d19c94128bcb6643f6e939f930a503ab9b9ca94/>
<csr-id-4d7b83df95ef8d039b9ceac96c34b9773744aa9d/>
<csr-id-a645105802b22a719c8c5ae9232c6ea27170a019/>
<csr-id-84b95392993cbbc65da36bc8b872241cce32a63e/>
<csr-id-a62b07b8ff321c400c6debefdb6199e273445490/>
<csr-id-b1bf6b1ac7851dc09e6757d7c2bde4558ec48098/>
<csr-id-189639f98aa5b9669da8143010c09600e2be449d/>
<csr-id-4e16308d4f12fbac49d3de8340495c5e29266009/>
<csr-id-95b325246e34bbe47d81799acfcd07cd6cc6b9ea/>
<csr-id-a867a07ca60f1391fbfea56d9ad246a71792652f/>

 - <csr-id-2ebdab7551f6da93967d921316cae5d04a409a43/> support policy service
 - <csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/> support OTEL traces end-to-end
 - <csr-id-48d4557c8ee895278055261bccb1293806b308b0/> support registry settings via config service and command-line flags
 - <csr-id-3588b5f9ce2f0c0a4718d9bd576904ef77682304/> implement ctl_topic_prefix
 - <csr-id-c9fecb99793649a6f9321b9224f85b9472889dec/> add support for custom lattice prefix
 - <csr-id-4144f711ad2056e9334e085cbe08663065605b0c/> build wasi preview components from wash
 - <csr-id-ec5675d11768ed9741a8d3e7c42cc1e5a823d41d/> implement host stop
 - <csr-id-ef20466a04d475159088b127b46111b80a5e1eb2/> introduce wasmbus lattice
 - <csr-id-7364dd8afae5c8884ca923b39c5680c60d8d0e3d/> implement data streaming
   - make claims optional (at least for now)
- add streaming support to `wasmcloud:bus`
- rename `wasmcloud_host` -> `wasmcloud_runtime`
- remove all `wasmcloud-interface-*` usages
- add support for `command` executables (I/O actors)
- add local lattice proving the concept, which is used for testing of the feature
- implement an actor instance pool
- simplifying the generic bounds
- providing a more extensible way of handling capabilities
- removing a need for custom result type within actor module
- making future addition of different Wasm engines easier
- executes actors via wasmtime
- implements the `wasmbus` FFI interface
- contains built-in provider implementation adaptors for `log` and `rand` crates
* add wait for start actor
* feat(ctl): warning if a contract ID looks like an nkey
* feat(*): fetch from wasmcloud cache on inspect commands
- includes new --no-cache option for inspect commands

### Bug Fixes

 - <csr-id-bd969b7e54d912109b36f61f2907d4d39a63ca3a/> do not set a default value for traces exporter
 - <csr-id-fe338a7d9820f055e2f8c6826aeb4c53ddb1fd71/> off-by-one range check
 - <csr-id-75a1fb075357ac2566fef1b45c930e6c400a4041/> store typed keys, not strings
 - <csr-id-ed24d3ecda0c28141114933d9af2a1cd44d998c8/> parse lists with a delimiter
 - <csr-id-ad82a331d4f72c2dcc15f20559eca4aab0575bdc/> remove unused `pub`
 - <csr-id-2abd61a5d5a74847417be603412804487c8489c4/> do not panic on errors
 - <csr-id-0ec90cd5fd2c7018dd9614e9777f508b9969e7b1/> safely parse null versions as empty string
 - <csr-id-3c32ae32b46d36fbb0a38d0a087a291d4e228a11/> fix stop nats after starting wasmcloud host failed
 - <csr-id-ae01c022f5793b76b0c37700890514be64734d9e/> the wadm pidfile is not removed when wadm is not started
 - <csr-id-7d4e6d46d7bb7a53a4860c499eb6dbab8a1b0a4c/> fix stop and cleanup wadm
 - <csr-id-4900f82caf39913e076c1664702d9e9d02836135/> Allows multiple hosts to run without sharing data
   I found out when running some blobby tests that if you spin up
   multiple hosts, the NATS servers are separate, but they actually
   use the same data directory by default for jetstream. This means
   that two different locally running hosts _technically_ have the
   same streams and data available, which could lead to conflicts.
   
   This segments it off into different data directories depending on
   the port the nats server is listening on. Technically there are
   still bugs when running two different nats servers as they write to
   the same log file, but we can solve that one later
 - <csr-id-d7a4423d6ddf637830e0f3cdb57f77ad46a90131/> call `_start` for tinygo actor modules
 - <csr-id-f94b602044ac33df53f1ff76a160549821134e93/> format error messages with variable substitution
 - <csr-id-09e61b5c9b67fe4dd583872fc0f35fd0295fbbd4/> Makes sure we can actually shutdown a detached process
   The down command was creating its own path to the binary which didn't
   take into account the new versioned paths
 - <csr-id-2e69e12d4b78f5ea7710ba12226345440e7541ef/> Makes sure that wash downloads different versions of wasmcloud
   This now downloads different versions to different directories. Also did
   a little bit of cleanup with some clippy warnings in the tests and bumping
   NATS to a later version
 - <csr-id-0e95f67f46dcb6d651c27a31fc310fe09a70f374/> minor addition to test
   While investigating diffs in wasmCloud/wash#361
   I wanted to see assertions for a roundtrip of call_alias
   and the human friendly capability name.
 - <csr-id-c805cbc032f0d828dfe666657bfeb553c74a0f33/> change test reg port
   Macs are quirky and it's best to avoid ports 5000 and 7000
   https://developer.apple.com/forums/thread/682332
 - <csr-id-2520b7c68e5b1d33569b867f2f2431f8022108c2/> add a border to the ascii art to prevent the leading whitespace from being stripped
 - <csr-id-5c2388e7e068ec4d5ffbd0d33cdaab554864fea2/> Fix home directory lookup for keys
   We were looking for the `HOME` variable, which isn't set on Windows.
   This switches things to use the `dirs` crate and switches from `String`
   to `PathBuf`. Also did a few clippy cleanups in the files I was working
   on
 - <csr-id-b15b5b9be9fac5f70635ebe137c789ddbf84ac8f/> Fixes windows path handling
   When creating a new project, a subdirectory would be pushed with its
   default `/` instead of the operating system specific slash. This fixes
   the issue and I did double check that it should work on *nix systems
   too
 - <csr-id-4fc074d62ddba070adcfbace293d33a8c56d50a1/> Removes incorrect information from help text

### Other

 - <csr-id-bc35010697e1eca3eb2093ee6aa5302a9bd1d437/> ensure that the wasmcloud-host can be stopped gracefully
 - <csr-id-ed4282c9ea1bb95e346c9a981acdc264b0fc9d3f/> update WIT dependencies
 - <csr-id-77ed1441bdd1da15e13ce9196138cfe7c037f6ba/> only use `bindeps` for WASI adapter build
   Most importantly, this ensures that downstream consumers of this library
   do not need to rely on nightly and/or enable the unstable `bindeps`
   feature.
   
   This also fixes `cargo tree`, which should finally allow Dependabot to
   manage the dependencies in this repo
 - <csr-id-82915861e422c845d97b3a8680738d55bd9bfce2/> fix broken references
 - <csr-id-088cbe0a20c7486bfaa80ec0d69e18ab2a2a6902/> remove redundant TODO
 - <csr-id-48eafc861099b08a531b7eeb033802ab8a215baf/> support the new abi for embedding and extracting claims
   * Experiment: support the new abi for embedding and extracting claims
 - <csr-id-0dca3ef4bb3db38aae6dbea57b520a36ef058e2f/> shell completions
 - <csr-id-52ef5b6b1b6b01bc5e7a2c8fe3cbb2a08d4ad864/> Creates new context library
   This creates a new context library with some extendable traits for
   loading as well as a fully featured module for handling context on
   disk.
   
   Additional tests will be in the next commit
 - <csr-id-7d0e031a57564cd550e7e2db48f939403ad22cd8/> add `cfg` module to get or create the `.wash` directory
   * add cfg_dir function to get/create .wash directory
 - <csr-id-1f2ad935620548acc4f2a51a4956056fa99e2e93/> Return better errors when provider id is given
   It is relatively common for first time users to pass the provider id rather
   than the contract ID when doing `wash ctl link del`. This adds a block
   that tests if it is a provider ID and returns a hint to the user. I also
   noticed we weren't returning an error code on link commands if they failed,
   so I added some logic to fix it
 - <csr-id-0bbdd4032b3bc1b63df6724b0d636176c2d49226/> add reg ping to test if oci url is valid

### Refactor

 - <csr-id-e1d7356bb0a07af9f4e6b1626f5df33709f3ed78/> replace lazy_static with once_cell
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers
 - <csr-id-0db5a5ba5b20535e16af46fd92f7040c8174d636/> establish NATS connections concurrently
 - <csr-id-8806ef1ff0afeb42986639d254e8d9cb918e1633/> introduce `test-actors` subcrate
 - <csr-id-fa064101e82385c2fb9c9cd0ce958d800a65d660/> rework handler API
 - <csr-id-010129a272ce327cbb251b874f6f4cf57a950f91/> rename `external` handler to `hostcall`
 - <csr-id-a4902e25212f7261ed444f7483a07aa210283a16/> use u32, rather than i32 wasm values
 - <csr-id-909e04f06139de52304babfeef6839e172aac5c2/> split `actor` module

### Reverted

 - <csr-id-b7f000ce54ddfab9581a246100384c9792be32b5/> "added call_context to component and module Ctx"
   This reverts commit e20846a55ccb2055159cc4b2a9ac942f91dd1f68.
   
   The functionality will be exposed via custom handlers on the actor
   instances, just like in the original implementation

### Style

 - <csr-id-f3f6c21f25632940a6cc1d5290f8e84271609496/> rename most instances of lattice and wasmbus to host

### Test

 - <csr-id-c6acc2c6f6183515441d1dcaca073ba1df109df2/> format errors correctly

### New Features (BREAKING)

 - <csr-id-a74b50297496578e5e6c0ee806304a3ff05cd073/> update wadm to 0.5.0
 - <csr-id-ed64180714873bd9be1f9008d29b09cbf276bba1/> implement structured logging
 - <csr-id-ff024913d3107dc65dd8aad69a1f598390de6d1a/> respect allow_file_load
 - <csr-id-dbcb84733099251ae600573ab4b48193324124b6/> add nats_jwt and nats_seed as fallbacks
 - <csr-id-921fa784ba3853b6b0a622c6850bb6d71437a011/> implement rpc,ctl,prov_rpc connections
 - <csr-id-7c389bee17d34db732babde7724286656c268f65/> use allow_latest and allowed_insecure config
 - <csr-id-9897b90e845470faa35e8caf4816c29e6dcefd91/> use js_domain provided by cli
 - <csr-id-7d290aa08b2196a6082972a4d662bf1a93d07dec/> implement graceful provider shutdown delay
 - <csr-id-1d63fd94b2bbb73992e0972eccfad1e617db846d/> use log level provided by cli
 - <csr-id-194f791c16ad6a7106393b4bcf0d0c51a70f638d/> maintain cluster issuers list
 - <csr-id-5b38e1007723032b847efd9402f2a16b22e80b54/> add missing fields for host inventory and auction ack
 - <csr-id-acdcd957bfedb5a86a0420c052da1e65d32e6c23/> allow get inventory to query the only host
 - <csr-id-673f8bc4e4bfebb49088dddd749c41acf42ae242/> async_nats 0.30, removed wasmbus_rpc dep
 - <csr-id-6a7dd430b64e5506996155bd6c64423e0065265e/> add support for `component-model`
   - Add `wasm32-wasi` to `rust-toolchain.toml`
- Update `wascap` to `0.10.0` for component model support
- Introduce ActorModule and ActorComponent distinction
- Make implementation async by default (requirement for WASI)

### Refactor (BREAKING)

 - <csr-id-8cff2e5b65fbb8b5e0578d1ce5ccb892e14caba7/> unexport low-level actor primitives

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 335 commits contributed to the release over the course of 812 calendar days.
 - 840 days passed between releases.
 - 107 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 91 unique issues were worked on: [#13](https://github.com/wasmCloud/wasmCloud/issues/13), [#14](https://github.com/wasmCloud/wasmCloud/issues/14), [#144](https://github.com/wasmCloud/wasmCloud/issues/144), [#145](https://github.com/wasmCloud/wasmCloud/issues/145), [#146](https://github.com/wasmCloud/wasmCloud/issues/146), [#150](https://github.com/wasmCloud/wasmCloud/issues/150), [#153](https://github.com/wasmCloud/wasmCloud/issues/153), [#16](https://github.com/wasmCloud/wasmCloud/issues/16), [#160](https://github.com/wasmCloud/wasmCloud/issues/160), [#162](https://github.com/wasmCloud/wasmCloud/issues/162), [#165](https://github.com/wasmCloud/wasmCloud/issues/165), [#171](https://github.com/wasmCloud/wasmCloud/issues/171), [#172](https://github.com/wasmCloud/wasmCloud/issues/172), [#173](https://github.com/wasmCloud/wasmCloud/issues/173), [#176](https://github.com/wasmCloud/wasmCloud/issues/176), [#177](https://github.com/wasmCloud/wasmCloud/issues/177), [#178](https://github.com/wasmCloud/wasmCloud/issues/178), [#196](https://github.com/wasmCloud/wasmCloud/issues/196), [#198](https://github.com/wasmCloud/wasmCloud/issues/198), [#199](https://github.com/wasmCloud/wasmCloud/issues/199), [#20](https://github.com/wasmCloud/wasmCloud/issues/20), [#202](https://github.com/wasmCloud/wasmCloud/issues/202), [#203](https://github.com/wasmCloud/wasmCloud/issues/203), [#204](https://github.com/wasmCloud/wasmCloud/issues/204), [#208](https://github.com/wasmCloud/wasmCloud/issues/208), [#211](https://github.com/wasmCloud/wasmCloud/issues/211), [#214](https://github.com/wasmCloud/wasmCloud/issues/214), [#216](https://github.com/wasmCloud/wasmCloud/issues/216), [#22](https://github.com/wasmCloud/wasmCloud/issues/22), [#220](https://github.com/wasmCloud/wasmCloud/issues/220), [#221](https://github.com/wasmCloud/wasmCloud/issues/221), [#223](https://github.com/wasmCloud/wasmCloud/issues/223), [#226](https://github.com/wasmCloud/wasmCloud/issues/226), [#229](https://github.com/wasmCloud/wasmCloud/issues/229), [#230](https://github.com/wasmCloud/wasmCloud/issues/230), [#236](https://github.com/wasmCloud/wasmCloud/issues/236), [#237](https://github.com/wasmCloud/wasmCloud/issues/237), [#240](https://github.com/wasmCloud/wasmCloud/issues/240), [#245](https://github.com/wasmCloud/wasmCloud/issues/245), [#246](https://github.com/wasmCloud/wasmCloud/issues/246), [#254](https://github.com/wasmCloud/wasmCloud/issues/254), [#255](https://github.com/wasmCloud/wasmCloud/issues/255), [#257](https://github.com/wasmCloud/wasmCloud/issues/257), [#269](https://github.com/wasmCloud/wasmCloud/issues/269), [#271](https://github.com/wasmCloud/wasmCloud/issues/271), [#272](https://github.com/wasmCloud/wasmCloud/issues/272), [#276](https://github.com/wasmCloud/wasmCloud/issues/276), [#278](https://github.com/wasmCloud/wasmCloud/issues/278), [#28](https://github.com/wasmCloud/wasmCloud/issues/28), [#280](https://github.com/wasmCloud/wasmCloud/issues/280), [#287](https://github.com/wasmCloud/wasmCloud/issues/287), [#288](https://github.com/wasmCloud/wasmCloud/issues/288), [#294](https://github.com/wasmCloud/wasmCloud/issues/294), [#297](https://github.com/wasmCloud/wasmCloud/issues/297), [#298](https://github.com/wasmCloud/wasmCloud/issues/298), [#300](https://github.com/wasmCloud/wasmCloud/issues/300), [#303](https://github.com/wasmCloud/wasmCloud/issues/303), [#308](https://github.com/wasmCloud/wasmCloud/issues/308), [#31](https://github.com/wasmCloud/wasmCloud/issues/31), [#310](https://github.com/wasmCloud/wasmCloud/issues/310), [#311](https://github.com/wasmCloud/wasmCloud/issues/311), [#319](https://github.com/wasmCloud/wasmCloud/issues/319), [#320](https://github.com/wasmCloud/wasmCloud/issues/320), [#324](https://github.com/wasmCloud/wasmCloud/issues/324), [#327](https://github.com/wasmCloud/wasmCloud/issues/327), [#33](https://github.com/wasmCloud/wasmCloud/issues/33), [#334](https://github.com/wasmCloud/wasmCloud/issues/334), [#338](https://github.com/wasmCloud/wasmCloud/issues/338), [#340](https://github.com/wasmCloud/wasmCloud/issues/340), [#35](https://github.com/wasmCloud/wasmCloud/issues/35), [#353](https://github.com/wasmCloud/wasmCloud/issues/353), [#354](https://github.com/wasmCloud/wasmCloud/issues/354), [#355](https://github.com/wasmCloud/wasmCloud/issues/355), [#36](https://github.com/wasmCloud/wasmCloud/issues/36), [#362](https://github.com/wasmCloud/wasmCloud/issues/362), [#376](https://github.com/wasmCloud/wasmCloud/issues/376), [#39](https://github.com/wasmCloud/wasmCloud/issues/39), [#393](https://github.com/wasmCloud/wasmCloud/issues/393), [#398](https://github.com/wasmCloud/wasmCloud/issues/398), [#399](https://github.com/wasmCloud/wasmCloud/issues/399), [#40](https://github.com/wasmCloud/wasmCloud/issues/40), [#41](https://github.com/wasmCloud/wasmCloud/issues/41), [#42](https://github.com/wasmCloud/wasmCloud/issues/42), [#44](https://github.com/wasmCloud/wasmCloud/issues/44), [#452](https://github.com/wasmCloud/wasmCloud/issues/452), [#46](https://github.com/wasmCloud/wasmCloud/issues/46), [#48](https://github.com/wasmCloud/wasmCloud/issues/48), [#520](https://github.com/wasmCloud/wasmCloud/issues/520), [#556](https://github.com/wasmCloud/wasmCloud/issues/556), [#677](https://github.com/wasmCloud/wasmCloud/issues/677), [#7](https://github.com/wasmCloud/wasmCloud/issues/7)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#13](https://github.com/wasmCloud/wasmCloud/issues/13)**
    - Moving interface types to shared dependency ([`93ca6de`](https://github.com/wasmCloud/wasmCloud/commit/93ca6de74d620ef772c4728d8ad460779fa3ff1d))
 * **[#14](https://github.com/wasmCloud/wasmCloud/issues/14)**
    - Adds stop host command ([`af27cb5`](https://github.com/wasmCloud/wasmCloud/commit/af27cb56b53491ad3f161619a0700056f8fa9ef8))
 * **[#144](https://github.com/wasmCloud/wasmCloud/issues/144)**
    - Restrict tables to available output width ([`3353157`](https://github.com/wasmCloud/wasmCloud/commit/3353157e155fcb29b1b96d3f2b6520177d0a76d6))
 * **[#145](https://github.com/wasmCloud/wasmCloud/issues/145)**
    - Truncate id and add name to inventory output ([`47c91ac`](https://github.com/wasmCloud/wasmCloud/commit/47c91ace6ad12b39cddf6ddd9b1a9f12917cec95))
 * **[#146](https://github.com/wasmCloud/wasmCloud/issues/146)**
    - Add credsfile argument to wash ctl subcommands ([`ec057ac`](https://github.com/wasmCloud/wasmCloud/commit/ec057ace22543ffa57b80b1f81c22980773ed558))
 * **[#150](https://github.com/wasmCloud/wasmCloud/issues/150)**
    - 0.5.0 release ([`4a06577`](https://github.com/wasmCloud/wasmCloud/commit/4a06577f0ccd83d0421903b1cb6ef301523f2834))
 * **[#153](https://github.com/wasmCloud/wasmCloud/issues/153)**
    - Otp changes (march toward 0.6.0) ([`63ec7b9`](https://github.com/wasmCloud/wasmCloud/commit/63ec7b91dbd6f6d43c6f17cd4672e4ebccab89f0))
 * **[#16](https://github.com/wasmCloud/wasmCloud/issues/16)**
    - Upgrading to latest versions of interfaces ([`a769c89`](https://github.com/wasmCloud/wasmCloud/commit/a769c89661fc723dfd84070d829dd30973f20e47))
 * **[#160](https://github.com/wasmCloud/wasmCloud/issues/160)**
    - Support querying, deleting link definitions ([`af5f610`](https://github.com/wasmCloud/wasmCloud/commit/af5f610b11f92d4370b9819c38a001f27426221f))
 * **[#162](https://github.com/wasmCloud/wasmCloud/issues/162)**
    - Update favorites template list to include factorial samples ([`48dbb24`](https://github.com/wasmCloud/wasmCloud/commit/48dbb24d4551fc0afa0942b1c8067adf4b8ddfc5))
 * **[#165](https://github.com/wasmCloud/wasmCloud/issues/165)**
    - Issue warning message for 'wash reg push' to localhost without --insecure ([`4058100`](https://github.com/wasmCloud/wasmCloud/commit/40581005353f43604b78cda66c5e3790a537db37))
 * **[#171](https://github.com/wasmCloud/wasmCloud/issues/171)**
    - Use longer timeout or start actor and provider ([`bd97e67`](https://github.com/wasmCloud/wasmCloud/commit/bd97e67724c58977b243d4ffb113e8d8b7ab2da7))
 * **[#172](https://github.com/wasmCloud/wasmCloud/issues/172)**
    - Update wasmbus rpc ([`45f228b`](https://github.com/wasmCloud/wasmCloud/commit/45f228bcf10cf601c5090c0ab14e91cc666c24b0))
 * **[#173](https://github.com/wasmCloud/wasmCloud/issues/173)**
    - Add reg ping to test if oci url is valid ([`0bbdd40`](https://github.com/wasmCloud/wasmCloud/commit/0bbdd4032b3bc1b63df6724b0d636176c2d49226))
 * **[#176](https://github.com/wasmCloud/wasmCloud/issues/176)**
    - Require cluster seed for rpc invocations ([`0b96efa`](https://github.com/wasmCloud/wasmCloud/commit/0b96efacc613c92afb021176da555f5b475701c9))
 * **[#177](https://github.com/wasmCloud/wasmCloud/issues/177)**
    - Mac fixes + remove debugging code ([`d9e2209`](https://github.com/wasmCloud/wasmCloud/commit/d9e2209d87a774ebc26e7c749ff22c99d0455c5b))
 * **[#178](https://github.com/wasmCloud/wasmCloud/issues/178)**
    - Use call --timeout param to set rpc timeout for actor ([`96b3028`](https://github.com/wasmCloud/wasmCloud/commit/96b3028905b22ed6298573a1634377ebd23a8d1a))
 * **[#196](https://github.com/wasmCloud/wasmCloud/issues/196)**
    - [FEATURE] wash Context ([`ebeedec`](https://github.com/wasmCloud/wasmCloud/commit/ebeedec78c1539fdc9a3612725f4ccf01718d1d7))
 * **[#198](https://github.com/wasmCloud/wasmCloud/issues/198)**
    - Adopt common naming convention with OTP host ([`c51fa5e`](https://github.com/wasmCloud/wasmCloud/commit/c51fa5e51115ccf001d916b3882e819d4ec7cea8))
 * **[#199](https://github.com/wasmCloud/wasmCloud/issues/199)**
    - Public key CLI args validation ([`a9f980c`](https://github.com/wasmCloud/wasmCloud/commit/a9f980c71a9f9e62ab53ab031ddf2a3a34889b53))
 * **[#20](https://github.com/wasmCloud/wasmCloud/issues/20)**
    - Start providers without speciftying a target host ([`580786e`](https://github.com/wasmCloud/wasmCloud/commit/580786e66b0d467b8af8799894ee85ce1643cd1a))
 * **[#202](https://github.com/wasmCloud/wasmCloud/issues/202)**
    - Cluster seed validation for CLI args (and interactive) ([`cf61264`](https://github.com/wasmCloud/wasmCloud/commit/cf6126436301f943a58a6dbc778014c093988307))
 * **[#203](https://github.com/wasmCloud/wasmCloud/issues/203)**
    - Remediate docker, snap, amd64 release issues ([`73c5e16`](https://github.com/wasmCloud/wasmCloud/commit/73c5e163010c22f178a176af72c6244d04729e2b))
 * **[#204](https://github.com/wasmCloud/wasmCloud/issues/204)**
    - Fetch from wasmcloud cache on inspect commands ([`95b3252`](https://github.com/wasmCloud/wasmCloud/commit/95b325246e34bbe47d81799acfcd07cd6cc6b9ea))
    - Fix --rpc-jwt auth mechanism ([`4e7d138`](https://github.com/wasmCloud/wasmCloud/commit/4e7d1380669d512dded2e4cebabf6aca166a9cc5))
 * **[#208](https://github.com/wasmCloud/wasmCloud/issues/208)**
    - Reintroduce --timeout with deprecation warning ([`7c913e0`](https://github.com/wasmCloud/wasmCloud/commit/7c913e069e154084bbe8b08f55d3dbca6eb55ad3))
 * **[#211](https://github.com/wasmCloud/wasmCloud/issues/211)**
    - Start actor count ([`cfd8c05`](https://github.com/wasmCloud/wasmCloud/commit/cfd8c053d5375b69140e0bc9d203ae6974aceac4))
 * **[#214](https://github.com/wasmCloud/wasmCloud/issues/214)**
    - Warning if contract IDs look like an nkey ([`4e16308`](https://github.com/wasmCloud/wasmCloud/commit/4e16308d4f12fbac49d3de8340495c5e29266009))
 * **[#216](https://github.com/wasmCloud/wasmCloud/issues/216)**
    - Add `cfg` module to get or create the `.wash` directory ([`7d0e031`](https://github.com/wasmCloud/wasmCloud/commit/7d0e031a57564cd550e7e2db48f939403ad22cd8))
 * **[#22](https://github.com/wasmCloud/wasmCloud/issues/22)**
    - Add support for publishing registry credential map to lattice ([`e54cef9`](https://github.com/wasmCloud/wasmCloud/commit/e54cef9c99a2d94902eedd1ce37009ed7d703a9b))
 * **[#220](https://github.com/wasmCloud/wasmCloud/issues/220)**
    - Replace .unwrap() with ? in image reference parsing ([`ac23edc`](https://github.com/wasmCloud/wasmCloud/commit/ac23edccf25eb6462735a0d67c532de7c56af4b7))
 * **[#221](https://github.com/wasmCloud/wasmCloud/issues/221)**
    - Refactor output messages to better support JSON ([`a437d26`](https://github.com/wasmCloud/wasmCloud/commit/a437d2612e7f671ec68a8e513bb32a34be0b9d72))
 * **[#223](https://github.com/wasmCloud/wasmCloud/issues/223)**
    - Fix for #210 ([`09a74ca`](https://github.com/wasmCloud/wasmCloud/commit/09a74ca29bc01ebfe274ed53f091d039cbb603ed))
 * **[#226](https://github.com/wasmCloud/wasmCloud/issues/226)**
    - Use lattice controller client & wasmbus linked to nats-aflowt ([`23c80d0`](https://github.com/wasmCloud/wasmCloud/commit/23c80d06880f7216138f5ed62b815ccbe16d335a))
 * **[#229](https://github.com/wasmCloud/wasmCloud/issues/229)**
    - Added count flag to start actor command ([`e542dd7`](https://github.com/wasmCloud/wasmCloud/commit/e542dd7291f881ef872523825de158099fa59333))
 * **[#230](https://github.com/wasmCloud/wasmCloud/issues/230)**
    - Renamed windows set_keys util function ([`dd311f7`](https://github.com/wasmCloud/wasmCloud/commit/dd311f7de0aa598797a76400dce8f56c6b68a092))
 * **[#236](https://github.com/wasmCloud/wasmCloud/issues/236)**
    - Support provider configuration json for start ([`d9b7744`](https://github.com/wasmCloud/wasmCloud/commit/d9b7744369a71f356ffb10367e9b965df372a0cd))
 * **[#237](https://github.com/wasmCloud/wasmCloud/issues/237)**
    - Update dependencies ([`6c12b43`](https://github.com/wasmCloud/wasmCloud/commit/6c12b43a7036c8a77c3f3567b9b3e1059ffd86f1))
 * **[#240](https://github.com/wasmCloud/wasmCloud/issues/240)**
    - Add git initalization and flag ([`80340a2`](https://github.com/wasmCloud/wasmCloud/commit/80340a26bdb0688efaff23b59a456fdf16dd41e3))
 * **[#245](https://github.com/wasmCloud/wasmCloud/issues/245)**
    - Implement wait for `ctl` commands ([`189639f`](https://github.com/wasmCloud/wasmCloud/commit/189639f98aa5b9669da8143010c09600e2be449d))
 * **[#246](https://github.com/wasmCloud/wasmCloud/issues/246)**
    - Issue 57 - Use indicatif instead of spinners library for command line waiting spinner ([`eff72c4`](https://github.com/wasmCloud/wasmCloud/commit/eff72c49e23c8a079e2c142e138e5279e570731d))
 * **[#254](https://github.com/wasmCloud/wasmCloud/issues/254)**
    - Overriding usage to add link_name ([`4e9ccc4`](https://github.com/wasmCloud/wasmCloud/commit/4e9ccc406b7ca996fb1e6153aa93732d62047186))
 * **[#255](https://github.com/wasmCloud/wasmCloud/issues/255)**
    - Fixed breaking clap change, resolved clippy warnings ([`15cf2eb`](https://github.com/wasmCloud/wasmCloud/commit/15cf2eb61b63c7c49d00272c534d24716f8ae16c))
 * **[#257](https://github.com/wasmCloud/wasmCloud/issues/257)**
    - Added support for oci manifest annotations ([`f024faa`](https://github.com/wasmCloud/wasmCloud/commit/f024faae04b993c19e1d0551178705d268cbb0db))
 * **[#269](https://github.com/wasmCloud/wasmCloud/issues/269)**
    - Support registry pushing with layers ([`465563b`](https://github.com/wasmCloud/wasmCloud/commit/465563b8a4d326bfcd3b8520ccc3c5d610b99d42))
 * **[#271](https://github.com/wasmCloud/wasmCloud/issues/271)**
    - Add tinygo actor template ([`af1476b`](https://github.com/wasmCloud/wasmCloud/commit/af1476b4ce872af2991d73f5e9937bebe0798566))
 * **[#272](https://github.com/wasmCloud/wasmCloud/issues/272)**
    - Add codegen for TinyGo actors ([`6626460`](https://github.com/wasmCloud/wasmCloud/commit/6626460199a7d44fe4ee7293e6cd8015aab7cd9f))
 * **[#276](https://github.com/wasmCloud/wasmCloud/issues/276)**
    - Update dependencies, fix clippy warnings, bump to 0.11.0 ([`1c09908`](https://github.com/wasmCloud/wasmCloud/commit/1c0990885d4d99e4545d3c6b9847d371cab26edb))
 * **[#278](https://github.com/wasmCloud/wasmCloud/issues/278)**
    - Upgraded to oci-distribution v0.9.1 to fix chunking ([`7b08bd6`](https://github.com/wasmCloud/wasmCloud/commit/7b08bd63db10bf7dab32071188fe8405fafcca3f))
 * **[#28](https://github.com/wasmCloud/wasmCloud/issues/28)**
    - Update/async nats ([`7c4663b`](https://github.com/wasmCloud/wasmCloud/commit/7c4663b3f93ace4cf19c8e3584c00d2c7b69aafd))
 * **[#280](https://github.com/wasmCloud/wasmCloud/issues/280)**
    - Remove cargo dependency and add GitReference enum to git.rs ([`ae3ffce`](https://github.com/wasmCloud/wasmCloud/commit/ae3ffcee0bb159ab8e58c3af2c847c3ae806273f))
 * **[#287](https://github.com/wasmCloud/wasmCloud/issues/287)**
    - Format error messages with variable substitution ([`f94b602`](https://github.com/wasmCloud/wasmCloud/commit/f94b602044ac33df53f1ff76a160549821134e93))
 * **[#288](https://github.com/wasmCloud/wasmCloud/issues/288)**
    - Refactor to use async-nats ([`1d240ab`](https://github.com/wasmCloud/wasmCloud/commit/1d240ab63d50210086079d553fbd78ba5bbc63b8))
 * **[#294](https://github.com/wasmCloud/wasmCloud/issues/294)**
    - `wash up` implementation ([`3104999`](https://github.com/wasmCloud/wasmCloud/commit/3104999bbbf9e86a806183d6978597a1f30140c1))
 * **[#297](https://github.com/wasmCloud/wasmCloud/issues/297)**
    - Create `wash build` command and add configuration parsing ([`f72ca88`](https://github.com/wasmCloud/wasmCloud/commit/f72ca88373870c688efb0144b796a8e67dc2aaf8))
 * **[#298](https://github.com/wasmCloud/wasmCloud/issues/298)**
    - Update all dependencies ([`bb85023`](https://github.com/wasmCloud/wasmCloud/commit/bb8502372c4b95d20cdc980e872f8d34bf8c48e4))
 * **[#300](https://github.com/wasmCloud/wasmCloud/issues/300)**
    - Use `Into<String>` generic to make `CommandOutput` slightly easier to construct. ([`d2762b2`](https://github.com/wasmCloud/wasmCloud/commit/d2762b292b1360b31d33dce18fff1ef32f533da8))
 * **[#303](https://github.com/wasmCloud/wasmCloud/issues/303)**
    - Update wash-lib with minimum version requirement and mix releases ([`13d44c7`](https://github.com/wasmCloud/wasmCloud/commit/13d44c7085951b523427624108fd3cf1415a53b6))
 * **[#308](https://github.com/wasmCloud/wasmCloud/issues/308)**
    - Set the default branch to main instead of master for new git repos ([`301ac97`](https://github.com/wasmCloud/wasmCloud/commit/301ac97fc7a85177ac12ad98ff2387b65af9d827))
 * **[#31](https://github.com/wasmCloud/wasmCloud/issues/31)**
    - Removes the confusing and unnecessary dependency on the capability provider contract interface. ([`5903204`](https://github.com/wasmCloud/wasmCloud/commit/59032048fa23453754d29aef8abc2fb53071eb81))
 * **[#310](https://github.com/wasmCloud/wasmCloud/issues/310)**
    - Add more context to `ctl` error messages ([`01ad509`](https://github.com/wasmCloud/wasmCloud/commit/01ad509c36663bc87ce00185f96494122ea4a985))
 * **[#311](https://github.com/wasmCloud/wasmCloud/issues/311)**
    - Add `backtrace` to error output when `RUST_BACKTRACE=1` ([`401df0a`](https://github.com/wasmCloud/wasmCloud/commit/401df0ac3570a81470d14577308f8f3490e87ea0))
 * **[#319](https://github.com/wasmCloud/wasmCloud/issues/319)**
    - Changed ctrl-c waiter to not spin a CPU ([`5a35b36`](https://github.com/wasmCloud/wasmCloud/commit/5a35b3623fc11a4afb8a3f6c57e1d838f6efdca0))
 * **[#320](https://github.com/wasmCloud/wasmCloud/issues/320)**
    - Observe WASMCLOUD_CTL_TOPIC_PREFIX ([`d3521cb`](https://github.com/wasmCloud/wasmCloud/commit/d3521cb7f4f830ae919691da84464ba30eaef6c5))
 * **[#324](https://github.com/wasmCloud/wasmCloud/issues/324)**
    - Remove git2 and openssh dependencies ([`17e657d`](https://github.com/wasmCloud/wasmCloud/commit/17e657dce7f823c125137618cadb6e36408c3065))
 * **[#327](https://github.com/wasmCloud/wasmCloud/issues/327)**
    - Feat/wash down ([`33cdd7d`](https://github.com/wasmCloud/wasmCloud/commit/33cdd7d763acb490a67556fbcbc2c4e42ccd907e))
 * **[#33](https://github.com/wasmCloud/wasmCloud/issues/33)**
    - Added support for constructing a client with prefix ([`a963f3a`](https://github.com/wasmCloud/wasmCloud/commit/a963f3a7f6de9c387a10925333acf56b94083c85))
 * **[#334](https://github.com/wasmCloud/wasmCloud/issues/334)**
    - Add a border to the ascii art to prevent the leading whitespace from being stripped ([`2520b7c`](https://github.com/wasmCloud/wasmCloud/commit/2520b7c68e5b1d33569b867f2f2431f8022108c2))
 * **[#338](https://github.com/wasmCloud/wasmCloud/issues/338)**
    - Add name to inspect json output ([`1138fe0`](https://github.com/wasmCloud/wasmCloud/commit/1138fe0e743e85b02c1b1e1f4adc2ed277267f28))
 * **[#340](https://github.com/wasmCloud/wasmCloud/issues/340)**
    - Converted OCI URLs to lowercase ([`9007837`](https://github.com/wasmCloud/wasmCloud/commit/9007837091577f4554f981259c985a1509d27fd3))
 * **[#35](https://github.com/wasmCloud/wasmCloud/issues/35)**
    - Add `#[derive(Debug)]` to Client ([`39e4e0e`](https://github.com/wasmCloud/wasmCloud/commit/39e4e0e8a49d3436bce0d863329e5b3924ba3f7a))
    - Adding support for the cluster claims type ([`c357d7d`](https://github.com/wasmCloud/wasmCloud/commit/c357d7d11041a6c06784725fc5d21330b69d0875))
 * **[#353](https://github.com/wasmCloud/wasmCloud/issues/353)**
    - Moved project build functionality to wash-lib ([`c31a5d4`](https://github.com/wasmCloud/wasmCloud/commit/c31a5d4d05427874fa9fc408f70a9072b4fd1ecd))
 * **[#354](https://github.com/wasmCloud/wasmCloud/issues/354)**
    - Fixed 352, added js_domain to context ([`c7f4c1d`](https://github.com/wasmCloud/wasmCloud/commit/c7f4c1d43d51582443dd657dde8c949c3e78f9de))
 * **[#355](https://github.com/wasmCloud/wasmCloud/issues/355)**
    - Moved generate module to wash-lib ([`9fa5331`](https://github.com/wasmCloud/wasmCloud/commit/9fa53311a6d674a1c532a770ea636c93562c962f))
 * **[#36](https://github.com/wasmCloud/wasmCloud/issues/36)**
    - Updated logging and extras to OTP builtin/logging contract ([`676d37e`](https://github.com/wasmCloud/wasmCloud/commit/676d37ecf329a94af0ad9089a8cc70077c678841))
 * **[#362](https://github.com/wasmCloud/wasmCloud/issues/362)**
    - Change test reg port ([`c805cbc`](https://github.com/wasmCloud/wasmCloud/commit/c805cbc032f0d828dfe666657bfeb553c74a0f33))
 * **[#376](https://github.com/wasmCloud/wasmCloud/issues/376)**
    - Create default context if host_config not found ([`51d4748`](https://github.com/wasmCloud/wasmCloud/commit/51d474851dbcf325cc6b422f9ee09486e43c6984))
 * **[#39](https://github.com/wasmCloud/wasmCloud/issues/39)**
    - Removes dependency on no longer maintained parity-wasm crate ([`7c9922b`](https://github.com/wasmCloud/wasmCloud/commit/7c9922bf42b53f8315e3899eae97fd3449179a2f))
 * **[#393](https://github.com/wasmCloud/wasmCloud/issues/393)**
    - Fix clippy lints ([`030b844`](https://github.com/wasmCloud/wasmCloud/commit/030b8449d46d880b3b9c4897870c7ea3c74ff003))
 * **[#398](https://github.com/wasmCloud/wasmCloud/issues/398)**
    - Fix `wash up --nats-connect-only` killing NATS servers it didn't start ([`462364d`](https://github.com/wasmCloud/wasmCloud/commit/462364d5d294b3527ebe022f942961a98a63784b))
 * **[#399](https://github.com/wasmCloud/wasmCloud/issues/399)**
    - Use exact imports instead of globs ([`95851b6`](https://github.com/wasmCloud/wasmCloud/commit/95851b667bd7d23d0c2114cd550f082db6cd935b))
 * **[#40](https://github.com/wasmCloud/wasmCloud/issues/40)**
    - Minor addition to test ([`0e95f67`](https://github.com/wasmCloud/wasmCloud/commit/0e95f67f46dcb6d651c27a31fc310fe09a70f374))
 * **[#41](https://github.com/wasmCloud/wasmCloud/issues/41)**
    - Strips previous embedded JWT before embedding new one ([`c924fa4`](https://github.com/wasmCloud/wasmCloud/commit/c924fa4da6ae554ddb3034dad8353bbb66a8d1fa))
 * **[#42](https://github.com/wasmCloud/wasmCloud/issues/42)**
    - Adding support for new kv bucket for metadata ([`abbf0a6`](https://github.com/wasmCloud/wasmCloud/commit/abbf0a66c4085962e5d1d1ff129a46607d0fc819))
 * **[#44](https://github.com/wasmCloud/wasmCloud/issues/44)**
    - Support the new abi for embedding and extracting claims ([`48eafc8`](https://github.com/wasmCloud/wasmCloud/commit/48eafc861099b08a531b7eeb033802ab8a215baf))
 * **[#452](https://github.com/wasmCloud/wasmCloud/issues/452)**
    - Feat/wash inspect ([`0b2f0d3`](https://github.com/wasmCloud/wasmCloud/commit/0b2f0d3c1d56d1a7d2f8fed0f389a82846817051))
 * **[#46](https://github.com/wasmCloud/wasmCloud/issues/46)**
    - Fixing incompatible hashes between v0.9.0 and v0.10.0 ([`8612e5d`](https://github.com/wasmCloud/wasmCloud/commit/8612e5d64469e2981cb0d6a4ff559890d9e3c124))
 * **[#48](https://github.com/wasmCloud/wasmCloud/issues/48)**
    - Adds support for JSON schema to capability provider claims ([`a1299cc`](https://github.com/wasmCloud/wasmCloud/commit/a1299cc722a122cbde590047bcad9d3edb57d6c2))
 * **[#520](https://github.com/wasmCloud/wasmCloud/issues/520)**
    - Feat(*) wadm 0.4 support in `wash app` ([`b3e2615`](https://github.com/wasmCloud/wasmCloud/commit/b3e2615b225d4fbc5eb8b4cb58c5755df0f68bbc))
 * **[#556](https://github.com/wasmCloud/wasmCloud/issues/556)**
    - Feat(*) wash burrito support ([`812f0e0`](https://github.com/wasmCloud/wasmCloud/commit/812f0e0bc44fd9cbab4acb7be44005657234fa7c))
 * **[#677](https://github.com/wasmCloud/wasmCloud/issues/677)**
    - Adding the ability to inspect and inject configuration schemas ([`db3fe8d`](https://github.com/wasmCloud/wasmCloud/commit/db3fe8d7da82cd43389beaf33eed754c0d1a5f19))
 * **[#7](https://github.com/wasmCloud/wasmCloud/issues/7)**
    - Adding the ability to mutate schema from outside the crate ([`5a5eb50`](https://github.com/wasmCloud/wasmCloud/commit/5a5eb500efff41baacb664dd569f0f70c77a7451))
 * **Uncategorized**
    - Support policy service ([`2ebdab7`](https://github.com/wasmCloud/wasmCloud/commit/2ebdab7551f6da93967d921316cae5d04a409a43))
    - Cfg macro for unix only signals ([`2f35af9`](https://github.com/wasmCloud/wasmCloud/commit/2f35af97ed4eb6eac99e62ef7dcd8c13eaa04d39))
    - Add SIGTERM support alongside ctrl_c ([`c71f71a`](https://github.com/wasmCloud/wasmCloud/commit/c71f71aa010861d1c253d9437cd1ef345e7f0c33))
    - Do not set a default value for traces exporter ([`bd969b7`](https://github.com/wasmCloud/wasmCloud/commit/bd969b7e54d912109b36f61f2907d4d39a63ca3a))
    - Off-by-one range check ([`fe338a7`](https://github.com/wasmCloud/wasmCloud/commit/fe338a7d9820f055e2f8c6826aeb4c53ddb1fd71))
    - Merge pull request from GHSA-5rgm-x48h-2mfm ([`664d9b9`](https://github.com/wasmCloud/wasmCloud/commit/664d9b9ae34f981d5c5a3bb6403530253894361c))
    - Replace lazy_static with once_cell ([`e1d7356`](https://github.com/wasmCloud/wasmCloud/commit/e1d7356bb0a07af9f4e6b1626f5df33709f3ed78))
    - Construct a strongly typed HostData to send to providers ([`23f1759`](https://github.com/wasmCloud/wasmCloud/commit/23f1759e818117f007df8d9b1bdfdfa7710c98c5))
    - Support OTEL traces end-to-end ([`675d364`](https://github.com/wasmCloud/wasmCloud/commit/675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6))
    - Update wadm to 0.5.0 ([`a74b502`](https://github.com/wasmCloud/wasmCloud/commit/a74b50297496578e5e6c0ee806304a3ff05cd073))
    - Support registry settings via config service and command-line flags ([`48d4557`](https://github.com/wasmCloud/wasmCloud/commit/48d4557c8ee895278055261bccb1293806b308b0))
    - Merge pull request #723 from Iceber/fix_error ([`d05cf72`](https://github.com/wasmCloud/wasmCloud/commit/d05cf72f624ceb8eeb81f43f8cb3b112e407370d))
    - Fix error returned by host startup failures ([`5db9786`](https://github.com/wasmCloud/wasmCloud/commit/5db97861d96aae1e90bb0a54ffc5c7938a75fba5))
    - Store typed keys, not strings ([`75a1fb0`](https://github.com/wasmCloud/wasmCloud/commit/75a1fb075357ac2566fef1b45c930e6c400a4041))
    - Establish NATS connections concurrently ([`0db5a5b`](https://github.com/wasmCloud/wasmCloud/commit/0db5a5ba5b20535e16af46fd92f7040c8174d636))
    - Implement structured logging ([`ed64180`](https://github.com/wasmCloud/wasmCloud/commit/ed64180714873bd9be1f9008d29b09cbf276bba1))
    - Respect allow_file_load ([`ff02491`](https://github.com/wasmCloud/wasmCloud/commit/ff024913d3107dc65dd8aad69a1f598390de6d1a))
    - Parse lists with a delimiter ([`ed24d3e`](https://github.com/wasmCloud/wasmCloud/commit/ed24d3ecda0c28141114933d9af2a1cd44d998c8))
    - Remove irrelevant ipv6 flag ([`e158789`](https://github.com/wasmCloud/wasmCloud/commit/e15878948673f9ad1cfbbafdc01c48c2d2678955))
    - Refactor args parsing ([`e459a7a`](https://github.com/wasmCloud/wasmCloud/commit/e459a7a2434ce926d211ff37c1f6ebef2b5faef5))
    - Implement ctl_topic_prefix ([`3588b5f`](https://github.com/wasmCloud/wasmCloud/commit/3588b5f9ce2f0c0a4718d9bd576904ef77682304))
    - Add nats_jwt and nats_seed as fallbacks ([`dbcb847`](https://github.com/wasmCloud/wasmCloud/commit/dbcb84733099251ae600573ab4b48193324124b6))
    - Implement rpc,ctl,prov_rpc connections ([`921fa78`](https://github.com/wasmCloud/wasmCloud/commit/921fa784ba3853b6b0a622c6850bb6d71437a011))
    - Unhide implemented CLI args ([`c2f765d`](https://github.com/wasmCloud/wasmCloud/commit/c2f765d5c25a18e5f79379955cd77ed4858954bd))
    - Use allow_latest and allowed_insecure config ([`7c389be`](https://github.com/wasmCloud/wasmCloud/commit/7c389bee17d34db732babde7724286656c268f65))
    - Use js_domain provided by cli ([`9897b90`](https://github.com/wasmCloud/wasmCloud/commit/9897b90e845470faa35e8caf4816c29e6dcefd91))
    - Implement graceful provider shutdown delay ([`7d290aa`](https://github.com/wasmCloud/wasmCloud/commit/7d290aa08b2196a6082972a4d662bf1a93d07dec))
    - Use log level provided by cli ([`1d63fd9`](https://github.com/wasmCloud/wasmCloud/commit/1d63fd94b2bbb73992e0972eccfad1e617db846d))
    - Maintain cluster issuers list ([`194f791`](https://github.com/wasmCloud/wasmCloud/commit/194f791c16ad6a7106393b4bcf0d0c51a70f638d))
    - Remove unused `pub` ([`ad82a33`](https://github.com/wasmCloud/wasmCloud/commit/ad82a331d4f72c2dcc15f20559eca4aab0575bdc))
    - Ignore .DS_Store ([`c9fef73`](https://github.com/wasmCloud/wasmCloud/commit/c9fef73977b86172afc5d3f2e8c4830c0277aff3))
    - Scaffold cli args ([`abb2de1`](https://github.com/wasmCloud/wasmCloud/commit/abb2de11f159191357f1676b11fe07bd39c5573c))
    - Do not panic on errors ([`2abd61a`](https://github.com/wasmCloud/wasmCloud/commit/2abd61a5d5a74847417be603412804487c8489c4))
    - Add support for custom lattice prefix ([`c9fecb9`](https://github.com/wasmCloud/wasmCloud/commit/c9fecb99793649a6f9321b9224f85b9472889dec))
    - Rename most instances of lattice and wasmbus to host ([`f3f6c21`](https://github.com/wasmCloud/wasmCloud/commit/f3f6c21f25632940a6cc1d5290f8e84271609496))
    - Safely parse null versions as empty string ([`0ec90cd`](https://github.com/wasmCloud/wasmCloud/commit/0ec90cd5fd2c7018dd9614e9777f508b9969e7b1))
    - Bump to 0.28, async-nats 0.31 ([`710ab08`](https://github.com/wasmCloud/wasmCloud/commit/710ab08386bd57da080c8a207ed18ea7f9ed217f))
    - Add missing fields for host inventory and auction ack ([`5b38e10`](https://github.com/wasmCloud/wasmCloud/commit/5b38e1007723032b847efd9402f2a16b22e80b54))
    - Merge pull request #683 from wasmCloud/feat/single-host-inventory-query ([`3fe92ae`](https://github.com/wasmCloud/wasmCloud/commit/3fe92aefcf573a52f7f67a30d06daba33861427c))
    - Allow get inventory to query the only host ([`acdcd95`](https://github.com/wasmCloud/wasmCloud/commit/acdcd957bfedb5a86a0420c052da1e65d32e6c23))
    - Merge pull request #663 from vados-cosmonic/feat/support-adapting-p2-components ([`28c4aa6`](https://github.com/wasmCloud/wasmCloud/commit/28c4aa66a5c113c08ade5da1ead303f6b932afaf))
    - Build wasi preview components from wash ([`4144f71`](https://github.com/wasmCloud/wasmCloud/commit/4144f711ad2056e9334e085cbe08663065605b0c))
    - Fix stop nats after starting wasmcloud host failed ([`3c32ae3`](https://github.com/wasmCloud/wasmCloud/commit/3c32ae32b46d36fbb0a38d0a087a291d4e228a11))
    - Ensure that the wasmcloud-host can be stopped gracefully ([`bc35010`](https://github.com/wasmCloud/wasmCloud/commit/bc35010697e1eca3eb2093ee6aa5302a9bd1d437))
    - Async_nats 0.30, removed wasmbus_rpc dep ([`673f8bc`](https://github.com/wasmCloud/wasmCloud/commit/673f8bc4e4bfebb49088dddd749c41acf42ae242))
    - Implement host stop ([`ec5675d`](https://github.com/wasmCloud/wasmCloud/commit/ec5675d11768ed9741a8d3e7c42cc1e5a823d41d))
    - Introduce wasmbus lattice ([`ef20466`](https://github.com/wasmCloud/wasmCloud/commit/ef20466a04d475159088b127b46111b80a5e1eb2))
    - Merge pull request #643 from lachieh/detachable-washboard ([`6402d13`](https://github.com/wasmCloud/wasmCloud/commit/6402d13de96ad18516dd5efc530b1c3f05964df1))
    - Add standalone washboard (experimental) ([`12fdad0`](https://github.com/wasmCloud/wasmCloud/commit/12fdad013f5222dd21fdf63f1c7b2f0c37098b89))
    - Implement data streaming ([`7364dd8`](https://github.com/wasmCloud/wasmCloud/commit/7364dd8afae5c8884ca923b39c5680c60d8d0e3d))
    - Merge pull request #629 from thomastaylor312/fix/multiple_nats ([`389a702`](https://github.com/wasmCloud/wasmCloud/commit/389a7023b9a6c584d27e2b48573f21e7b09c41ba))
    - The wadm pidfile is not removed when wadm is not started ([`ae01c02`](https://github.com/wasmCloud/wasmCloud/commit/ae01c022f5793b76b0c37700890514be64734d9e))
    - Fix stop and cleanup wadm ([`7d4e6d4`](https://github.com/wasmCloud/wasmCloud/commit/7d4e6d46d7bb7a53a4860c499eb6dbab8a1b0a4c))
    - Allows multiple hosts to run without sharing data ([`4900f82`](https://github.com/wasmCloud/wasmCloud/commit/4900f82caf39913e076c1664702d9e9d02836135))
    - Delete useless src/start.rs ([`c8a2d0b`](https://github.com/wasmCloud/wasmCloud/commit/c8a2d0b99bed8c92cd51d95c8a62addb67f2bb1d))
    - Merge pull request #622 from Iceber/update_help ([`4a53914`](https://github.com/wasmCloud/wasmCloud/commit/4a5391431a19ac2a4997d86dec6d3879adf21bcc))
    - Add `wash get` to help text ([`b7b4338`](https://github.com/wasmCloud/wasmCloud/commit/b7b43385ef52c0026b65d6eefe85d7bd12d15682))
    - Merge pull request #608 from vados-cosmonic/docs/ux/add-flattened-commands-to-help-text ([`a6a3388`](https://github.com/wasmCloud/wasmCloud/commit/a6a33885c54d89214adc19e2d6322f0d54ad7d3c))
    - Add top level commands to help text, restructure ([`b7016b6`](https://github.com/wasmCloud/wasmCloud/commit/b7016b648d5f7f1d3605e6dff933d1e58c8a797c))
    - Merge pull request #610 from vados-cosmonic/feat/add-wash-dev ([`00e0aea`](https://github.com/wasmCloud/wasmCloud/commit/00e0aea33815b6ac5abdb4c2cf2a5815ebe35cb3))
    - Add wash dev command ([`e9fe020`](https://github.com/wasmCloud/wasmCloud/commit/e9fe020a0906cb377f6ea8bd3a9879e5bad877b7))
    - Moved registry cli things to registry cli ([`1172806`](https://github.com/wasmCloud/wasmCloud/commit/1172806ea5a7e2a24d4570d76cf53f104a0d3e30))
    - Refuse to stop NATS when pidfile absent ([`44832b2`](https://github.com/wasmCloud/wasmCloud/commit/44832b2b3de0fe3246e1c5b9609fbe8f03bc7cc8))
    - Merge pull request #612 from thomastaylor312/feat/wash_capture ([`3a14bbc`](https://github.com/wasmCloud/wasmCloud/commit/3a14bbc9999e680f5044223aff7d13c0e3b319bc))
    - Adds a new experimental `wash capture` command ([`8c96789`](https://github.com/wasmCloud/wasmCloud/commit/8c96789f1c793c5565715080b84fecfbe0653b43))
    - Merge pull request #603 from thomastaylor312/feat/wash_spy ([`213ac6b`](https://github.com/wasmCloud/wasmCloud/commit/213ac6b8e9b3d745764d8df1f20ceb41b10cd1f2))
    - Adds `wash spy` command with experimental flag support ([`e58c6a6`](https://github.com/wasmCloud/wasmCloud/commit/e58c6a60928a7157ffbbc95f9eabcc9cae3db2a7))
    - Bumps wadm to 0.4.0 stable ([`41d3d3c`](https://github.com/wasmCloud/wasmCloud/commit/41d3d3cfa2e5a285833c8ecd2a21bb6821d2f47e))
    - Add deprecation warnings for changed CLI commands ([`d2bc216`](https://github.com/wasmCloud/wasmCloud/commit/d2bc21681306ef2251be4347224249e2ce8c4c18))
    - Flatten multiple commands into wash get ([`6923ce7`](https://github.com/wasmCloud/wasmCloud/commit/6923ce7efb721f8678c33f42647b87ea33a7653a))
    - Update WIT dependencies ([`ed4282c`](https://github.com/wasmCloud/wasmCloud/commit/ed4282c9ea1bb95e346c9a981acdc264b0fc9d3f))
    - Merge pull request #580 from vados-cosmonic/feat/ux/wash-reg-push-and-pull ([`a553348`](https://github.com/wasmCloud/wasmCloud/commit/a553348a44b430937bd3222600a477f52300fb74))
    - Flatten wash reg push/pull into wash push/pull ([`4daf51b`](https://github.com/wasmCloud/wasmCloud/commit/4daf51be422d395bc0142d62b8d59060b89feafa))
    - Merge pull request #576 from vados-cosmonic/feat/ux/flatten-wash-stop ([`7b66d65`](https://github.com/wasmCloud/wasmCloud/commit/7b66d6575e8f1b360ff331e171bc784d96e3681a))
    - Flatten `wash ctl stop` into `wash stop` ([`128f760`](https://github.com/wasmCloud/wasmCloud/commit/128f7603c67443f23e76c3cb4bd1468ffd8f5462))
    - Merge pull request #573 from vados-cosmonic/feat/ux/flatten-wash-start ([`612951b`](https://github.com/wasmCloud/wasmCloud/commit/612951ba8ac5078f4234677c842b41c729f08985))
    - Flatten `wash ctl start` into `wash start` ([`2a6c401`](https://github.com/wasmCloud/wasmCloud/commit/2a6c401834b4cb55ef420538e15503b98281eaf1))
    - Merge pull request #569 from vados-cosmonic/feat/ux/flatten-wash-link ([`def34b6`](https://github.com/wasmCloud/wasmCloud/commit/def34b60b5fea48a3747b661a7a7daf2fb8daff7))
    - Flatten `wash ctl link` into `wash link` ([`24bba48`](https://github.com/wasmCloud/wasmCloud/commit/24bba484009be9e87bfcbd926a731534e936c339))
    - Bumped wadm to 0.4.0-alpha.3 ([`a01b605`](https://github.com/wasmCloud/wasmCloud/commit/a01b605041e9b2041944a939ae00f9d38e782f26))
    - Override help text with RFC command structure ([`3c35705`](https://github.com/wasmCloud/wasmCloud/commit/3c35705a6a3dc2aad827425911db55662f4160cc))
    - Updated deps to fix link query error ([`ac740d9`](https://github.com/wasmCloud/wasmCloud/commit/ac740d9a6827643b6e472920e7949ffa7a56083f))
    - Implement builtin capabilities via WIT ([`caa965a`](https://github.com/wasmCloud/wasmCloud/commit/caa965ac17eeda67c35f41b38a236f1b682cf462))
    - "added call_context to component and module Ctx" ([`b7f000c`](https://github.com/wasmCloud/wasmCloud/commit/b7f000ce54ddfab9581a246100384c9792be32b5))
    - Fixed issue with wash connecting to host ([`5016539`](https://github.com/wasmCloud/wasmCloud/commit/5016539285d002ce93159d1582dc40a25b076169))
    - Bumped wasmcloud to v0.62 ([`04372e5`](https://github.com/wasmCloud/wasmCloud/commit/04372e514902143df921b6c8060c50615aa1705f))
    - Only use `bindeps` for WASI adapter build ([`77ed144`](https://github.com/wasmCloud/wasmCloud/commit/77ed1441bdd1da15e13ce9196138cfe7c037f6ba))
    - Introduce `test-actors` subcrate ([`8806ef1`](https://github.com/wasmCloud/wasmCloud/commit/8806ef1ff0afeb42986639d254e8d9cb918e1633))
    - Merge pull request #46 from connorsmith256/bump/async-nats ([`65a03c4`](https://github.com/wasmCloud/wasmCloud/commit/65a03c40efab5d14263c0739ed9af35b16fe1b37))
    - Update to wasmbus-rpc v0.13 and async-nats v0.29 ([`57c8534`](https://github.com/wasmCloud/wasmCloud/commit/57c8534641575de3b42fbb17ccca5eb30d1dbd43))
    - Merge pull request #513 from connorsmith256/feat/allow-file-upload ([`bf4e46c`](https://github.com/wasmCloud/wasmCloud/commit/bf4e46cf816fc3385540ca752dfdaa1fd13ae78e))
    - Enable WASMCLOUD_ALLOW_FILE_LOAD by default ([`7cca2e7`](https://github.com/wasmCloud/wasmCloud/commit/7cca2e76f0048bd37a50960c8df5b40ed0e16d7d))
    - Merge pull request #508 from aish-where-ya/main ([`6fd026c`](https://github.com/wasmCloud/wasmCloud/commit/6fd026ce1670a75f23bc93fdc9325d5bc756050d))
    - Call `_start` for tinygo actor modules ([`d7a4423`](https://github.com/wasmCloud/wasmCloud/commit/d7a4423d6ddf637830e0f3cdb57f77ad46a90131))
    - Minor fix ([`732f9e0`](https://github.com/wasmCloud/wasmCloud/commit/732f9e0eb9413c4fc90a8dc25133d02a15297477))
    - Stop nats server if host fails to connect to washboard ([`c31c3b5`](https://github.com/wasmCloud/wasmCloud/commit/c31c3b5bedd421b66d1e2c74ef765925e1431d70))
    - Fixes to localhost ([`888db54`](https://github.com/wasmCloud/wasmCloud/commit/888db540e294e1633cc324f1509821ce0c91e574))
    - Refactoring based on review comments ([`448211e`](https://github.com/wasmCloud/wasmCloud/commit/448211e55f8491fb9a12611e6c61615411cd47fd))
    - Wash up waits for washboard to be up ([`efaacd7`](https://github.com/wasmCloud/wasmCloud/commit/efaacd7d67bef6873980d9b8575dd268e13f941f))
    - Merge pull request #45 from connorsmith256/feat/expose-lattice-prefix ([`840e395`](https://github.com/wasmCloud/wasmCloud/commit/840e395a694d7562b1028d90e6f217918678fada))
    - Expose lattice_prefix on Client ([`c40ac19`](https://github.com/wasmCloud/wasmCloud/commit/c40ac19aed8077a73705af05a7b7f4936fbb6905))
    - Rename ns_prefix to lattice_prefix ([`484cd3c`](https://github.com/wasmCloud/wasmCloud/commit/484cd3c2186838a38336cef681404d82653829f7))
    - Added call_context to component and module Ctx ([`e20846a`](https://github.com/wasmCloud/wasmCloud/commit/e20846a55ccb2055159cc4b2a9ac942f91dd1f68))
    - Merge pull request #477 from connorsmith256/bump/wasmcloud-host-version ([`7dbd961`](https://github.com/wasmCloud/wasmCloud/commit/7dbd961378a314a0647e812b819abf014e08c004))
    - Bump to v0.61.0 of wasmcloud host ([`3d80c4e`](https://github.com/wasmCloud/wasmCloud/commit/3d80c4e1ce3bcc7e71cc4dbffe927ca87c524f42))
    - Format errors correctly ([`c6acc2c`](https://github.com/wasmCloud/wasmCloud/commit/c6acc2c6f6183515441d1dcaca073ba1df109df2))
    - Avoid capability checking by default ([`79e66a6`](https://github.com/wasmCloud/wasmCloud/commit/79e66a64a8d20926a18967e8efb970d2104e6596))
    - Merge pull request #279 from rvolosatovs/refactor/handlers ([`77790d2`](https://github.com/wasmCloud/wasmCloud/commit/77790d2f709ce217f0c6a2c64ae1fae9b06942d3))
    - Rework handler API ([`fa06410`](https://github.com/wasmCloud/wasmCloud/commit/fa064101e82385c2fb9c9cd0ce958d800a65d660))
    - Merge pull request #278 from rvolosatovs/ci/doc ([`17e6a4b`](https://github.com/wasmCloud/wasmCloud/commit/17e6a4bd35a993d5e5eb37a674bd3e1bc11ead98))
    - Fix broken references ([`8291586`](https://github.com/wasmCloud/wasmCloud/commit/82915861e422c845d97b3a8680738d55bd9bfce2))
    - Merge pull request #276 from rvolosatovs/feat/component-model ([`1aecee9`](https://github.com/wasmCloud/wasmCloud/commit/1aecee95c76faf262123d65801c3a09e45b0ff70))
    - Unexport low-level actor primitives ([`8cff2e5`](https://github.com/wasmCloud/wasmCloud/commit/8cff2e5b65fbb8b5e0578d1ce5ccb892e14caba7))
    - Support running actors in binary ([`d8900cc`](https://github.com/wasmCloud/wasmCloud/commit/d8900ccc62f1383ed231bee1b6a28fd434f74c5a))
    - Introduce `Actor` abstraction ([`bb89f2c`](https://github.com/wasmCloud/wasmCloud/commit/bb89f2c516339e155a6c942871907a2c044ee014))
    - Add support for `component-model` ([`6a7dd43`](https://github.com/wasmCloud/wasmCloud/commit/6a7dd430b64e5506996155bd6c64423e0065265e))
    - Merge pull request #274 from rvolosatovs/feat/runtime-config ([`128e358`](https://github.com/wasmCloud/wasmCloud/commit/128e3588b2a79593314d5d2c36d3313681019985))
    - Rename `external` handler to `hostcall` ([`010129a`](https://github.com/wasmCloud/wasmCloud/commit/010129a272ce327cbb251b874f6f4cf57a950f91))
    - Provide `HostHandlerBuilder` ([`4db7517`](https://github.com/wasmCloud/wasmCloud/commit/4db7517586aec531137e7f83836da3fcd684d18e))
    - Use u32, rather than i32 wasm values ([`a4902e2`](https://github.com/wasmCloud/wasmCloud/commit/a4902e25212f7261ed444f7483a07aa210283a16))
    - Remove redundant TODO ([`088cbe0`](https://github.com/wasmCloud/wasmCloud/commit/088cbe0a20c7486bfaa80ec0d69e18ab2a2a6902))
    - Introduce `Runtime`-wide instance config ([`fc65c2c`](https://github.com/wasmCloud/wasmCloud/commit/fc65c2cb27ad15e0ef27fa45e61a3d62c2d0c033))
    - Introduce `Runtime`-wide handler ([`fc6620a`](https://github.com/wasmCloud/wasmCloud/commit/fc6620a5ba92b1e6fce4e16c21cc4b6cb5ccae0d))
    - Include `claims` as capability handler parameter ([`3d19c94`](https://github.com/wasmCloud/wasmCloud/commit/3d19c94128bcb6643f6e939f930a503ab9b9ca94))
    - Split `actor` module ([`909e04f`](https://github.com/wasmCloud/wasmCloud/commit/909e04f06139de52304babfeef6839e172aac5c2))
    - Merge pull request #273 from rvolosatovs/init/host ([`a2ccbb2`](https://github.com/wasmCloud/wasmCloud/commit/a2ccbb2e1786d2251951f953bdc7f0cca1d8e9fa))
    - Introduce a `Handler` trait ([`4d7b83d`](https://github.com/wasmCloud/wasmCloud/commit/4d7b83df95ef8d039b9ceac96c34b9773744aa9d))
    - Add initial host implementation ([`a645105`](https://github.com/wasmCloud/wasmCloud/commit/a645105802b22a719c8c5ae9232c6ea27170a019))
    - Merge branch 'main' into fix/nextest-usage-in-makefile ([`03c02f2`](https://github.com/wasmCloud/wasmCloud/commit/03c02f270faed157c95dd01ee42069610662314b))
    - Merge pull request #392 from vados-cosmonic/feat/completions ([`abbe44a`](https://github.com/wasmCloud/wasmCloud/commit/abbe44a9ce66cd2a782825ddf583a8f9c1bd5e56))
    - Shell completions ([`0dca3ef`](https://github.com/wasmCloud/wasmCloud/commit/0dca3ef4bb3db38aae6dbea57b520a36ef058e2f))
    - Merge pull request #404 from connorsmith256/update/control-interface-kv-support ([`35af263`](https://github.com/wasmCloud/wasmCloud/commit/35af26395dfdd70372921ed61324cd387ab1c6bc))
    - Support custom js_domain from context and via explicit flag ([`c668ed2`](https://github.com/wasmCloud/wasmCloud/commit/c668ed2e85cfb2a3fb115dae554b8b4830146aaf))
    - Use client builder to support querying KV bucket directly ([`1aa089c`](https://github.com/wasmCloud/wasmCloud/commit/1aa089cda377993c980c538107b3e4c7f51e9267))
    - Merge pull request #384 from thomastaylor312/fix/actually_tear_down ([`7952883`](https://github.com/wasmCloud/wasmCloud/commit/79528832836d9ee880d6b1a973b38a32fc9e72f8))
    - Makes sure we can actually shutdown a detached process ([`09e61b5`](https://github.com/wasmCloud/wasmCloud/commit/09e61b5c9b67fe4dd583872fc0f35fd0295fbbd4))
    - Merge pull request #381 from wasmCloud/bump/0.15.0-wasmcloud-0.60.0 ([`b06b71b`](https://github.com/wasmCloud/wasmCloud/commit/b06b71b68ba78405a321a9bbd6968f1ad8b461b7))
    - Makes sure that wash downloads different versions of wasmcloud ([`2e69e12`](https://github.com/wasmCloud/wasmCloud/commit/2e69e12d4b78f5ea7710ba12226345440e7541ef))
    - Updated wasmCloud version to 0.60 ([`b145702`](https://github.com/wasmCloud/wasmCloud/commit/b145702bd7ff942c8758fa36480c00c6c9e8280a))
    - Merge pull request #43 from connorsmith256/add-log ([`1da36e3`](https://github.com/wasmCloud/wasmCloud/commit/1da36e3b146e8315754fc8a60617d839e90c0df0))
    - Oh clippy, witness my sacrifice and recognize my faith ([`e4daf35`](https://github.com/wasmCloud/wasmCloud/commit/e4daf35b637ad1cc5a78c8608ccf8554d14bdeef))
    - Add log message specifying how link queries will be performed ([`4a1472a`](https://github.com/wasmCloud/wasmCloud/commit/4a1472aabd55f549a96edf640ee8ad326d97e3b2))
    - Merge branch 'main' into bump-wascap ([`cd35ff9`](https://github.com/wasmCloud/wasmCloud/commit/cd35ff9a4994469b45318a34fed8b13e6312cf95))
    - Merge pull request #42 from connorsmith256/fix/remove-old-jwt-section ([`bfe1ce7`](https://github.com/wasmCloud/wasmCloud/commit/bfe1ce7a9d68041adc2d2f5567e306bd639649b4))
    - Also remove old JWT section when removing custom section ([`2fb8360`](https://github.com/wasmCloud/wasmCloud/commit/2fb8360e5bdfadc85e4df00b13f31d68468b725f))
    - Merge pull request #41 from connorsmith256/feat/add-contract-id-to-provider-descriptions ([`3c0523e`](https://github.com/wasmCloud/wasmCloud/commit/3c0523e2fd4a076afc1065a5fba549a558694446))
    - Add contract_id to provider descriptions ([`a50a013`](https://github.com/wasmCloud/wasmCloud/commit/a50a0138e0714bb8a9ba963c57dbee01abd924cd))
    - Merge pull request #345 from thomastaylor312/lib/claims ([`b0e385d`](https://github.com/wasmCloud/wasmCloud/commit/b0e385d1d4198614ce19299f0d71531225d85a96))
    - Moves claims and registry code into wash lib ([`84b9539`](https://github.com/wasmCloud/wasmCloud/commit/84b95392993cbbc65da36bc8b872241cce32a63e))
    - Merge pull request #344 from thomastaylor312/lib/keys ([`08bbb0f`](https://github.com/wasmCloud/wasmCloud/commit/08bbb0f2b9693d1c53842e454c83129e8c7bdaa3))
    - Adds new keys module to wash-lib ([`a62b07b`](https://github.com/wasmCloud/wasmCloud/commit/a62b07b8ff321c400c6debefdb6199e273445490))
    - Merge pull request #342 from thomastaylor312/fix/all_the_clippys ([`1065eb4`](https://github.com/wasmCloud/wasmCloud/commit/1065eb4d03453bf8dd89e5329db132f93dc43e08))
    - Fixes a bunch of clippy lints ([`9d94dcc`](https://github.com/wasmCloud/wasmCloud/commit/9d94dccea42c486c95e9fa497c1d1e7cf7cd5a0b))
    - Merge pull request #339 from thomastaylor312/lib/context ([`10f9c1b`](https://github.com/wasmCloud/wasmCloud/commit/10f9c1bb06e0b413c4c5fd579f015e32dae86f69))
    - Fixes issue with creating initial context ([`92f448e`](https://github.com/wasmCloud/wasmCloud/commit/92f448e69fdaa415ab6fa2fdfd3dce638ac2572d))
    - Creates new context library ([`52ef5b6`](https://github.com/wasmCloud/wasmCloud/commit/52ef5b6b1b6b01bc5e7a2c8fe3cbb2a08d4ad864))
    - Merge pull request #337 from thomastaylor312/feat/wash-lib ([`06cea91`](https://github.com/wasmCloud/wasmCloud/commit/06cea91e6541583a46ab306ad871e4a7781274cf))
    - Adds drain command to wash lib ([`b1bf6b1`](https://github.com/wasmCloud/wasmCloud/commit/b1bf6b1ac7851dc09e6757d7c2bde4558ec48098))
    - Merge pull request #317 from ricochet/chore/clap-v4 ([`c6ab554`](https://github.com/wasmCloud/wasmCloud/commit/c6ab554fc18de4525a6a90e8b94559f704e5c0b3))
    - Update clap to v4 ([`22d374a`](https://github.com/wasmCloud/wasmCloud/commit/22d374a1d750c0803a52bd93bb057018576e804d))
    - Merge pull request #37 from connorsmith256/feat/add-registry-type ([`313101a`](https://github.com/wasmCloud/wasmCloud/commit/313101a8f53f932497f95bcbbf35b430f3cce37a))
    - Add registryType as required field to RegistryCredential ([`0290f29`](https://github.com/wasmCloud/wasmCloud/commit/0290f29c5e4737ab8321649132125bf5ec806adc))
    - Merge pull request #36 from connorsmith256/fix/missing-provider-annotations ([`9ad0a91`](https://github.com/wasmCloud/wasmCloud/commit/9ad0a914b1ccf03ae5b8851ede34a039fc28d643))
    - Add annotations to ProviderDescription ([`93ec57c`](https://github.com/wasmCloud/wasmCloud/commit/93ec57c0d39b20f66c35391ab2c7d6c7499c272a))
    - Merge pull request #30 from wasmCloud/flushy-control-interface ([`2a02be3`](https://github.com/wasmCloud/wasmCloud/commit/2a02be3eccd7b7d093d413cceeab60e7392fc439))
    - Add flush after publish ([`f000788`](https://github.com/wasmCloud/wasmCloud/commit/f000788d590ef4082d756d3875a6e7e86977d3df))
    - Merge pull request #29 from wasmCloud/feat/traces-on-wasmbus-ctl ([`7a4bcdd`](https://github.com/wasmCloud/wasmCloud/commit/7a4bcddb45eea68d90dd5553a8609ebf9942582c))
    - Fix instrumentation of async call ([`140623d`](https://github.com/wasmCloud/wasmCloud/commit/140623d0c1d92f7f4460bf4a677bb5755de98ce3))
    - Fix format ([`36eeb97`](https://github.com/wasmCloud/wasmCloud/commit/36eeb9798262422f7d4fe6756ae70a780b7b4a42))
    - Add tracing to wasmcloud-control-interface ([`0c1637f`](https://github.com/wasmCloud/wasmCloud/commit/0c1637f491a319097113b1ff95f1e9c57e61ac19))
    - Merge pull request #285 from wasmCloud/feat/exp_wadm ([`a013433`](https://github.com/wasmCloud/wasmCloud/commit/a013433de1b32cba53aea0ebd238e2e9cebfa06b))
    - More cleaning ([`fc1223d`](https://github.com/wasmCloud/wasmCloud/commit/fc1223d9aea528dab326605bc29902ea1c4db3bc))
    - Refactoring and cleaning ([`9e08871`](https://github.com/wasmCloud/wasmCloud/commit/9e0887168db4b0538c8f0836da1d6a746b47a2f2))
    - Initial implementation of experimental wadm client ([`ea6054b`](https://github.com/wasmCloud/wasmCloud/commit/ea6054bbf1299436310ae4c9a9fabed397497785))
    - Merge pull request #25 from wasmCloud/fix/reg_put ([`25ae938`](https://github.com/wasmCloud/wasmCloud/commit/25ae9383c351d33d978b213affb4ea22e0777909))
    - Changing registry put to a publish operation ([`f31060d`](https://github.com/wasmCloud/wasmCloud/commit/f31060d8374c1944fa15e66b6e3c1e3249c4988d))
    - Merge pull request #250 from emattiza/fix/split-options-with-base64-padding ([`e1e3a5c`](https://github.com/wasmCloud/wasmCloud/commit/e1e3a5c74ed2931c8287ea1b96ec620fc096e356))
    - Use split_once and pattern match ([`50e1ac7`](https://github.com/wasmCloud/wasmCloud/commit/50e1ac7ec8ddf2a2316c0246217c21640042c79c))
    - Resolving failing test with bounded splitn ([`c8c1801`](https://github.com/wasmCloud/wasmCloud/commit/c8c18011502e65fb6a1da3c97b790d41d03b04c2))
    - Added failing test for bug ([`b198b94`](https://github.com/wasmCloud/wasmCloud/commit/b198b945a7b598cc25d00e49a940203a77370b8b))
    - Merge pull request #238 from mattwilkinsonn/scale-actor ([`3212bbd`](https://github.com/wasmCloud/wasmCloud/commit/3212bbdf46426c51066337dba682d2d6f8cb43f9))
    - Add docs on annotations ([`9d97f35`](https://github.com/wasmCloud/wasmCloud/commit/9d97f3517289c7d507a596de4fd310a152f228fd))
    - Create scale actor functionality ([`f7eed47`](https://github.com/wasmCloud/wasmCloud/commit/f7eed47aeb56174829c20318deb7ed5d6f9ef0bb))
    - Prototyping scale actor arguments ([`8c4a291`](https://github.com/wasmCloud/wasmCloud/commit/8c4a291a6d5922f46f20dd4ccbf8cc5f905f5460))
    - Merge pull request #3 from wasmCloud/async-compression ([`6a64252`](https://github.com/wasmCloud/wasmCloud/commit/6a64252a786cf369168073eebdde029b66e2c3c6))
    - Better function names ([`5ee2276`](https://github.com/wasmCloud/wasmCloud/commit/5ee22761429a448be2bcbda307e0c879c5fc95e5))
    - Reworked loading provider archive for memory efficiency @oftaylor ([`9cfc2ba`](https://github.com/wasmCloud/wasmCloud/commit/9cfc2ba66b0084d3b69f779bc67354e846b05d68))
    - Merge pull request #21 from wasmCloud/wasmbus-rpc-07 ([`ddf28d7`](https://github.com/wasmCloud/wasmCloud/commit/ddf28d724503b2860f9605fc1213fd495513933b))
    - Merge pull request #2 from wasmCloud/wascap-0.8 ([`1829d34`](https://github.com/wasmCloud/wasmCloud/commit/1829d34df8985cc8b975b0687bccea6741f6a82d))
    - Bump dependencies, bump crate to 0.11.0 ([`1c3a52b`](https://github.com/wasmCloud/wasmCloud/commit/1c3a52b86312831b32e66d1b833541cc16f21141))
    - Update wascap; clippy; bump to 0.5.0 ([`d7d392b`](https://github.com/wasmCloud/wasmCloud/commit/d7d392b1b103e63b5c51057cec80fdcdaadd0711))
    - Merge pull request #37 from wasmCloud/upgrade-deps-remove-chrono ([`5fb69af`](https://github.com/wasmCloud/wasmCloud/commit/5fb69afe2d88f7a225b737edc2aa36505cd9becc))
    - Simplified expressions ([`e945bda`](https://github.com/wasmCloud/wasmCloud/commit/e945bda7836787f721ac3fb1128a0ae6bc302e45))
    - Clippy and rustfmt ([`4362467`](https://github.com/wasmCloud/wasmCloud/commit/4362467ee3bc8bc2ed4a7a3d7e58e64c511b9d31))
    - Fix CVE by removing chrono; upgrade nats dependency to 0.2; bump crate to 0.8.0 ([`769f825`](https://github.com/wasmCloud/wasmCloud/commit/769f825180864fcd1b3306fefca19b9b4e098fc7))
    - Handle fast return when there are no subscribers and nats returns empty data ([`52e1313`](https://github.com/wasmCloud/wasmCloud/commit/52e1313281312b9d860135715ccd2477556e0ddc))
    - Merge pull request #19 from wasmCloud/control-interface-async ([`9e0bf16`](https://github.com/wasmCloud/wasmCloud/commit/9e0bf16b62266c637b0f33034d0c7e0eea7d4f1e))
    - Rename wasmbus_rpc variants so test use wasmbus_rpc ([`e728e09`](https://github.com/wasmCloud/wasmCloud/commit/e728e09fbfa2e82d70dc4c3cd74b32b6edc55df2))
    - Move from asynk to aflowt; add auction_timeout to instance data ([`1664f5a`](https://github.com/wasmCloud/wasmCloud/commit/1664f5a407cf88b329fc8e3d35c600815e204b76))
    - Merge pull request #18 from wasmCloud/fix/scale-actor-id ([`42afcef`](https://github.com/wasmCloud/wasmCloud/commit/42afcefaacb5e81fa0340c078f7638142fb42c04))
    - Added actor ID to scale actor command ([`fccfcf2`](https://github.com/wasmCloud/wasmCloud/commit/fccfcf296931aa8ede0c4cb10428298c0f50aa83))
    - Merge pull request #17 from wasmCloud/add-scale-actor ([`b3ba6a3`](https://github.com/wasmCloud/wasmCloud/commit/b3ba6a398dc0f7d0b2792779ef606b85e61b31f5))
    - Add ScaleActorCommand ([`1f1000a`](https://github.com/wasmCloud/wasmCloud/commit/1f1000a15132b75c6f935032e184fb7e96560cce))
    - Merge pull request #225 from protochron/unix_permissions_keys ([`296e3b9`](https://github.com/wasmCloud/wasmCloud/commit/296e3b90a68cc4ed1fe5101ebf4d36d16e5420e3))
    - Set permissions on generated keys to be user-only on Unix systems ([`0b7853f`](https://github.com/wasmCloud/wasmCloud/commit/0b7853f921d5e2d4afdf487cf40970afab48e0f8))
    - Initial commit, move from wasmcloud/wasmcloud ([`5f6ae1a`](https://github.com/wasmCloud/wasmCloud/commit/5f6ae1a446264ae0d7a4ca2932f42c49d7e23d6b))
    - Fmt ([`79672ef`](https://github.com/wasmCloud/wasmCloud/commit/79672ef59f63dbca2fe27da2966b2c0866deb884))
    - Merge pull request #15 from wasmCloud/actor-count ([`fbea891`](https://github.com/wasmCloud/wasmCloud/commit/fbea8912995930518c7147f72e5febc5792da6f3))
    - Add count to startActor and stopActor commands; bump crate to 0.7.0 ([`90eff3d`](https://github.com/wasmCloud/wasmCloud/commit/90eff3dfa6208cd07f1c7a91166844af275dac28))
    - Merge pull request #207 from thomastaylor312/ref/better_link_error ([`867b4e9`](https://github.com/wasmCloud/wasmCloud/commit/867b4e9c968e7bc705d3679f7094112d22bee12a))
    - Return better errors when provider id is given ([`1f2ad93`](https://github.com/wasmCloud/wasmCloud/commit/1f2ad935620548acc4f2a51a4956056fa99e2e93))
    - Merge pull request #197 from thomastaylor312/feat/stop_host ([`2958c02`](https://github.com/wasmCloud/wasmCloud/commit/2958c024a09dc24229044543dcf43692dd4a8ace))
    - Better success message ([`f8e2d5b`](https://github.com/wasmCloud/wasmCloud/commit/f8e2d5bd88078f74587588659dcb284e4d9747a6))
    - Add stop host ([`a867a07`](https://github.com/wasmCloud/wasmCloud/commit/a867a07ca60f1391fbfea56d9ad246a71792652f))
    - Merge pull request #185 from thomastaylor312/chore/bump_patch ([`7200cee`](https://github.com/wasmCloud/wasmCloud/commit/7200cee8117571fc1c2be9bf6b6da3be6c9e9b7a))
    - Bumps version to 0.6.6 ([`315c808`](https://github.com/wasmCloud/wasmCloud/commit/315c808777c4800dbbd52efacf7bf36b2b245f5a))
    - Merge pull request #184 from thomastaylor312/fix/moar_windows_pathz ([`9684740`](https://github.com/wasmCloud/wasmCloud/commit/9684740a3dbb8b428aacef3fd7565bf0826e390c))
    - Fix home directory lookup for keys ([`5c2388e`](https://github.com/wasmCloud/wasmCloud/commit/5c2388e7e068ec4d5ffbd0d33cdaab554864fea2))
    - Merge pull request #183 from thomastaylor312/fix/windows_paths ([`6c80611`](https://github.com/wasmCloud/wasmCloud/commit/6c8061127c6fe31dcd9d6dadf7d8a898d9aac296))
    - Fixes windows path handling ([`b15b5b9`](https://github.com/wasmCloud/wasmCloud/commit/b15b5b9be9fac5f70635ebe137c789ddbf84ac8f))
    - Merge pull request #175 from thomastaylor312/fix/help_text ([`73e9372`](https://github.com/wasmCloud/wasmCloud/commit/73e93720cf749eb13ab8eefeb8ff5c25adeb9f5f))
    - Removes incorrect information from help text ([`4fc074d`](https://github.com/wasmCloud/wasmCloud/commit/4fc074d62ddba070adcfbace293d33a8c56d50a1))
    - Merge pull request #169 from wasmCloud/gen-command ([`ba4c7ce`](https://github.com/wasmCloud/wasmCloud/commit/ba4c7cec5a50825802905d813244a2bfc8081896))
    - Improve comments (used for auto-generated help) and improve formatting of template info message ([`e56a9e4`](https://github.com/wasmCloud/wasmCloud/commit/e56a9e45c1362fc23fe6b1aacf420397843f47eb))
    - Bump crate to 0.6.1; upgrade dependencies ([`15c1e81`](https://github.com/wasmCloud/wasmCloud/commit/15c1e81485fb54dcb28c00a00fcc4ee47131b92d))
    - Add 'gen' command for triggering code generation; fixes wash #163 ([`57d0be4`](https://github.com/wasmCloud/wasmCloud/commit/57d0be4e1403c979f357ee4ed621dcaada97c1a0))
    - Merge pull request #161 from wasmCloud/generate-new-project ([`972fa70`](https://github.com/wasmCloud/wasmCloud/commit/972fa702616927db87c354971705ab6a51a4dcea))
    - Address PR feedback ([`ea2253e`](https://github.com/wasmCloud/wasmCloud/commit/ea2253eebcc180cfbd2507aaff56fe364eef87ac))
    - Argh - off by one space! re-ran rustfmt ([`111619d`](https://github.com/wasmCloud/wasmCloud/commit/111619d32bee229de5d6501b5c0fb6a543fed686))
    - Cache -> caches ([`e528d4a`](https://github.com/wasmCloud/wasmCloud/commit/e528d4a10eb01aa742c8fa90d4503a04cea15333))
    - Add lint and validate cli functions; rustfmt ([`5a88eab`](https://github.com/wasmCloud/wasmCloud/commit/5a88eabc86eb6d8c6375f763db2cc736bdc1cbe9))
    - Add new project generation capability ([`5afd53d`](https://github.com/wasmCloud/wasmCloud/commit/5afd53d6ae02b14a62f4ac72eb4a53baa15d6ca9))
    - Merge pull request #156 from wasmCloud/invoke-actor-tests ([`23e7cf9`](https://github.com/wasmCloud/wasmCloud/commit/23e7cf9469d3a738776ec842fe1834b8174b8584))
    - Fix json formatting of msgpack ([`e023df2`](https://github.com/wasmCloud/wasmCloud/commit/e023df2d2a2243a6b0e9ed7a2ee3e512fbd953e1))
    - Add save param to test case ([`a5862f6`](https://github.com/wasmCloud/wasmCloud/commit/a5862f637993bcf86b635318980c6a4b9580c6da))
    - Merge branch 'main' into invoke-actor-tests ([`5360920`](https://github.com/wasmCloud/wasmCloud/commit/5360920e71fcd717fc56dea5ddea2f3143b0682f))
    - Add '--save' to save binary output; don't call to_utf8_lossy before doing json deserialize ([`ad77e56`](https://github.com/wasmCloud/wasmCloud/commit/ad77e56d36e6da1e336b8dd4ac703e0b3e893153))
    - Merge pull request #157 from wasmCloud/feature/apply_manifest ([`d5275e0`](https://github.com/wasmCloud/wasmCloud/commit/d5275e08e5a677bc3dd97652b43a32aa531f140c))
    - Fixing clippy warning ([`2e18541`](https://github.com/wasmCloud/wasmCloud/commit/2e185413b4a5ec18c9bb4dad6bc051766ad912e6))
    - Removing unnecessary comment block ([`7525d7b`](https://github.com/wasmCloud/wasmCloud/commit/7525d7b912facc9182e76e13a593f29f53233cbf))
    - Support for application of manifest files ([`ab9a75b`](https://github.com/wasmCloud/wasmCloud/commit/ab9a75b9930bb51e895f6ef8469cbc8281acdfa9))
    - Add some flags to call actor so it can call a test actor ([`2d60924`](https://github.com/wasmCloud/wasmCloud/commit/2d609242c43f1dfe8372983d6995d8170908d6d3))
</details>

<csr-unknown>
 add wash dev command Adds a new experimental wash capture commandThis one is very experimental, so I didn’t even add it to the toplevel help text, but it is all manually tested and good to go Adds wash spy command with experimental flag support add deprecation warnings for changed CLI commands flatten multiple commands into wash get flatten wash reg push/pull into wash push/pull flatten wash ctl stop into wash stop flatten wash ctl start into wash start flatten wash ctl link into wash link implement builtin capabilities via WIT enable WASMCLOUD_ALLOW_FILE_LOAD by default avoid capability checking by defaultThe host does not have enough context to do so reliably support running actors in binary introduce Actor abstractionThis also includes a call method on Actor as well as the lower-levelmodules and components, which lets the consumer of the library bypassthe instantiation step provide HostHandlerBuilder introduce Runtime-wide instance config introduce Runtime-wide handler include claims as capability handler parameter introduce a Handler traitUntie actor module from capability handling for better separation ofconcerns, which has nice side effects of: add initial host implementationThe initial host implementation: Moves claims and registry code into wash libSorry for the big PR here, but due to a bunch of codependent code,I had to move a bunch of stuff at once. There are two main threadsto this PR. First, I noticed that the claims code is all CLI specific,but it is likely that anyone building a CLI will not want to rewrite thatagain. If you are doing this purely in code, you can just use thewascap library. To make this work, I started added the CLI specific stuffto the cli module of wash lib. There will probably be other things weadd to it as we finish this refactorSecond, this moves the reusable registry bits into its own module, whichis super handy even for those not doing a CLI as it avoids directinteraction with the lower level OCI crates Adds new keys module to wash-libPlease note that this introduces one small breaking change to outputthat removes the .nk suffix from the list of keys. However, there isbackward compatibility for providing <key_name>.nk to wash keys getso it will still function as it did previously. This change wasspecifically made because the key name is more important than the suffix.If desired, I can back out that change, but it seemed to make more senseto make it less like a wash-specific ls of a directory Adds drain command to wash libThis also starts the process of creating a config module that I’llcontinue to update as I push forward the other PRs. Please note thatthis is the first of many PRs. I plan on doing each command as a separatePR instead of a mega PR Implement wait for ctl commands warning if contract IDs look like an nkey fetch from wasmcloud cache on inspect commands Add stop hostThis updates a few libraries so we could add the stop host command. I alsodid a small cleanup of some clippy lints<csr-unknown/>

## v0.78.0-rc10 (2023-09-21)

## v0.18.2 (2021-05-14)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release over the course of 10 calendar days.
 - 11 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 4 unique issues were worked on: [#134](https://github.com/wasmCloud/wasmCloud/issues/134), [#135](https://github.com/wasmCloud/wasmCloud/issues/135), [#140](https://github.com/wasmCloud/wasmCloud/issues/140), [#196](https://github.com/wasmCloud/wasmCloud/issues/196)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#134](https://github.com/wasmCloud/wasmCloud/issues/134)**
    - Wash watch ([`086492d`](https://github.com/wasmCloud/wasmCloud/commit/086492d0bae3ec6f635ed7c53c99f74255bacf6d))
 * **[#135](https://github.com/wasmCloud/wasmCloud/issues/135)**
    - Bump wasmcloud-host and control-interface ([`165316d`](https://github.com/wasmCloud/wasmCloud/commit/165316d720b053dca0ac07151903e5d3991a6701))
 * **[#140](https://github.com/wasmCloud/wasmCloud/issues/140)**
    - Fix bug in wash drain ([`83e82ad`](https://github.com/wasmCloud/wasmCloud/commit/83e82ad3ebbf655098ac2545f3ee62a637987d2e))
 * **[#196](https://github.com/wasmCloud/wasmCloud/issues/196)**
    - Allowed host to launch without nats when manifest is present ([`9a155aa`](https://github.com/wasmCloud/wasmCloud/commit/9a155aac06ada8ef86cbb2d5cb723d42134d0b2b))
</details>

## v0.18.1 (2021-05-03)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 6 commits contributed to the release over the course of 17 calendar days.
 - 17 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 6 unique issues were worked on: [#107](https://github.com/wasmCloud/wasmCloud/issues/107), [#120](https://github.com/wasmCloud/wasmCloud/issues/120), [#122](https://github.com/wasmCloud/wasmCloud/issues/122), [#123](https://github.com/wasmCloud/wasmCloud/issues/123), [#124](https://github.com/wasmCloud/wasmCloud/issues/124), [#129](https://github.com/wasmCloud/wasmCloud/issues/129)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#107](https://github.com/wasmCloud/wasmCloud/issues/107)**
    - No lattice repl ([`e99f9dc`](https://github.com/wasmCloud/wasmCloud/commit/e99f9dc73428a2f3ad459f377683e7233b8b256f))
 * **[#120](https://github.com/wasmCloud/wasmCloud/issues/120)**
    - A second breakfast first pass to add repl focus cues to the wash repl ([`48b01cb`](https://github.com/wasmCloud/wasmCloud/commit/48b01cb6182802a2bb2faa01165181cd01b2bab7))
 * **[#122](https://github.com/wasmCloud/wasmCloud/issues/122)**
    - Allow starting signed local actors in embedded REPL host ([`8914a65`](https://github.com/wasmCloud/wasmCloud/commit/8914a6581603d129dd9e7fec6e281a9638381a1a))
 * **[#123](https://github.com/wasmCloud/wasmCloud/issues/123)**
    - Manifest support ([`904b945`](https://github.com/wasmCloud/wasmCloud/commit/904b9458b0c441abb8a6e2c78a83873f63730a86))
 * **[#124](https://github.com/wasmCloud/wasmCloud/issues/124)**
    - Fixed #103 ([`6f705f9`](https://github.com/wasmCloud/wasmCloud/commit/6f705f9871bd1b3ce4ad482bcebc696751fed07a))
 * **[#129](https://github.com/wasmCloud/wasmCloud/issues/129)**
    - Improved cursor position logic ([`a97dfdf`](https://github.com/wasmCloud/wasmCloud/commit/a97dfdf494868c3f64855114bb95d393d8fd5952))
</details>

## v0.18.0 (2021-04-16)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 2 commits contributed to the release over the course of 2 calendar days.
 - 2 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 2 unique issues were worked on: [#152](https://github.com/wasmCloud/wasmCloud/issues/152), [#161](https://github.com/wasmCloud/wasmCloud/issues/161)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#152](https://github.com/wasmCloud/wasmCloud/issues/152)**
    - Implemented feature flags for wasmcloud binary ([`b832a3d`](https://github.com/wasmCloud/wasmCloud/commit/b832a3de343b0bf760d88ab9359b412c63d22636))
 * **[#161](https://github.com/wasmCloud/wasmCloud/issues/161)**
    - Adding CLI option for namespace prefix ([`ce1e35d`](https://github.com/wasmCloud/wasmCloud/commit/ce1e35d3105342cfe27849c6fb22856f83da504d))
</details>

## v0.17.0 (2021-04-13)

## v0.16.1 (2021-04-09)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 2 commits contributed to the release over the course of 10 calendar days.
 - 13 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 2 unique issues were worked on: [#113](https://github.com/wasmCloud/wasmCloud/issues/113), [#115](https://github.com/wasmCloud/wasmCloud/issues/115)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#113](https://github.com/wasmCloud/wasmCloud/issues/113)**
    - Fix help text for reg command ([`338c9a4`](https://github.com/wasmCloud/wasmCloud/commit/338c9a4897094958617a4f1c3167983daba83d43))
 * **[#115](https://github.com/wasmCloud/wasmCloud/issues/115)**
    - Properly generate tokens for providers ([`a97f84d`](https://github.com/wasmCloud/wasmCloud/commit/a97f84d2e5f09eae4cf57b58ab78022ab8101c15))
</details>

## v0.16.0 (2021-03-26)

<csr-id-68e15f2a2caa6031cdad617b906383b06cb197a5/>

### Other

 - <csr-id-68e15f2a2caa6031cdad617b906383b06cb197a5/> don't panic on invalid URL in claims inspect

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release over the course of 3 calendar days.
 - 3 days passed between releases.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: [#111](https://github.com/wasmCloud/wasmCloud/issues/111)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#111](https://github.com/wasmCloud/wasmCloud/issues/111)**
    - Don't panic on invalid URL in claims inspect ([`68e15f2`](https://github.com/wasmCloud/wasmCloud/commit/68e15f2a2caa6031cdad617b906383b06cb197a5))
</details>

## v0.15.5 (2021-03-23)

## v0.15.4 (2021-03-22)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release over the course of 3 calendar days.
 - 13 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: [#126](https://github.com/wasmCloud/wasmCloud/issues/126)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#126](https://github.com/wasmCloud/wasmCloud/issues/126)**
    - Add --label to wasmcloud ([`49a8ddb`](https://github.com/wasmCloud/wasmCloud/commit/49a8ddb7b95ee475422158118543d8418e57665b))
</details>

## v0.15.3 (2021-03-09)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 6 commits contributed to the release over the course of 13 calendar days.
 - 19 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 6 unique issues were worked on: [#100](https://github.com/wasmCloud/wasmCloud/issues/100), [#102](https://github.com/wasmCloud/wasmCloud/issues/102), [#111](https://github.com/wasmCloud/wasmCloud/issues/111), [#95](https://github.com/wasmCloud/wasmCloud/issues/95), [#96](https://github.com/wasmCloud/wasmCloud/issues/96), [#99](https://github.com/wasmCloud/wasmCloud/issues/99)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#100](https://github.com/wasmCloud/wasmCloud/issues/100)**
    - These files were corrected based on my interpretation of the requested task. ([`bf03fc6`](https://github.com/wasmCloud/wasmCloud/commit/bf03fc6b0ab342709ff349713eac4a47ffcce438))
 * **[#102](https://github.com/wasmCloud/wasmCloud/issues/102)**
    - Call Alias ([`8f7a273`](https://github.com/wasmCloud/wasmCloud/commit/8f7a2738a915f75b0f197bb260e508e8d44851ce))
 * **[#111](https://github.com/wasmCloud/wasmCloud/issues/111)**
    - Adding support for manifest loading to the wasmcloud binary ([`0cea03a`](https://github.com/wasmCloud/wasmCloud/commit/0cea03a4aa5f9824f9ef2e9e39c590813dfc41ab))
 * **[#95](https://github.com/wasmCloud/wasmCloud/issues/95)**
    - Allow tui logger to scroll ([`a23675e`](https://github.com/wasmCloud/wasmCloud/commit/a23675e3ad3f6948b00d8d8ced1acf94f18581a2))
 * **[#96](https://github.com/wasmCloud/wasmCloud/issues/96)**
    - Integration tests ([`c2889f5`](https://github.com/wasmCloud/wasmCloud/commit/c2889f5fcb3b235f44e905f12a8fb6b23b9f7713))
 * **[#99](https://github.com/wasmCloud/wasmCloud/issues/99)**
    - Update up.rs ([`cf8fb35`](https://github.com/wasmCloud/wasmCloud/commit/cf8fb35a081256b93cb9fc19172fc94efd02cf48))
</details>

## v0.15.0 (2021-02-17)

<csr-id-e54d40cf9d3f60d0ee7b6473b34f7fb653dcb503/>

### Other

 - <csr-id-e54d40cf9d3f60d0ee7b6473b34f7fb653dcb503/> More resilient and reliable streaming
   * Handle out of order and duplicate upload chunks.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 155 commits contributed to the release over the course of 607 calendar days.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 38 unique issues were worked on: [#1](https://github.com/wasmCloud/wasmCloud/issues/1), [#10](https://github.com/wasmCloud/wasmCloud/issues/10), [#11](https://github.com/wasmCloud/wasmCloud/issues/11), [#13](https://github.com/wasmCloud/wasmCloud/issues/13), [#14](https://github.com/wasmCloud/wasmCloud/issues/14), [#16](https://github.com/wasmCloud/wasmCloud/issues/16), [#19](https://github.com/wasmCloud/wasmCloud/issues/19), [#2](https://github.com/wasmCloud/wasmCloud/issues/2), [#20](https://github.com/wasmCloud/wasmCloud/issues/20), [#23](https://github.com/wasmCloud/wasmCloud/issues/23), [#25](https://github.com/wasmCloud/wasmCloud/issues/25), [#27](https://github.com/wasmCloud/wasmCloud/issues/27), [#3](https://github.com/wasmCloud/wasmCloud/issues/3), [#30](https://github.com/wasmCloud/wasmCloud/issues/30), [#32](https://github.com/wasmCloud/wasmCloud/issues/32), [#33](https://github.com/wasmCloud/wasmCloud/issues/33), [#39](https://github.com/wasmCloud/wasmCloud/issues/39), [#4](https://github.com/wasmCloud/wasmCloud/issues/4), [#40](https://github.com/wasmCloud/wasmCloud/issues/40), [#42](https://github.com/wasmCloud/wasmCloud/issues/42), [#43](https://github.com/wasmCloud/wasmCloud/issues/43), [#45](https://github.com/wasmCloud/wasmCloud/issues/45), [#48](https://github.com/wasmCloud/wasmCloud/issues/48), [#5](https://github.com/wasmCloud/wasmCloud/issues/5), [#6](https://github.com/wasmCloud/wasmCloud/issues/6), [#63](https://github.com/wasmCloud/wasmCloud/issues/63), [#64](https://github.com/wasmCloud/wasmCloud/issues/64), [#65](https://github.com/wasmCloud/wasmCloud/issues/65), [#66](https://github.com/wasmCloud/wasmCloud/issues/66), [#69](https://github.com/wasmCloud/wasmCloud/issues/69), [#7](https://github.com/wasmCloud/wasmCloud/issues/7), [#72](https://github.com/wasmCloud/wasmCloud/issues/72), [#78](https://github.com/wasmCloud/wasmCloud/issues/78), [#8](https://github.com/wasmCloud/wasmCloud/issues/8), [#80](https://github.com/wasmCloud/wasmCloud/issues/80), [#83](https://github.com/wasmCloud/wasmCloud/issues/83), [#87](https://github.com/wasmCloud/wasmCloud/issues/87), [#9](https://github.com/wasmCloud/wasmCloud/issues/9)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#1](https://github.com/wasmCloud/wasmCloud/issues/1)**
    - Wcc CLI design proposal ([`d841528`](https://github.com/wasmCloud/wasmCloud/commit/d841528635b50ee6809dba1ec9a6775671dabcfa))
    - Upgrading codec, exposing provider descriptor ([`7e73367`](https://github.com/wasmCloud/wasmCloud/commit/7e733676f00f22a454eea5b709ba0ff72e2fce40))
    - New codec / named bindings support ([`9ad8b2c`](https://github.com/wasmCloud/wasmCloud/commit/9ad8b2cabe647ca5411a87914e4e73d85d69b073))
    - Named bindings, new codec ([`c3cf760`](https://github.com/wasmCloud/wasmCloud/commit/c3cf76022150221eb4f1627c3155a4b6a6a53ea0))
    - Create wascc:logging ([`0edbc49`](https://github.com/wasmCloud/wasmCloud/commit/0edbc4903426f309001bd2812fbdefad79d26573))
 * **[#10](https://github.com/wasmCloud/wasmCloud/issues/10)**
    - Added pre-commit and format workflows ([`2bb9002`](https://github.com/wasmCloud/wasmCloud/commit/2bb9002ee84939dc48c7c1b1f727992348e03801))
    - Propagate response headers from codec Response to actix HttpResponse ([`b7a86f3`](https://github.com/wasmCloud/wasmCloud/commit/b7a86f3755dadeaa7245dca169750397ba02dd52))
 * **[#11](https://github.com/wasmCloud/wasmCloud/issues/11)**
    - Claims autogeneration, optional autogeneration ([`a974cb1`](https://github.com/wasmCloud/wasmCloud/commit/a974cb171593b948230376ec0c8e0b86195f2c26))
    - Verifying that this works against the latest wascc codec ([`66491c5`](https://github.com/wasmCloud/wasmCloud/commit/66491c50c02f2f96c9464ad38c09787185c8bb86))
 * **[#13](https://github.com/wasmCloud/wasmCloud/issues/13)**
    - Add wascc:logging as a well-known capability type ([`a2d6683`](https://github.com/wasmCloud/wasmCloud/commit/a2d6683c27cb90f6c4b2f102bed29661432e974d))
 * **[#14](https://github.com/wasmCloud/wasmCloud/issues/14)**
    - First test of implementation of lattice RPC over nats ([`2551c87`](https://github.com/wasmCloud/wasmCloud/commit/2551c87f56177c8733526a01d050e3e2ac00caf7))
 * **[#16](https://github.com/wasmCloud/wasmCloud/issues/16)**
    - Safety/WIP checkin. Partially done implementing control interface ([`308bbe4`](https://github.com/wasmCloud/wasmCloud/commit/308bbe4605f2f21359a7eed8518a8fe844a4f149))
    - Check jwt segment count ([`46b94d1`](https://github.com/wasmCloud/wasmCloud/commit/46b94d1d31b316243d90cfb989d93f86dd93d77e))
 * **[#19](https://github.com/wasmCloud/wasmCloud/issues/19)**
    - Ensure a token contains both issuer and subject claims ([`641ed82`](https://github.com/wasmCloud/wasmCloud/commit/641ed82ed3312bacaa4f5ac13052abb735a13b37))
 * **[#2](https://github.com/wasmCloud/wasmCloud/issues/2)**
    - Additional CLI design ([`5938f7f`](https://github.com/wasmCloud/wasmCloud/commit/5938f7f9d86f669b288be0ecfe1308ccff00e939))
    - Upgrading to latest codec, providing descriptor ([`65d6907`](https://github.com/wasmCloud/wasmCloud/commit/65d69073a1c8d32f4f5f48c84489e00984320ffc))
    - Upgrading to latest codec, descriptors, adding missing OP_REMOVE_ACTOR handler ([`2c2dd5d`](https://github.com/wasmCloud/wasmCloud/commit/2c2dd5d9427159613c465fa31e9c111adf5af3fa))
    - Prepping for crates publication ([`e5b3de4`](https://github.com/wasmCloud/wasmCloud/commit/e5b3de40f080a3d5bfadda17a44e87526d6b4b87))
 * **[#20](https://github.com/wasmCloud/wasmCloud/issues/20)**
    - Initial Implementation of OCI push and pull commands ([`36f5cab`](https://github.com/wasmCloud/wasmCloud/commit/36f5cab9be20b7b52dfab40064c4cb8e958e44bb))
    - Exit non-zero when there is an error ([`7692635`](https://github.com/wasmCloud/wasmCloud/commit/7692635fc8cade3bb61bcce4e41076efabf7e461))
 * **[#23](https://github.com/wasmCloud/wasmCloud/issues/23)**
    - Adding the new Invocation claim type ([`18c9dff`](https://github.com/wasmCloud/wasmCloud/commit/18c9dffd5db7980a9442d87ee1dd9eababc6b5fa))
 * **[#25](https://github.com/wasmCloud/wasmCloud/issues/25)**
    - Initial wash up commit with command parsing ([`eb3877a`](https://github.com/wasmCloud/wasmCloud/commit/eb3877ac8a1637cd05d7f6b7fd3492b62325e0aa))
    - Adding support for new claims type - capability provider ([`44824fa`](https://github.com/wasmCloud/wasmCloud/commit/44824faf25255f1ba7e27e9fed1b55ab23ffd46f))
 * **[#27](https://github.com/wasmCloud/wasmCloud/issues/27)**
    - Initial ctl implementation ([`9e65f7a`](https://github.com/wasmCloud/wasmCloud/commit/9e65f7a1204f1e93adeb1d955a92a167898dfec9))
 * **[#3](https://github.com/wasmCloud/wasmCloud/issues/3)**
    - Migrate functionality from existing CLIs ([`f151be5`](https://github.com/wasmCloud/wasmCloud/commit/f151be55f514b9b7f802d58aafe6e6d47052dcdf))
    - Initial commit - refactor to newest codec ([`619f832`](https://github.com/wasmCloud/wasmCloud/commit/619f832af5b352480c1a50fb7ad270a7154b7b8c))
    - More resilient and reliable streaming ([`e54d40c`](https://github.com/wasmCloud/wasmCloud/commit/e54d40cf9d3f60d0ee7b6473b34f7fb653dcb503))
 * **[#30](https://github.com/wasmCloud/wasmCloud/issues/30)**
    - Claims and par improvements ([`3d979c4`](https://github.com/wasmCloud/wasmCloud/commit/3d979c472117eeba6a3fa7eff0e12132bf8fa93b))
 * **[#32](https://github.com/wasmCloud/wasmCloud/issues/32)**
    - Initial implementation of actor update functionality ([`2390d79`](https://github.com/wasmCloud/wasmCloud/commit/2390d79063f3df58a8f358010839462a3f8e77a1))
 * **[#33](https://github.com/wasmCloud/wasmCloud/issues/33)**
    - Add call alias field to actor metadata ([`3806f2f`](https://github.com/wasmCloud/wasmCloud/commit/3806f2f97a30f540ae1dd4e9f7e597afbc3a7e68))
    - WasmCloud REPL ([`0ea3647`](https://github.com/wasmCloud/wasmCloud/commit/0ea3647fabc757b78d0f9a564f993f94b19a4462))
 * **[#39](https://github.com/wasmCloud/wasmCloud/issues/39)**
    - Global JSON output ([`93c6948`](https://github.com/wasmCloud/wasmCloud/commit/93c69483e36ca81c6281f70d3e52cfec5f8f4d84))
 * **[#4](https://github.com/wasmCloud/wasmCloud/issues/4)**
    - Wash par implementation ([`c8cb66f`](https://github.com/wasmCloud/wasmCloud/commit/c8cb66f2dab5ccb83eab6ab0e87df2c562644e70))
    - Updated to codec version 0.8 ([`921a157`](https://github.com/wasmCloud/wasmCloud/commit/921a157434e7c4ec9c68653cca79979f80302710))
    - Add initial provider implementation ([`748221d`](https://github.com/wasmCloud/wasmCloud/commit/748221d41e21bf1f28b8d33dbe4a727f69abc867))
    - Closes #2 - Sanitizes container and blob IDs so that they can't be used to view content outside the FS provider's root path ([`754413d`](https://github.com/wasmCloud/wasmCloud/commit/754413d84eeb86c7cc0ba4c7f0ad06cb65cded6a))
    - Updated dependencies ([`4a0b018`](https://github.com/wasmCloud/wasmCloud/commit/4a0b018bf7f0175c38cc5745c990d0a0a41a2538))
 * **[#40](https://github.com/wasmCloud/wasmCloud/issues/40)**
    - Fixed path bug ([`e440f17`](https://github.com/wasmCloud/wasmCloud/commit/e440f17f29e423f724d49fc654116650bdbf0e01))
 * **[#42](https://github.com/wasmCloud/wasmCloud/issues/42)**
    - Integrated structopt into wasmcloud main ([`374b923`](https://github.com/wasmCloud/wasmCloud/commit/374b923f03570fd93c23632a110ff3c25ff193f0))
 * **[#43](https://github.com/wasmCloud/wasmCloud/issues/43)**
    - Implemented remote claims inspect ([`b695c3a`](https://github.com/wasmCloud/wasmCloud/commit/b695c3a796e20d17a3f54c3d73def7969247835d))
 * **[#45](https://github.com/wasmCloud/wasmCloud/issues/45)**
    - Print 'rev' and 'ver' if available when running 'wash par inspect' ([`82838d7`](https://github.com/wasmCloud/wasmCloud/commit/82838d79196717f72e1eba8b148eac52651a1496))
 * **[#48](https://github.com/wasmCloud/wasmCloud/issues/48)**
    - Support claims related commands in repl ([`8fd3a83`](https://github.com/wasmCloud/wasmCloud/commit/8fd3a83608820deedf928093cb18b1f0b3130961))
 * **[#5](https://github.com/wasmCloud/wasmCloud/issues/5)**
    - Upgrading to new (lack of) codec ([`4d56da6`](https://github.com/wasmCloud/wasmCloud/commit/4d56da6df3bed12961b9d11530e4dc76dd6d7f69))
    - Upgrading to new codec, exposing provider descriptor ([`8172ee0`](https://github.com/wasmCloud/wasmCloud/commit/8172ee0b239d453391bbeff2fb300201fe8c8139))
    - Upgrading codec, exposing capability descriptor ([`0284a5e`](https://github.com/wasmCloud/wasmCloud/commit/0284a5e91da6e3fc1f62fc61704616084809f97f))
    - Named bindings / new codec support ([`9f5621f`](https://github.com/wasmCloud/wasmCloud/commit/9f5621f70237ab5dc4745a36ff40fc3704fb8f68))
    - Named bindings / new codec support ([`ffe0d6b`](https://github.com/wasmCloud/wasmCloud/commit/ffe0d6bdbfd2469056ea23edc900ef146f059706))
 * **[#6](https://github.com/wasmCloud/wasmCloud/issues/6)**
    - Included public key in par inspect command, bump version ([`482b1f7`](https://github.com/wasmCloud/wasmCloud/commit/482b1f7435701718940b1fa221bdc179ffff707b))
    - Updated to codec 0.8 ([`7ec1e3d`](https://github.com/wasmCloud/wasmCloud/commit/7ec1e3d0aa8eccd5a2e1ba531d88cf160575c3bf))
    - Upgrading to the latest codec, provider descriptors ([`7d538bb`](https://github.com/wasmCloud/wasmCloud/commit/7d538bb732f56e4e5dc1303c89cbce5515e72f7c))
    - Upgrading codec, exposing capability descriptor ([`40b8c60`](https://github.com/wasmCloud/wasmCloud/commit/40b8c60693fb82630f17abeed58b727a865f0b35))
 * **[#63](https://github.com/wasmCloud/wasmCloud/issues/63)**
    - Fixing small issues in 0.2.0 milestone ([`91aa374`](https://github.com/wasmCloud/wasmCloud/commit/91aa3749460e18c979fa3e1289b095ac49c308bd))
 * **[#64](https://github.com/wasmCloud/wasmCloud/issues/64)**
    - Added support for WASH_RPC_HOST, _TIMEOUT, and _PORT in wash ctl ([`0b9cb11`](https://github.com/wasmCloud/wasmCloud/commit/0b9cb114dc562edc8adb899b4a632286480d2eb6))
 * **[#65](https://github.com/wasmCloud/wasmCloud/issues/65)**
    - Display refactor ([`06cee76`](https://github.com/wasmCloud/wasmCloud/commit/06cee76f81637e408570c32153e56371f5b359cf))
 * **[#66](https://github.com/wasmCloud/wasmCloud/issues/66)**
    - Provided option for allowed list of insecure registries ([`659682f`](https://github.com/wasmCloud/wasmCloud/commit/659682fc61faf9f87cc823c8144dcb66e47736fa))
 * **[#69](https://github.com/wasmCloud/wasmCloud/issues/69)**
    - Dependency bump, kvcounter script ([`e00d00d`](https://github.com/wasmCloud/wasmCloud/commit/e00d00de7eb1a11fc1f3101c1316f8aafe560189))
 * **[#7](https://github.com/wasmCloud/wasmCloud/issues/7)**
    - Update dependencies ([`abf2cb5`](https://github.com/wasmCloud/wasmCloud/commit/abf2cb59848e81a086cf6fc5128fe617e0c7df07))
    - Change package name and update header ([`c8b2bc5`](https://github.com/wasmCloud/wasmCloud/commit/c8b2bc5a9c4a59a68fd7f9a928250efc20a3acb3))
    - Achievement Unlocked: commit from an airplane ✈️ ([`31509db`](https://github.com/wasmCloud/wasmCloud/commit/31509db6a42eabb9664fd2f9a9e33b08cffc8632))
 * **[#72](https://github.com/wasmCloud/wasmCloud/issues/72)**
    - Non blocking commands ([`40d475d`](https://github.com/wasmCloud/wasmCloud/commit/40d475dc653af190cec71ba2563b8b38ac1ceb46))
 * **[#78](https://github.com/wasmCloud/wasmCloud/issues/78)**
    - Drain ([`8316151`](https://github.com/wasmCloud/wasmCloud/commit/8316151dfbf813b559fb063d47609b597727e27b))
 * **[#8](https://github.com/wasmCloud/wasmCloud/issues/8)**
    - Updating to codec 0.8 ([`91256e8`](https://github.com/wasmCloud/wasmCloud/commit/91256e80ecc7d746b2d776ec69d38ecc777ef41c))
    - Add timeout and redirect configuration ([`3c80776`](https://github.com/wasmCloud/wasmCloud/commit/3c80776c4c79dc1d16b31b4d475230557c809713))
 * **[#80](https://github.com/wasmCloud/wasmCloud/issues/80)**
    - Wash UX improvements, writing up a README ([`1c4ff69`](https://github.com/wasmCloud/wasmCloud/commit/1c4ff69c13cd44b28621193442023842b22835c8))
 * **[#83](https://github.com/wasmCloud/wasmCloud/issues/83)**
    - Bump all non-breaking changes with wascap ([`0043ec8`](https://github.com/wasmCloud/wasmCloud/commit/0043ec8dc6b292425da9ede8af35dba8425b248a))
 * **[#87](https://github.com/wasmCloud/wasmCloud/issues/87)**
    - 0.2.0 release ([`8377e8e`](https://github.com/wasmCloud/wasmCloud/commit/8377e8e2fead424f2542df0489e1c70d231ff384))
 * **[#9](https://github.com/wasmCloud/wasmCloud/issues/9)**
    - Support PAR compression and interacting with compressed parJEEzy files ([`6cabebb`](https://github.com/wasmCloud/wasmCloud/commit/6cabebb5e6850c247ee9436dd4a4cba8523a2253))
    - Update codec to 0.7 ([`6be9a27`](https://github.com/wasmCloud/wasmCloud/commit/6be9a27adf31fb357ef26c042db45e8087481271))
    - Closes #8 - support for multiple additional signing keys for operators and accounts. ([`6be5ccd`](https://github.com/wasmCloud/wasmCloud/commit/6be5ccde30c2444df58bab38d8a5d9dc58e754b4))
 * **Uncategorized**
    - Added localhost to insecure allow list ([`fd55ab1`](https://github.com/wasmCloud/wasmCloud/commit/fd55ab18f40ca8d2422e0a13db101a0bfdea8be7))
    - Merge remote-tracking branch 'upstream/main' into release_gh ([`c02921b`](https://github.com/wasmCloud/wasmCloud/commit/c02921bf17cf14767894449df08b886aab2e9eed))
    - Add env var input for par subject/issuer ([`96b5b05`](https://github.com/wasmCloud/wasmCloud/commit/96b5b05b6433be1f5bc09103a5b380e26e1907d9))
    - Merge remote-tracking branch 'fs/master' into main ([`64844b3`](https://github.com/wasmCloud/wasmCloud/commit/64844b399bd2a3f1c8151cbe574835298f1797d1))
    - Merge remote-tracking branch 'logging/master' into main ([`288a91f`](https://github.com/wasmCloud/wasmCloud/commit/288a91fcc0ca89ac196d825657f384ebee0cd198))
    - Merge remote-tracking branch 'http-client/master' into main ([`e64c64b`](https://github.com/wasmCloud/wasmCloud/commit/e64c64bd80cd4ce30f41067de8e98663f1943ffe))
    - Merge remote-tracking branch 'telnet/master' into main ([`0f2651c`](https://github.com/wasmCloud/wasmCloud/commit/0f2651ce5e38a1df0b1f3a654769c4407f5a79d9))
    - Merge remote-tracking branch 's3/master' into main ([`84391b3`](https://github.com/wasmCloud/wasmCloud/commit/84391b35d4a817ccbd412160af73ee90167d9c2d))
    - Merge remote-tracking branch 'redis-streams/master' into main ([`d30b88e`](https://github.com/wasmCloud/wasmCloud/commit/d30b88eae75ec91bda50d6e765618fc43a2a05f0))
    - Merge remote-tracking branch 'redis/master' into main ([`328936b`](https://github.com/wasmCloud/wasmCloud/commit/328936bfe2614bc5b0536a11675f35db1b6c1734))
    - Merge remote-tracking branch 'nats/master' into main ([`b4fb958`](https://github.com/wasmCloud/wasmCloud/commit/b4fb95897cebd4a4863888a80fa0f4129ff07d7c))
    - Safety checkin ([`0530d18`](https://github.com/wasmCloud/wasmCloud/commit/0530d188d73c2640ea303b9d030db5295aa94a21))
    - Yet another safety checkin ([`52f212e`](https://github.com/wasmCloud/wasmCloud/commit/52f212e0dbbec4ebe781d37441fa1afda98c84dc))
    - Another safety check-in ([`d2629bf`](https://github.com/wasmCloud/wasmCloud/commit/d2629bff92e1dddc5b31c6a932ab72c080e6c821))
    - Safety checkin ([`d4d10ac`](https://github.com/wasmCloud/wasmCloud/commit/d4d10acfa3bfb370dbab2fc9b953ece08dfa7ccf))
    - Basic bus functionality ([`9f6be9d`](https://github.com/wasmCloud/wasmCloud/commit/9f6be9d739dddff50db4316e06666da899145e3f))
    - Pre-weekend safety checkin of WIP ([`2fddaab`](https://github.com/wasmCloud/wasmCloud/commit/2fddaab30e0db93c046fdd0e3a99ccff999d5c6a))
    - More scaffolding ([`19ba023`](https://github.com/wasmCloud/wasmCloud/commit/19ba0238cbd80eb918fe7994c876f81cad045539))
    - Initial commit of scaffolding and project structure ([`8aba2db`](https://github.com/wasmCloud/wasmCloud/commit/8aba2db60c115254909e533cbf5c6f53247922a6))
    - Initial stubbed cli commit, ascii art included ([`3fafdf2`](https://github.com/wasmCloud/wasmCloud/commit/3fafdf2bfacaf5ca23b6941b5ed3de6319664cba))
    - S3 provider now honors the context field from stream download request ([`fdbc028`](https://github.com/wasmCloud/wasmCloud/commit/fdbc0286f0ed42d66f8d002290383fc79e66477e))
    - FS provider honors the context field from stream download request ([`1462517`](https://github.com/wasmCloud/wasmCloud/commit/1462517ac54003db5d8607c5811096ed7aa8a679))
    - Closes #24 - Exports Error and Result as WascapError and WascapResult to avoid polluting consumer namespaces ([`41eb9f2`](https://github.com/wasmCloud/wasmCloud/commit/41eb9f223b143a69f808d8ae1e63124d5b9ebbef))
    - Initial commit ([`d4e75a5`](https://github.com/wasmCloud/wasmCloud/commit/d4e75a559feceea20e1a95a80092a41d876275ba))
    - Prep for publish to crates ([`ca5c373`](https://github.com/wasmCloud/wasmCloud/commit/ca5c3730f72ab6522498b010232d020a7483d314))
    - Fixing create_container bug that did not return the appropriate data to the consumer ([`88cbc87`](https://github.com/wasmCloud/wasmCloud/commit/88cbc87843278c8f9c5a4228b507da22b4f848fc))
    - Enabling static linking and prep for cargo publish ([`e0f24a4`](https://github.com/wasmCloud/wasmCloud/commit/e0f24a4a596ae9287f8a8b0961831aa290a84ee5))
    - Support for static linking and prep for crates publication ([`fd7178e`](https://github.com/wasmCloud/wasmCloud/commit/fd7178e4f37e5e9a06f8cdf2669b5cfe414b25ac))
    - Updating to latest codec, enabling static embedding in host ([`ec670e5`](https://github.com/wasmCloud/wasmCloud/commit/ec670e51cd0a1210223f250d1635cf85e3a2f17d))
    - Fixing incorrect badge ([`ddb1838`](https://github.com/wasmCloud/wasmCloud/commit/ddb18383abc75e7a9ff65f5fe0a1c3bce38a49ef))
    - Merge fix ([`89cc2ba`](https://github.com/wasmCloud/wasmCloud/commit/89cc2ba883a3f6ca0ab7a192004b9b5fb38921a6))
    - Merge fix ([`7227b1f`](https://github.com/wasmCloud/wasmCloud/commit/7227b1f377735c606a012ccb2ba9ed73b0872411))
    - Fixing merge ([`cece99f`](https://github.com/wasmCloud/wasmCloud/commit/cece99f41d03ef95863b4e2f0c88c42d0b2bc78a))
    - Prepping for crates publish ([`45aa4fe`](https://github.com/wasmCloud/wasmCloud/commit/45aa4fe00227ddd027ea3f6a40757311406629a9))
    - Prepping for crates publication ([`63a7d53`](https://github.com/wasmCloud/wasmCloud/commit/63a7d53dacabd4f15c06975844eac1a3ebe930f8))
    - Prepping for crates publication ([`c3dfc17`](https://github.com/wasmCloud/wasmCloud/commit/c3dfc175452097dab03da5189c632d919c009200))
    - Prepping for crates upload ([`552926a`](https://github.com/wasmCloud/wasmCloud/commit/552926ad5596d1fb1eecc87ba61d4143d2179f94))
    - Prepping for crates publication ([`faee3fe`](https://github.com/wasmCloud/wasmCloud/commit/faee3fe0a673c77bcdfd2a65b254686598fc471a))
    - Removing superfluous println in test ([`c5f683f`](https://github.com/wasmCloud/wasmCloud/commit/c5f683fb7fe0ce24f6d2aa98bc937e3e9dab2ff5))
    - Fixing chunk slicing to send the right chunks ([`2e9c9c8`](https://github.com/wasmCloud/wasmCloud/commit/2e9c9c8ac5d8ccb0c93a6a701c844130a9e6ca72))
    - Removing printlns ([`e5b2236`](https://github.com/wasmCloud/wasmCloud/commit/e5b22363ff43c026d5746d65badc4bec8a694f70))
    - TESTS PASS. W00T. ([`12951b1`](https://github.com/wasmCloud/wasmCloud/commit/12951b157e1e9ba8f9978209feb2307aa7be620b))
    - I have no idea what I'm doing ([`43de624`](https://github.com/wasmCloud/wasmCloud/commit/43de624b95f0f5e2201021233f5ad2035077c253))
    - Moving runtime creation to struct ([`f3da107`](https://github.com/wasmCloud/wasmCloud/commit/f3da1071b8d7c1410d369f82493dc29158bff479))
    - Tokio-pure version, using tokio spawn instead of thread spawn. Background tasks cancel and fail ([`abacc1d`](https://github.com/wasmCloud/wasmCloud/commit/abacc1d9a646669ca5cb577479788a546e7e2cfe))
    - Getting closer to figuring out the async. It's still horrible: ([`f544b67`](https://github.com/wasmCloud/wasmCloud/commit/f544b6799fc0cc83d630bfbdc96c632583e198a1))
    - Initial commit. Do not use ([`5418087`](https://github.com/wasmCloud/wasmCloud/commit/5418087b00b25154a63cf7328975c5c4a7efcff1))
    - Capability display from CLI now properly distinguishes between actors and cap providers ([`54057e1`](https://github.com/wasmCloud/wasmCloud/commit/54057e1c951b2e1b55e6dc43c79519b126e85d61))
    - Upgrading to latest codec ([`4f55c21`](https://github.com/wasmCloud/wasmCloud/commit/4f55c2161fa5504eaf4f518b17a30e4f0fd1f23a))
    - Upgrading to the latest codec ([`a636d40`](https://github.com/wasmCloud/wasmCloud/commit/a636d40d9538c4764853cce344095e66b033f460))
    - Upgrading to the latest codec ([`2ddb94b`](https://github.com/wasmCloud/wasmCloud/commit/2ddb94b05b6afa0824d0d2c7d11755b96d3d6299))
    - Upgrading to the latest version of the codec ([`ecb31a1`](https://github.com/wasmCloud/wasmCloud/commit/ecb31a1bb5431d679d03a95b66862d69907d79c7))
    - Upgrading to the latest version of the codec ([`adfe90a`](https://github.com/wasmCloud/wasmCloud/commit/adfe90ad829919f67fe669a8c00f8f7af58a001a))
    - Bug fix ([`f47c45d`](https://github.com/wasmCloud/wasmCloud/commit/f47c45dbd2f40188bb8472fe63c38242013bebc7))
    - Initial commit ([`74aac39`](https://github.com/wasmCloud/wasmCloud/commit/74aac39c5674aae39b3afa61d743b2e8a14e12b0))
    - Adding well-known capability IDs: blob store, extras, event streams ([`0ce50f5`](https://github.com/wasmCloud/wasmCloud/commit/0ce50f5738c06a10533742c7e9c991a1a06ede38))
    - Initial commit ([`5b4256b`](https://github.com/wasmCloud/wasmCloud/commit/5b4256b38f0e592acdf140a2dc7d275280249863))
    - Operator and Account names are now optional to make de-serialization of tokens safer ([`f95903d`](https://github.com/wasmCloud/wasmCloud/commit/f95903d2bcf2491a5bac22b47b21102c2406afb5))
    - Removing superfluous tracing ([`eb58ccc`](https://github.com/wasmCloud/wasmCloud/commit/eb58cccd97d2cede1452e77326d148fce46da10c))
    - Provide meaningful error when redis is accidentally not configured for an actor ([`4cda4c5`](https://github.com/wasmCloud/wasmCloud/commit/4cda4c53b284bb978948cef3dc4bf83dfdc1fbd0))
    - Tweak ([`317a8cd`](https://github.com/wasmCloud/wasmCloud/commit/317a8cd85676a293fa17a4a38102f68cfbd15c14))
    - What do we love? Dependency management. When do we love it? All the time. ([`7ac9e05`](https://github.com/wasmCloud/wasmCloud/commit/7ac9e05b68ef9b847f167d7b05f28db35ffc70ac))
    - HTTP server now avoids panicking when sent a request to de-configure a server that doesn't exist. ([`e67a6e3`](https://github.com/wasmCloud/wasmCloud/commit/e67a6e34c2e62a3d87e7401097a01fd3cee339c0))
    - Merge pull request #4 from bketelsen/add-rev-version ([`d1be595`](https://github.com/wasmCloud/wasmCloud/commit/d1be59518db474e88d0b7ca334b6c38cf238bacd))
    - Ownership problems solved in pairing ([`7528565`](https://github.com/wasmCloud/wasmCloud/commit/7528565aef5f929e1998d123ab045b3b8b142a60))
    - Still broken, know where ([`327681a`](https://github.com/wasmCloud/wasmCloud/commit/327681a7e92b465de165b8965631c5f73cd98326))
    - Most tests passing, still broken on hashing check ([`c074e70`](https://github.com/wasmCloud/wasmCloud/commit/c074e70d7ea67fca399ac82dd107074680703782))
    - First attempt at adding rev ([`59fc5e6`](https://github.com/wasmCloud/wasmCloud/commit/59fc5e6469f77ad6f6d1f62df6900088da02b23d))
    - Updated to avoid panic when env logger is initialized multiple times ([`8fbc375`](https://github.com/wasmCloud/wasmCloud/commit/8fbc375e661dc98ea43fdc7581af60fe6910d8a3))
    - Adding support for actor removal and preventing a panic from attempting to initialize a global logger twice ([`71a3014`](https://github.com/wasmCloud/wasmCloud/commit/71a30143a70579ed4747bf486e6a0c965992eed4))
    - Updating so multiple invocations of env logger init will not panic ([`c27cd19`](https://github.com/wasmCloud/wasmCloud/commit/c27cd19fee5f1a864a22ba4aacb008ab5637b505))
    - Now properly terminates HTTP server and frees up port when a configured actor is removed. ([`8d25c89`](https://github.com/wasmCloud/wasmCloud/commit/8d25c8939c527f9977217886f0950e90ff876fe0))
    - Updating to be aware of actor removal ([`81ff4b9`](https://github.com/wasmCloud/wasmCloud/commit/81ff4b9510e0d039b307c57ef2ab2b4343867b14))
    - Initial commit of support for actor removal. does not yet shut down HTTP server. ([`4fe13c1`](https://github.com/wasmCloud/wasmCloud/commit/4fe13c1766310b94e2d9b13a02273ec1153b8b0b))
    - Fixed bug preventing you from signing a previously signed module. Added the ability to indicate whether a module is also a provider ([`263c9b2`](https://github.com/wasmCloud/wasmCloud/commit/263c9b2fdddd93361c01116c2de3f9692fc0c45d))
    - Upgrading to new version of nkeys ([`b63f97d`](https://github.com/wasmCloud/wasmCloud/commit/b63f97d2a8e8306a6f7b0df6fe25b9bce9c9b7af))
    - Initial commit ([`9b6e749`](https://github.com/wasmCloud/wasmCloud/commit/9b6e749f2d18902186198bdb797b6d9af0de66cb))
    - Initial commit ([`074a545`](https://github.com/wasmCloud/wasmCloud/commit/074a5458fb890658252774465fc3caa89033eee4))
    - HTTP server now enforces rule that only the 'system' actor can submit configurations ([`7f2f802`](https://github.com/wasmCloud/wasmCloud/commit/7f2f802ba14493b94f02bad284f4c5727b140bea))
    - Initial commit ([`a595c82`](https://github.com/wasmCloud/wasmCloud/commit/a595c8299e8673fca9b893440d8c6375bfb8abd5))
    - Merge pull request #2 from wascc/wascc_upgrade ([`5f38b39`](https://github.com/wasmCloud/wasmCloud/commit/5f38b39460ec6f52bb2d0fe45765ecdbf75aef6d))
    - Updating rustdoc ([`9e4b243`](https://github.com/wasmCloud/wasmCloud/commit/9e4b243e05173e9f16b6fd6f741dc51d438faf5a))
    - Fixing some clippy warnings, changing the default capabilities to wascc namespace instead of wascap. ([`70ee277`](https://github.com/wasmCloud/wasmCloud/commit/70ee277f300de23b1646cdaa24a6bccfb5f86999))
    - Adding travis integration and badges ([`c7745ff`](https://github.com/wasmCloud/wasmCloud/commit/c7745ffc8658a1be854b155992ae80ef89bb079d))
    - Copyright headers ([`6ed993e`](https://github.com/wasmCloud/wasmCloud/commit/6ed993e832dfbe29a08084c71b339e1bb2df8aa9))
    - Initial commit ([`00ac25b`](https://github.com/wasmCloud/wasmCloud/commit/00ac25bdae99f169061af86cad3695cb42645ca7))
</details>


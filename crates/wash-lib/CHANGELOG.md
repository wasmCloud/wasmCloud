# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.29.0 (2024-10-23)

### Chore

 - <csr-id-343c0d7509d3e7ef88ec45798d16bca105831611/> remove unused claims/push_insecure
 - <csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/> bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, wasmcloud-host v0.21.0, wasmcloud-runtime v0.5.0, wasmcloud-test-util v0.13.0
 - <csr-id-81281b490d9df214e60281a05db35f781656d64f/> tests
 - <csr-id-e5b75f416b4f17d29d5939f25b211a466c842788/> remove async-nats v0.33
 - <csr-id-9df2bb1754fbffc36ed03a00098831eca49f3171/> standardize emoji usage

### New Features

 - <csr-id-1d1f53437558603d3698d6ff3be05e691449f06a/> add support for Rust wasm32-wasip2 target
 - <csr-id-1b70cfafed45553f62226d8b9644e96ef1e1e3ec/> update wadm to 0.18
 - <csr-id-c0520722f5b4543f702c0f13fa75630be8008d9c/> Adds support for wasmcloud named things for packages
   This also integrates the wkg.toml stuff directly into wasmcloud.toml
 - <csr-id-9dda559ad835fd12eb820942df5082b5b24a3dbb/> support Rust debug component builds
 - <csr-id-93f5bc247da89de1fe3fcb1f1ae7efd2af9a4e05/> add wit_path, path, and reg insecure to wasmcloud.toml
 - <csr-id-f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2/> Updates tests and examples to support the new wkg deps
   This updates all dependencies to have a wkg.lock but I didn't add to the
   gitignore for convenience. The deps are still committed in tree for backwards
   compatibility and they all use the new versioned logging. This looks
   really chunky bust is mostly dep updates/deletes
 - <csr-id-eccccb6b3a84b7d465fec64a33e2b13db4cc2b64/> add interface-driven overrides to dev config
 - <csr-id-7e3d2d06ec542293b8ed3c86734b5401b4e080cb/> Adds monkey patching for wasi-logging
   Yep, this is ugly. There is no way to sugar coat it. But this is probably
   the easiest way around the fact that we can't work with unversioned deps.
   Good news is we can remove it once we hit 2.0
 - <csr-id-9f5c7b92a876208e15f5cb808c3649f77fe3c2da/> add dev configuration
   This commit adds configuration to `wash-lib` that enables features in
   the implementation of `wash dev`, namely the configuration for a
   specified pre-existing WADM manifest along with config and secrets.
 - <csr-id-9a50dd6c0fac06a8c13cd6919d84510606442dc5/> Tinygo scheduler & garbage collection overrides
 - <csr-id-6f3d223d7527c98bbdffa0ad0de1be91cb3eb528/> add wash drain dev

### Bug Fixes

 - <csr-id-5be8a7a2f65dfa1cfe025fd9760e2fe958987980/> fix doc tests
 - <csr-id-79e5988f0cf098280a63bb34dd29b6204fa59293/> impl `Default` for `WashConnectionOptions`
 - <csr-id-cc1164cf500509f9b2e184e041b3fef18f4b2da0/> Update doctests to not fail when run
 - <csr-id-a9a3a67ee7e18d25c624baf7b8d0f9cf4fd75d50/> Store wadm.pid in the correct location
 - <csr-id-f838d6c9506caa82ab33779eef50197a3af5befb/> constantly failing tests
   This commit fixes some tests that were failing (i.e. `make test-all`)
   when trying to test wash.
 - <csr-id-eb5a6638fa0014de52b53d7daf4e9718dfb73cd0/> input usage for new dialoguer version
 - <csr-id-4f642d30945fcc74370a862c26fd5d75e104618e/> Make sure lock file is created in case of dep issue
 - <csr-id-0b7d9368e48507c4d511fbd1e39c1248fdf4b48f/> Addressing PR comments
 - <csr-id-1a07544c5f8959b4dcb2c7e4078984681ba72437/> differentiate config, support fast-reload

### Other

 - <csr-id-bff14fac85ce4673a1abe432e152067f506fb994/> correct tests with new functionality

### Refactor

 - <csr-id-da7b2cd26cdc4c929e45dc4aee4e5b092c3b26f9/> address PR nits and clippy warnings
 - <csr-id-30a7dacf19254c7e0e0762f5c6b007cfc27ad1f0/> remove raw structs, simplify deserialization
 - <csr-id-b3e79caf381e53172fb61cf4c7668816efd65b09/> change docker check to use testcontainers
 - <csr-id-a4a9e365095270bd97b59634699b3790b990bc73/> move dev from common config to external
 - <csr-id-d23f3ef01c8fdb980462aca3f7f37237e531bc4b/> return early if NATS bin path parent missing
 - <csr-id-25e7bb204023277d651fc3550d6a7c15a540c934/> update wash-lib for control-interface v2.2.0

### Test

 - <csr-id-de73278b2730f19e71d3d08996d00f205d9559cf/> add test for overriding interface

### New Features (BREAKING)

 - <csr-id-cf09c44d4d08bdb9039f51e95320bf2183e3454e/> wash build override build/wit/project dir
 - <csr-id-644bd35df2a24d1f12aba4d0613a3b667db9c70b/> Adds support for fetching dependencies automatically
   This uses the newly available wit commands from wasm-pkg-tools to fetch
   things from upstream registry. I have only updated select examples (that
   are less likely to be used while we finish the last bits of work) so we
   could test things.
   
   I also fixed the wash build tests so they could run properly in parallel
   (which hopefully makes them faster too). Once we have all wasi cloud wit
   in OCI, we can update all deps and roll to a new version of wash. This
   will involve updating git ignores to ignore the deps directory, but
   leaving the current deps intact for those on older versions of wash for
   at least the next version or two. Once we're past that we can remove the
   deps dirs
 - <csr-id-bcb98361f20f6ed449e1091bb4f1af1b3e13abbd/> align wasi target
   Remove references to preview.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 36 commits contributed to the release over the course of 20 calendar days.
 - 22 days passed between releases.
 - 36 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Add support for Rust wasm32-wasip2 target ([`1d1f534`](https://github.com/wasmCloud/wasmCloud/commit/1d1f53437558603d3698d6ff3be05e691449f06a))
    - Update wadm to 0.18 ([`1b70cfa`](https://github.com/wasmCloud/wasmCloud/commit/1b70cfafed45553f62226d8b9644e96ef1e1e3ec))
    - Fix doc tests ([`5be8a7a`](https://github.com/wasmCloud/wasmCloud/commit/5be8a7a2f65dfa1cfe025fd9760e2fe958987980))
    - Impl `Default` for `WashConnectionOptions` ([`79e5988`](https://github.com/wasmCloud/wasmCloud/commit/79e5988f0cf098280a63bb34dd29b6204fa59293))
    - Adds support for wasmcloud named things for packages ([`c052072`](https://github.com/wasmCloud/wasmCloud/commit/c0520722f5b4543f702c0f13fa75630be8008d9c))
    - Update doctests to not fail when run ([`cc1164c`](https://github.com/wasmCloud/wasmCloud/commit/cc1164cf500509f9b2e184e041b3fef18f4b2da0))
    - Store wadm.pid in the correct location ([`a9a3a67`](https://github.com/wasmCloud/wasmCloud/commit/a9a3a67ee7e18d25c624baf7b8d0f9cf4fd75d50))
    - Address PR nits and clippy warnings ([`da7b2cd`](https://github.com/wasmCloud/wasmCloud/commit/da7b2cd26cdc4c929e45dc4aee4e5b092c3b26f9))
    - Correct tests with new functionality ([`bff14fa`](https://github.com/wasmCloud/wasmCloud/commit/bff14fac85ce4673a1abe432e152067f506fb994))
    - Wash build override build/wit/project dir ([`cf09c44`](https://github.com/wasmCloud/wasmCloud/commit/cf09c44d4d08bdb9039f51e95320bf2183e3454e))
    - Support Rust debug component builds ([`9dda559`](https://github.com/wasmCloud/wasmCloud/commit/9dda559ad835fd12eb820942df5082b5b24a3dbb))
    - Add wit_path, path, and reg insecure to wasmcloud.toml ([`93f5bc2`](https://github.com/wasmCloud/wasmCloud/commit/93f5bc247da89de1fe3fcb1f1ae7efd2af9a4e05))
    - Remove unused claims/push_insecure ([`343c0d7`](https://github.com/wasmCloud/wasmCloud/commit/343c0d7509d3e7ef88ec45798d16bca105831611))
    - Remove raw structs, simplify deserialization ([`30a7dac`](https://github.com/wasmCloud/wasmCloud/commit/30a7dacf19254c7e0e0762f5c6b007cfc27ad1f0))
    - Updates tests and examples to support the new wkg deps ([`f0f3fd7`](https://github.com/wasmCloud/wasmCloud/commit/f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2))
    - Add test for overriding interface ([`de73278`](https://github.com/wasmCloud/wasmCloud/commit/de73278b2730f19e71d3d08996d00f205d9559cf))
    - Add interface-driven overrides to dev config ([`eccccb6`](https://github.com/wasmCloud/wasmCloud/commit/eccccb6b3a84b7d465fec64a33e2b13db4cc2b64))
    - Change docker check to use testcontainers ([`b3e79ca`](https://github.com/wasmCloud/wasmCloud/commit/b3e79caf381e53172fb61cf4c7668816efd65b09))
    - Constantly failing tests ([`f838d6c`](https://github.com/wasmCloud/wasmCloud/commit/f838d6c9506caa82ab33779eef50197a3af5befb))
    - Input usage for new dialoguer version ([`eb5a663`](https://github.com/wasmCloud/wasmCloud/commit/eb5a6638fa0014de52b53d7daf4e9718dfb73cd0))
    - Adds monkey patching for wasi-logging ([`7e3d2d0`](https://github.com/wasmCloud/wasmCloud/commit/7e3d2d06ec542293b8ed3c86734b5401b4e080cb))
    - Make sure lock file is created in case of dep issue ([`4f642d3`](https://github.com/wasmCloud/wasmCloud/commit/4f642d30945fcc74370a862c26fd5d75e104618e))
    - Adds support for fetching dependencies automatically ([`644bd35`](https://github.com/wasmCloud/wasmCloud/commit/644bd35df2a24d1f12aba4d0613a3b667db9c70b))
    - Move dev from common config to external ([`a4a9e36`](https://github.com/wasmCloud/wasmCloud/commit/a4a9e365095270bd97b59634699b3790b990bc73))
    - Add dev configuration ([`9f5c7b9`](https://github.com/wasmCloud/wasmCloud/commit/9f5c7b92a876208e15f5cb808c3649f77fe3c2da))
    - Bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates ([`44bf4c8`](https://github.com/wasmCloud/wasmCloud/commit/44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a))
    - Addressing PR comments ([`0b7d936`](https://github.com/wasmCloud/wasmCloud/commit/0b7d9368e48507c4d511fbd1e39c1248fdf4b48f))
    - Tests ([`81281b4`](https://github.com/wasmCloud/wasmCloud/commit/81281b490d9df214e60281a05db35f781656d64f))
    - Tinygo scheduler & garbage collection overrides ([`9a50dd6`](https://github.com/wasmCloud/wasmCloud/commit/9a50dd6c0fac06a8c13cd6919d84510606442dc5))
    - Return early if NATS bin path parent missing ([`d23f3ef`](https://github.com/wasmCloud/wasmCloud/commit/d23f3ef01c8fdb980462aca3f7f37237e531bc4b))
    - Update wash-lib for control-interface v2.2.0 ([`25e7bb2`](https://github.com/wasmCloud/wasmCloud/commit/25e7bb204023277d651fc3550d6a7c15a540c934))
    - Remove async-nats v0.33 ([`e5b75f4`](https://github.com/wasmCloud/wasmCloud/commit/e5b75f416b4f17d29d5939f25b211a466c842788))
    - Align wasi target ([`bcb9836`](https://github.com/wasmCloud/wasmCloud/commit/bcb98361f20f6ed449e1091bb4f1af1b3e13abbd))
    - Standardize emoji usage ([`9df2bb1`](https://github.com/wasmCloud/wasmCloud/commit/9df2bb1754fbffc36ed03a00098831eca49f3171))
    - Add wash drain dev ([`6f3d223`](https://github.com/wasmCloud/wasmCloud/commit/6f3d223d7527c98bbdffa0ad0de1be91cb3eb528))
    - Differentiate config, support fast-reload ([`1a07544`](https://github.com/wasmCloud/wasmCloud/commit/1a07544c5f8959b4dcb2c7e4078984681ba72437))
</details>

## v0.28.0 (2024-10-09)

<csr-id-81281b490d9df214e60281a05db35f781656d64f/>
<csr-id-e5b75f416b4f17d29d5939f25b211a466c842788/>
<csr-id-9df2bb1754fbffc36ed03a00098831eca49f3171/>
<csr-id-d23f3ef01c8fdb980462aca3f7f37237e531bc4b/>
<csr-id-25e7bb204023277d651fc3550d6a7c15a540c934/>

### Chore

 - <csr-id-81281b490d9df214e60281a05db35f781656d64f/> tests
 - <csr-id-e5b75f416b4f17d29d5939f25b211a466c842788/> remove async-nats v0.33
 - <csr-id-9df2bb1754fbffc36ed03a00098831eca49f3171/> standardize emoji usage

### New Features

 - <csr-id-9a50dd6c0fac06a8c13cd6919d84510606442dc5/> Tinygo scheduler & garbage collection overrides
 - <csr-id-6f3d223d7527c98bbdffa0ad0de1be91cb3eb528/> add wash drain dev

### Bug Fixes

 - <csr-id-0b7d9368e48507c4d511fbd1e39c1248fdf4b48f/> Addressing PR comments
 - <csr-id-1a07544c5f8959b4dcb2c7e4078984681ba72437/> differentiate config, support fast-reload

### Refactor

 - <csr-id-d23f3ef01c8fdb980462aca3f7f37237e531bc4b/> return early if NATS bin path parent missing
 - <csr-id-25e7bb204023277d651fc3550d6a7c15a540c934/> update wash-lib for control-interface v2.2.0

### New Features (BREAKING)

 - <csr-id-bcb98361f20f6ed449e1091bb4f1af1b3e13abbd/> align wasi target
   Remove references to preview.

## v0.27.0 (2024-09-30)

<csr-id-b86cd09511b1066055139b87b46151125f1ea323/>
<csr-id-8eb7008551a396249a9b7cab3836399adf26e291/>
<csr-id-2dafe48949b39d2818b2757ad0828b7897e0b8b9/>
<csr-id-c1aff12a27444dbb7024d3fe953963984cc7c60d/>
<csr-id-68034a31ae374e573cd0d6d93c495b2060959258/>
<csr-id-a7f28bd931015ce40649909e5ed4f12111cebecb/>
<csr-id-720dae8d7e30a755ca8ddcfc1609f388c3994855/>
<csr-id-eff19c6b7e54c3c5e9f30a018dbad7e8ec05e29f/>
<csr-id-d8a480bfba3769e56471d408f90d0aaf5a356a4a/>
<csr-id-0ba8332c7f531fcb189b134ac9a02a0c141e692c/>
<csr-id-88646e221462763a68a93ad52ec363f8dad0a451/>
<csr-id-69039793fe275c35ebf647d52f117c0bbf3bf675/>
<csr-id-5cf073e738a97f772af47fd4dff5c4075daa5698/>
<csr-id-71fc4b8f60a1f5f469912b712452f1c96a7744ef/>
<csr-id-3a29609a9d22e3a34da49cdfa049f89e8bd72bef/>

### Chore

 - <csr-id-b86cd09511b1066055139b87b46151125f1ea323/> fix spacing
 - <csr-id-8eb7008551a396249a9b7cab3836399adf26e291/> Removing wit-bindgen dependency
 - <csr-id-2dafe48949b39d2818b2757ad0828b7897e0b8b9/> Iterating on CI
 - <csr-id-c1aff12a27444dbb7024d3fe953963984cc7c60d/> Addressing tests
 - <csr-id-68034a31ae374e573cd0d6d93c495b2060959258/> Rollback examples
   They need to be on a different PR
 - <csr-id-a7f28bd931015ce40649909e5ed4f12111cebecb/> tinygo bindgen test
 - <csr-id-720dae8d7e30a755ca8ddcfc1609f388c3994855/> Calling 'go generate' instead of wit-bindgen directly
 - <csr-id-eff19c6b7e54c3c5e9f30a018dbad7e8ec05e29f/> wash Go wasip2 support
 - <csr-id-d8a480bfba3769e56471d408f90d0aaf5a356a4a/> Adopt predefined testcontainers
 - <csr-id-0ba8332c7f531fcb189b134ac9a02a0c141e692c/> update rust wasm32 targets
 - <csr-id-88646e221462763a68a93ad52ec363f8dad0a451/> update wasi rust target
 - <csr-id-69039793fe275c35ebf647d52f117c0bbf3bf675/> Replace dirs dependency with home

### New Features

 - <csr-id-d3001a14736cfbf5faa3353044e49af8cfb1f78e/> add more command output
 - <csr-id-ce1569cb6423f3bc42ff645e8c062287d8b3b78f/> Implemented Humantime duration input for --watch flags
   - Backwards compatibility with millisecond input still maintained
- Improved terminal handling while watching application lattice
- watch interval for 'app list' is now configurable with a default interval of 1000ms.
- Added Short flag of -w as an alternative to --watch

### Bug Fixes

 - <csr-id-474a1f48af92a49648167ebf975d3ee4f32685e0/> moar cargo clippy
 - <csr-id-37b0c0bc2b58670f4d4a56f5ab4527da7a16a454/> clippy

### Other

 - <csr-id-5cf073e738a97f772af47fd4dff5c4075daa5698/> wash-lib v0.27.0

### Test

 - <csr-id-71fc4b8f60a1f5f469912b712452f1c96a7744ef/> add test for wash app undeploy --all

### Chore (BREAKING)

 - <csr-id-3a29609a9d22e3a34da49cdfa049f89e8bd72bef/> use tokio Commands

### New Features (BREAKING)

 - <csr-id-31b21c2baae4dbf16434bca4ddd70938f769618f/> Stop returning default credentials from project config if none can be resolved

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 20 commits contributed to the release over the course of 11 calendar days.
 - 11 days passed between releases.
 - 20 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Wash-lib v0.27.0 ([`5cf073e`](https://github.com/wasmCloud/wasmCloud/commit/5cf073e738a97f772af47fd4dff5c4075daa5698))
    - Fix spacing ([`b86cd09`](https://github.com/wasmCloud/wasmCloud/commit/b86cd09511b1066055139b87b46151125f1ea323))
    - Use tokio Commands ([`3a29609`](https://github.com/wasmCloud/wasmCloud/commit/3a29609a9d22e3a34da49cdfa049f89e8bd72bef))
    - Add test for wash app undeploy --all ([`71fc4b8`](https://github.com/wasmCloud/wasmCloud/commit/71fc4b8f60a1f5f469912b712452f1c96a7744ef))
    - Add more command output ([`d3001a1`](https://github.com/wasmCloud/wasmCloud/commit/d3001a14736cfbf5faa3353044e49af8cfb1f78e))
    - Removing wit-bindgen dependency ([`8eb7008`](https://github.com/wasmCloud/wasmCloud/commit/8eb7008551a396249a9b7cab3836399adf26e291))
    - Iterating on CI ([`2dafe48`](https://github.com/wasmCloud/wasmCloud/commit/2dafe48949b39d2818b2757ad0828b7897e0b8b9))
    - Addressing tests ([`c1aff12`](https://github.com/wasmCloud/wasmCloud/commit/c1aff12a27444dbb7024d3fe953963984cc7c60d))
    - Rollback examples ([`68034a3`](https://github.com/wasmCloud/wasmCloud/commit/68034a31ae374e573cd0d6d93c495b2060959258))
    - Moar cargo clippy ([`474a1f4`](https://github.com/wasmCloud/wasmCloud/commit/474a1f48af92a49648167ebf975d3ee4f32685e0))
    - Tinygo bindgen test ([`a7f28bd`](https://github.com/wasmCloud/wasmCloud/commit/a7f28bd931015ce40649909e5ed4f12111cebecb))
    - Calling 'go generate' instead of wit-bindgen directly ([`720dae8`](https://github.com/wasmCloud/wasmCloud/commit/720dae8d7e30a755ca8ddcfc1609f388c3994855))
    - Clippy ([`37b0c0b`](https://github.com/wasmCloud/wasmCloud/commit/37b0c0bc2b58670f4d4a56f5ab4527da7a16a454))
    - Wash Go wasip2 support ([`eff19c6`](https://github.com/wasmCloud/wasmCloud/commit/eff19c6b7e54c3c5e9f30a018dbad7e8ec05e29f))
    - Adopt predefined testcontainers ([`d8a480b`](https://github.com/wasmCloud/wasmCloud/commit/d8a480bfba3769e56471d408f90d0aaf5a356a4a))
    - Update rust wasm32 targets ([`0ba8332`](https://github.com/wasmCloud/wasmCloud/commit/0ba8332c7f531fcb189b134ac9a02a0c141e692c))
    - Update wasi rust target ([`88646e2`](https://github.com/wasmCloud/wasmCloud/commit/88646e221462763a68a93ad52ec363f8dad0a451))
    - Implemented Humantime duration input for --watch flags ([`ce1569c`](https://github.com/wasmCloud/wasmCloud/commit/ce1569cb6423f3bc42ff645e8c062287d8b3b78f))
    - Stop returning default credentials from project config if none can be resolved ([`31b21c2`](https://github.com/wasmCloud/wasmCloud/commit/31b21c2baae4dbf16434bca4ddd70938f769618f))
    - Replace dirs dependency with home ([`6903979`](https://github.com/wasmCloud/wasmCloud/commit/69039793fe275c35ebf647d52f117c0bbf3bf675))
</details>

## v0.26.0 (2024-09-18)

<csr-id-fbd1dd10a7c92a40a69c21b2cbba21c07ae8e893/>
<csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/>
<csr-id-c65d9cab4cc8917eedcad1672812bafad0311ee0/>
<csr-id-d0593c02f472cbaa963fe19df258894888db3a6e/>
<csr-id-c7dfe65bdb7847d35abf1ad6dc187bda801dc945/>
<csr-id-14ae4b6b72c78dc39bbb2d613e44dfc58ff11e5a/>

### Chore

 - <csr-id-fbd1dd10a7c92a40a69c21b2cbba21c07ae8e893/> Switch to using oci feature

### Other

 - <csr-id-14ae4b6b72c78dc39bbb2d613e44dfc58ff11e5a/> wash-lib v0.26.0

### New Features

 - <csr-id-7738695b405d20261b92c730329387886d1ba04a/> add ability to check and validate OCI image references in WADM manifests

### Bug Fixes

 - <csr-id-5c47b8cc1aade1794e266a938d52655cf903fff7/> remove wasmcloud-host from deps

### Other

 - <csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/> bump wasmcloud-core v0.10.0, safety bump 5 crates
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-tracing v0.8.0, wasmcloud-provider-sdk v0.9.0, wash-cli v0.33.0, wash-lib v0.26.0
 - <csr-id-c65d9cab4cc8917eedcad1672812bafad0311ee0/> upgrade to 0.36

### Refactor (BREAKING)

 - <csr-id-d0593c02f472cbaa963fe19df258894888db3a6e/> break out stop_provider for reuse
 - <csr-id-c7dfe65bdb7847d35abf1ad6dc187bda801dc945/> support process groups for wadm & nats
   This commit updates the `wash-lib` methods used for starting WADM and
   NATS to support process groups.
   
   On Unix and Windows, process groups enable child processes to avoid
   automatically receiving processes sent to the parent process. This
   enables more control for situations in which the child processes
   should possibly outlive the parent, or should be controlled more
   directly by the parent process (ex. delaying signal passthrough until
   after processing, etc)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 8 commits contributed to the release over the course of 11 calendar days.
 - 13 days passed between releases.
 - 8 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Wash-lib v0.26.0 ([`14ae4b6`](https://github.com/wasmCloud/wasmCloud/commit/14ae4b6b72c78dc39bbb2d613e44dfc58ff11e5a))
    - Bump wasmcloud-core v0.10.0, safety bump 5 crates ([`1af6e05`](https://github.com/wasmCloud/wasmCloud/commit/1af6e05f1a47be4e62a4c21d1704aff2e09bef89))
    - Break out stop_provider for reuse ([`d0593c0`](https://github.com/wasmCloud/wasmCloud/commit/d0593c02f472cbaa963fe19df258894888db3a6e))
    - Switch to using oci feature ([`fbd1dd1`](https://github.com/wasmCloud/wasmCloud/commit/fbd1dd10a7c92a40a69c21b2cbba21c07ae8e893))
    - Remove wasmcloud-host from deps ([`5c47b8c`](https://github.com/wasmCloud/wasmCloud/commit/5c47b8cc1aade1794e266a938d52655cf903fff7))
    - Support process groups for wadm & nats ([`c7dfe65`](https://github.com/wasmCloud/wasmCloud/commit/c7dfe65bdb7847d35abf1ad6dc187bda801dc945))
    - Upgrade to 0.36 ([`c65d9ca`](https://github.com/wasmCloud/wasmCloud/commit/c65d9cab4cc8917eedcad1672812bafad0311ee0))
    - Add ability to check and validate OCI image references in WADM manifests ([`7738695`](https://github.com/wasmCloud/wasmCloud/commit/7738695b405d20261b92c730329387886d1ba04a))
</details>

## v0.25.1 (2024-09-05)

<csr-id-76f8a5819b7e0422343fc52087e55ef18ad98fdd/>
<csr-id-e0d4c09ba7c1176f76a994f32f4c1e3147a3e59b/>
<csr-id-ba636cd344433db8701f6312be85e3377ca8a22e/>
<csr-id-144ba4f4d6a457a7e29eab9203c88e6ee1e05d99/>
<csr-id-6b42f9a2282eab209a2f1f3e169bb66582aa6d62/>
<csr-id-1168926db9fd825a6d2e0f0dca3079fcc603ad5c/>
<csr-id-7448729a1927e4ea48738bbf153533dd60ba2ad1/>
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>
<csr-id-3fb79daf65f9f029ca0227cfdac7b504d7bd9c6c/>

### Chore

 - <csr-id-76f8a5819b7e0422343fc52087e55ef18ad98fdd/> enable gc feature in wasmtime
 - <csr-id-e0d4c09ba7c1176f76a994f32f4c1e3147a3e59b/> help styling to streamline cli markdown
 - <csr-id-ba636cd344433db8701f6312be85e3377ca8a22e/> update testcontainers to stable version
 - <csr-id-144ba4f4d6a457a7e29eab9203c88e6ee1e05d99/> update Wasmtime  and wasm-tools usage in wash
 - <csr-id-6b42f9a2282eab209a2f1f3e169bb66582aa6d62/> more explicit errors for missing binary

### New Features

 - <csr-id-d53c2d8114d55fbec958ed9cc63fe3d35e6e5dd0/> add utility function `TypeConfig::wit_world`
 - <csr-id-cee789f1b4a04076c38b40bf14cc39be46ad08fe/> Add --watch flag to view live changes in host inventory
 - <csr-id-d9491b364499f36880eaf32fc9765d5cf1fcb664/> ref parsing for components to match providers
   This commit improves the component reference parsing/resolution to be
   as advanced for components as it is for providers.
 - <csr-id-c3a5a6f63c05076baa1233fabc9c9345456e2169/> add wadm_pid_file() path fn

### Bug Fixes

 - <csr-id-fa945c6bcc094afda0babfc2255b38a25a129e1b/> wash dev on non-xkeys component IDs
   This commit fixes an issue wher `wash dev` assumed that component IDs
   had to be `ModuleId`s (i.e. nkeys).
   
   While in the past component IDs *were* nkeys, they are no longer
   required to be, and can be user-friendly names.
 - <csr-id-5efa281da43f2b6f4ae29d5ec8c90822b0bc27f5/> remove misleading creds error message
 - <csr-id-ea98e1ee0e42de3134bee5e62c6ee7522a71a105/> use resolved component ref for start
 - <csr-id-2cc1a364d0f37adcb87bec27799884edf2208e93/> decode body payload as string
 - <csr-id-4adb08ac26210537ff7bf6a87722d0b3a7248761/> add missing tokio features
 - <csr-id-1a4f81ddd6e344c20c09b2493dd02047c3d651ca/> remove double event wait for scale

### Other

 - <csr-id-1168926db9fd825a6d2e0f0dca3079fcc603ad5c/> v0.25.1
 - <csr-id-7448729a1927e4ea48738bbf153533dd60ba2ad1/> wash-lib v0.25.0, wash-cli v0.32.0
 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0

### Refactor

 - <csr-id-3fb79daf65f9f029ca0227cfdac7b504d7bd9c6c/> use upstream preview1 adapter crate

### New Features (BREAKING)

 - <csr-id-301043bb0f86d15e3afb93e410a3a40242c6317a/> display detailed app status

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 20 commits contributed to the release over the course of 31 calendar days.
 - 34 days passed between releases.
 - 20 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - V0.25.1 ([`1168926`](https://github.com/wasmCloud/wasmCloud/commit/1168926db9fd825a6d2e0f0dca3079fcc603ad5c))
    - Enable gc feature in wasmtime ([`76f8a58`](https://github.com/wasmCloud/wasmCloud/commit/76f8a5819b7e0422343fc52087e55ef18ad98fdd))
    - Add utility function `TypeConfig::wit_world` ([`d53c2d8`](https://github.com/wasmCloud/wasmCloud/commit/d53c2d8114d55fbec958ed9cc63fe3d35e6e5dd0))
    - Add --watch flag to view live changes in host inventory ([`cee789f`](https://github.com/wasmCloud/wasmCloud/commit/cee789f1b4a04076c38b40bf14cc39be46ad08fe))
    - Wash-lib v0.25.0, wash-cli v0.32.0 ([`7448729`](https://github.com/wasmCloud/wasmCloud/commit/7448729a1927e4ea48738bbf153533dd60ba2ad1))
    - Wash dev on non-xkeys component IDs ([`fa945c6`](https://github.com/wasmCloud/wasmCloud/commit/fa945c6bcc094afda0babfc2255b38a25a129e1b))
    - Remove misleading creds error message ([`5efa281`](https://github.com/wasmCloud/wasmCloud/commit/5efa281da43f2b6f4ae29d5ec8c90822b0bc27f5))
    - Help styling to streamline cli markdown ([`e0d4c09`](https://github.com/wasmCloud/wasmCloud/commit/e0d4c09ba7c1176f76a994f32f4c1e3147a3e59b))
    - Bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates ([`8403350`](https://github.com/wasmCloud/wasmCloud/commit/8403350432a2387d4a2bce9c096f002005ba54be))
    - Use resolved component ref for start ([`ea98e1e`](https://github.com/wasmCloud/wasmCloud/commit/ea98e1ee0e42de3134bee5e62c6ee7522a71a105))
    - Display detailed app status ([`301043b`](https://github.com/wasmCloud/wasmCloud/commit/301043bb0f86d15e3afb93e410a3a40242c6317a))
    - Ref parsing for components to match providers ([`d9491b3`](https://github.com/wasmCloud/wasmCloud/commit/d9491b364499f36880eaf32fc9765d5cf1fcb664))
    - Update testcontainers to stable version ([`ba636cd`](https://github.com/wasmCloud/wasmCloud/commit/ba636cd344433db8701f6312be85e3377ca8a22e))
    - Add wadm_pid_file() path fn ([`c3a5a6f`](https://github.com/wasmCloud/wasmCloud/commit/c3a5a6f63c05076baa1233fabc9c9345456e2169))
    - Decode body payload as string ([`2cc1a36`](https://github.com/wasmCloud/wasmCloud/commit/2cc1a364d0f37adcb87bec27799884edf2208e93))
    - Add missing tokio features ([`4adb08a`](https://github.com/wasmCloud/wasmCloud/commit/4adb08ac26210537ff7bf6a87722d0b3a7248761))
    - Use upstream preview1 adapter crate ([`3fb79da`](https://github.com/wasmCloud/wasmCloud/commit/3fb79daf65f9f029ca0227cfdac7b504d7bd9c6c))
    - Update Wasmtime  and wasm-tools usage in wash ([`144ba4f`](https://github.com/wasmCloud/wasmCloud/commit/144ba4f4d6a457a7e29eab9203c88e6ee1e05d99))
    - More explicit errors for missing binary ([`6b42f9a`](https://github.com/wasmCloud/wasmCloud/commit/6b42f9a2282eab209a2f1f3e169bb66582aa6d62))
    - Remove double event wait for scale ([`1a4f81d`](https://github.com/wasmCloud/wasmCloud/commit/1a4f81ddd6e344c20c09b2493dd02047c3d651ca))
</details>

## v0.25.0 (2024-08-29)

<csr-id-e0d4c09ba7c1176f76a994f32f4c1e3147a3e59b/>
<csr-id-ba636cd344433db8701f6312be85e3377ca8a22e/>
<csr-id-144ba4f4d6a457a7e29eab9203c88e6ee1e05d99/>
<csr-id-6b42f9a2282eab209a2f1f3e169bb66582aa6d62/>
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>
<csr-id-3fb79daf65f9f029ca0227cfdac7b504d7bd9c6c/>

### Chore

 - <csr-id-e0d4c09ba7c1176f76a994f32f4c1e3147a3e59b/> help styling to streamline cli markdown
 - <csr-id-ba636cd344433db8701f6312be85e3377ca8a22e/> update testcontainers to stable version
 - <csr-id-144ba4f4d6a457a7e29eab9203c88e6ee1e05d99/> update Wasmtime  and wasm-tools usage in wash
 - <csr-id-6b42f9a2282eab209a2f1f3e169bb66582aa6d62/> more explicit errors for missing binary

### New Features

 - <csr-id-d9491b364499f36880eaf32fc9765d5cf1fcb664/> ref parsing for components to match providers
   This commit improves the component reference parsing/resolution to be
   as advanced for components as it is for providers.
 - <csr-id-c3a5a6f63c05076baa1233fabc9c9345456e2169/> add wadm_pid_file() path fn

### Bug Fixes

 - <csr-id-fa945c6bcc094afda0babfc2255b38a25a129e1b/> wash dev on non-xkeys component IDs
   This commit fixes an issue wher `wash dev` assumed that component IDs
   had to be `ModuleId`s (i.e. nkeys).
   
   While in the past component IDs *were* nkeys, they are no longer
   required to be, and can be user-friendly names.
 - <csr-id-5efa281da43f2b6f4ae29d5ec8c90822b0bc27f5/> remove misleading creds error message
 - <csr-id-ea98e1ee0e42de3134bee5e62c6ee7522a71a105/> use resolved component ref for start
 - <csr-id-2cc1a364d0f37adcb87bec27799884edf2208e93/> decode body payload as string
 - <csr-id-4adb08ac26210537ff7bf6a87722d0b3a7248761/> add missing tokio features
 - <csr-id-1a4f81ddd6e344c20c09b2493dd02047c3d651ca/> remove double event wait for scale

### Other

 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0

### Refactor

 - <csr-id-3fb79daf65f9f029ca0227cfdac7b504d7bd9c6c/> use upstream preview1 adapter crate

### New Features (BREAKING)

 - <csr-id-301043bb0f86d15e3afb93e410a3a40242c6317a/> display detailed app status

## v0.24.0 (2024-08-23)

<csr-id-ba636cd344433db8701f6312be85e3377ca8a22e/>
<csr-id-144ba4f4d6a457a7e29eab9203c88e6ee1e05d99/>
<csr-id-6b42f9a2282eab209a2f1f3e169bb66582aa6d62/>
<csr-id-3fb79daf65f9f029ca0227cfdac7b504d7bd9c6c/>

### Chore

 - <csr-id-ba636cd344433db8701f6312be85e3377ca8a22e/> update testcontainers to stable version
 - <csr-id-144ba4f4d6a457a7e29eab9203c88e6ee1e05d99/> update Wasmtime  and wasm-tools usage in wash
 - <csr-id-6b42f9a2282eab209a2f1f3e169bb66582aa6d62/> more explicit errors for missing binary

### New Features

 - <csr-id-d9491b364499f36880eaf32fc9765d5cf1fcb664/> ref parsing for components to match providers
   This commit improves the component reference parsing/resolution to be
   as advanced for components as it is for providers.
 - <csr-id-c3a5a6f63c05076baa1233fabc9c9345456e2169/> add wadm_pid_file() path fn

### Bug Fixes

 - <csr-id-ea98e1ee0e42de3134bee5e62c6ee7522a71a105/> use resolved component ref for start
 - <csr-id-2cc1a364d0f37adcb87bec27799884edf2208e93/> decode body payload as string
 - <csr-id-4adb08ac26210537ff7bf6a87722d0b3a7248761/> add missing tokio features
 - <csr-id-1a4f81ddd6e344c20c09b2493dd02047c3d651ca/> remove double event wait for scale

### Refactor

 - <csr-id-3fb79daf65f9f029ca0227cfdac7b504d7bd9c6c/> use upstream preview1 adapter crate

### New Features (BREAKING)

 - <csr-id-301043bb0f86d15e3afb93e410a3a40242c6317a/> display detailed app status

## v0.23.0 (2024-08-02)

<csr-id-e39430bbdba29d70ee0afbb0f62270189d8e74c7/>
<csr-id-8115f3208e6221d2f65a00d6618f333566a923de/>
<csr-id-94bfb0e23d4f1f58b70500eaa635717a6ba83484/>
<csr-id-27cdeb83c0737251a699acf55c718e05fc39032e/>
<csr-id-24e459251eaff69820180c8aaf7663ecc4e76b35/>
<csr-id-353e0ca7761757fbd8f6e7b992d6aaa1d1fa15bd/>
<csr-id-0cfa42e3de670695abff179a87d5bb145b9e7844/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>

### Chore

 - <csr-id-e39430bbdba29d70ee0afbb0f62270189d8e74c7/> Add host_pid_file convenience function for locating wasmcloud host pid
 - <csr-id-8115f3208e6221d2f65a00d6618f333566a923de/> fix clippy lints
 - <csr-id-94bfb0e23d4f1f58b70500eaa635717a6ba83484/> partially update to NATS 0.35.1
 - <csr-id-27cdeb83c0737251a699acf55c718e05fc39032e/> add alias for --interfaces
 - <csr-id-24e459251eaff69820180c8aaf7663ecc4e76b35/> remove warnings on windows
 - <csr-id-353e0ca7761757fbd8f6e7b992d6aaa1d1fa15bd/> bump to v0.29.2 for wadm-client
 - <csr-id-0cfa42e3de670695abff179a87d5bb145b9e7844/> update wadm-client to v0.1.2 in lock

### New Features

 - <csr-id-98558bb6746dc6a7f3a1a6826e2143b68efae77c/> build rust providers based on the build mode
 - <csr-id-d5bcd66c19c8a6a106c263516766a3f8e183d061/> add support for debug build for rust providers
 - <csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/> upgrade `wrpc`, `async-nats`, `wasmtime`
 - <csr-id-4eba7f8b738ee83c53040cb22494f5b249cd79af/> Adds flag to wash up to allow reading custom NATS config
   - Updated NATS server startup command to handle a configuration file (any file ending with .conf).

### Bug Fixes

 - <csr-id-0719901e8fe7a7303c817cf732867ff2a486f588/> Ensure wasmcloud host pid file exists before attempting to remove it
 - <csr-id-f34b6014065e9bc75c451fb6712d4a6d349d8ab7/> update usage of WitPrinter::print
 - <csr-id-2cee7ba6619a3b861abca87722f462294b78042b/> fix build.rs cfg setting
 - <csr-id-ec9659a915631134064d8e252b6c7d8b6bf322e1/> re-add wash call
   This commit re-adds `wash call` with the existing functionality (no
   improvements) that existed prior to the recent wrpc update.
   
   With this update, invoking simple components and HTTP components both
   work, and tests have been re-enabled.
 - <csr-id-d14d2acc8f1c108b5f506b6031b5b9b58e07d3ef/> remove invalid category 'wasmcloud'
 - <csr-id-29a9accee5741962b76d13d5a724f518b6882bef/> invalid target_arch riscv64gc
 - <csr-id-1a601bb18f09feee0af403555ff96fcfac39a8e8/> default debug to optional
 - <csr-id-e6e445f5857c0bca95ca78657eedbeb5aed33f95/> print the wadm version notes to stderr
   The messages being printed to stdout where JSON output would normally
   be produced, leading to test failures (when we tried to parse output).
 - <csr-id-94188ea344d0507510f50f0f8d4e72fd2a204500/> enable `std` feature for `clap`
 - <csr-id-88209d0c59b813fb6b4e82a5457dd216fddbd877/> use squid-proxy from cgr.dev
 - <csr-id-759764db606a168104ef085bc64b947730140980/> avoid repeated downloads of wadm binary #2308

### Other

 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 23 commits contributed to the release over the course of 49 calendar days.
 - 49 days passed between releases.
 - 23 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Add host_pid_file convenience function for locating wasmcloud host pid ([`e39430b`](https://github.com/wasmCloud/wasmCloud/commit/e39430bbdba29d70ee0afbb0f62270189d8e74c7))
    - Ensure wasmcloud host pid file exists before attempting to remove it ([`0719901`](https://github.com/wasmCloud/wasmCloud/commit/0719901e8fe7a7303c817cf732867ff2a486f588))
    - Update usage of WitPrinter::print ([`f34b601`](https://github.com/wasmCloud/wasmCloud/commit/f34b6014065e9bc75c451fb6712d4a6d349d8ab7))
    - Bump for test-util release ([`7cd2e71`](https://github.com/wasmCloud/wasmCloud/commit/7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4))
    - Fix build.rs cfg setting ([`2cee7ba`](https://github.com/wasmCloud/wasmCloud/commit/2cee7ba6619a3b861abca87722f462294b78042b))
    - Fix clippy lints ([`8115f32`](https://github.com/wasmCloud/wasmCloud/commit/8115f3208e6221d2f65a00d6618f333566a923de))
    - Re-add wash call ([`ec9659a`](https://github.com/wasmCloud/wasmCloud/commit/ec9659a915631134064d8e252b6c7d8b6bf322e1))
    - Remove invalid category 'wasmcloud' ([`d14d2ac`](https://github.com/wasmCloud/wasmCloud/commit/d14d2acc8f1c108b5f506b6031b5b9b58e07d3ef))
    - Invalid target_arch riscv64gc ([`29a9acc`](https://github.com/wasmCloud/wasmCloud/commit/29a9accee5741962b76d13d5a724f518b6882bef))
    - Default debug to optional ([`1a601bb`](https://github.com/wasmCloud/wasmCloud/commit/1a601bb18f09feee0af403555ff96fcfac39a8e8))
    - Build rust providers based on the build mode ([`98558bb`](https://github.com/wasmCloud/wasmCloud/commit/98558bb6746dc6a7f3a1a6826e2143b68efae77c))
    - Add support for debug build for rust providers ([`d5bcd66`](https://github.com/wasmCloud/wasmCloud/commit/d5bcd66c19c8a6a106c263516766a3f8e183d061))
    - Print the wadm version notes to stderr ([`e6e445f`](https://github.com/wasmCloud/wasmCloud/commit/e6e445f5857c0bca95ca78657eedbeb5aed33f95))
    - Partially update to NATS 0.35.1 ([`94bfb0e`](https://github.com/wasmCloud/wasmCloud/commit/94bfb0e23d4f1f58b70500eaa635717a6ba83484))
    - Enable `std` feature for `clap` ([`94188ea`](https://github.com/wasmCloud/wasmCloud/commit/94188ea344d0507510f50f0f8d4e72fd2a204500))
    - Upgrade `wrpc`, `async-nats`, `wasmtime` ([`9cb1b78`](https://github.com/wasmCloud/wasmCloud/commit/9cb1b784fe7a8892d73bdb40d1172b1879fcd932))
    - Use squid-proxy from cgr.dev ([`88209d0`](https://github.com/wasmCloud/wasmCloud/commit/88209d0c59b813fb6b4e82a5457dd216fddbd877))
    - Add alias for --interfaces ([`27cdeb8`](https://github.com/wasmCloud/wasmCloud/commit/27cdeb83c0737251a699acf55c718e05fc39032e))
    - Adds flag to wash up to allow reading custom NATS config ([`4eba7f8`](https://github.com/wasmCloud/wasmCloud/commit/4eba7f8b738ee83c53040cb22494f5b249cd79af))
    - Avoid repeated downloads of wadm binary #2308 ([`759764d`](https://github.com/wasmCloud/wasmCloud/commit/759764db606a168104ef085bc64b947730140980))
    - Remove warnings on windows ([`24e4592`](https://github.com/wasmCloud/wasmCloud/commit/24e459251eaff69820180c8aaf7663ecc4e76b35))
    - Bump to v0.29.2 for wadm-client ([`353e0ca`](https://github.com/wasmCloud/wasmCloud/commit/353e0ca7761757fbd8f6e7b992d6aaa1d1fa15bd))
    - Update wadm-client to v0.1.2 in lock ([`0cfa42e`](https://github.com/wasmCloud/wasmCloud/commit/0cfa42e3de670695abff179a87d5bb145b9e7844))
</details>

## v0.22.1 (2024-06-13)

<csr-id-3cd6d232ed4359d69973dc6ee5a766115d0823d4/>
<csr-id-e57d01800606f0ba0486b20c207f8cd952181414/>
<csr-id-6cc63eb91260bc44c79a7e7c4a208f679ac90792/>
<csr-id-7b8800121b7112d3ce44a7f4b939a5d654c35a61/>
<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-c7d5819ffead001bd5e2cd5ca628ee9c4be92e08/>
<csr-id-88c07bf3be18da4f4afac3e7e356ddc507a6d85e/>
<csr-id-0a08cd885f2df95b6330677bf9b0a9573300a394/>
<csr-id-2336eebf38fc9c64727a5350f99c00d86b6f19c9/>
<csr-id-8bd1b0990caea13466cc26cd911cc84059308ae2/>
<csr-id-63afb6b67c23aad38a51e829f0ae7bfd5c41def6/>

### Chore

 - <csr-id-3cd6d232ed4359d69973dc6ee5a766115d0823d4/> Apply cargo fmt
 - <csr-id-e57d01800606f0ba0486b20c207f8cd952181414/> Remove cloud events related to actor
 - <csr-id-6cc63eb91260bc44c79a7e7c4a208f679ac90792/> Replace actor references by component in wash-lib crate
 - <csr-id-7b8800121b7112d3ce44a7f4b939a5d654c35a61/> update nkeys to 0.4
   Update to nkeys 0.4 in preparation for using xkeys in the host.
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-c7d5819ffead001bd5e2cd5ca628ee9c4be92e08/> Add tests to validate HTTP(S)_PROXY configuration with and without auth
 - <csr-id-88c07bf3be18da4f4afac3e7e356ddc507a6d85e/> Bump oci-distribution to 0.11.0

### New Features

<csr-id-2aa6086f5ef482cd596e022f8ef1649238ccb4f4/>
<csr-id-ec653e0f91e9d72f9cf63fbf96aa26bbfbff336b/>
<csr-id-4b38dddf2295316677cbe75695eb4bffadfe1d18/>
<csr-id-3b4e27cdd43f01420ee86d58c70cf5f9ea93bf3c/>

 - <csr-id-b521b6d9405322d43763be5b924d567a330df48c/> error when updating a component multiple hosts run
   This commit updates wash-lib to throw an error when attempting to
   update a component that multiple hosts run.
 - <csr-id-179839605f6e350e0674020d5a4b90fe620ab5f8/> enable custom TLS CA usage
 - <csr-id-d859c74dcded69bfbb505663ba2ee1b1429eb465/> Allows for pushing binary wit packages with wash
   This rounds out a feature I didn't think we'd need for a while
 - <csr-id-10e1d72fd1e899b01e38f842b9a4c7c3048f2657/> add `wash app validate` subcommand
   This commit adds a `wash app validate` subcommand which can be used to
   check and suggest fixes for WADM manifests.
   
   As the breadth of possible errors with a manifest is wide, it's
   difficult to enumerate and check every possible error, but validate
   serves as a starting point in being able to give users proactive
   advice on WADM manifests.
   
   For now, it checks:
   - interface names (ex. typos, misnamed host-supported interfaces)

### Bug Fixes

 - <csr-id-1b3c506b2ffceab47bbe8c23c09241600c0fac37/> serialize manifest to deploy
 - <csr-id-b0b0497238ff8b1858b4440f5d189b3a6d430e10/> Setup extra_root_certificates for OCI push client

### Other

 - <csr-id-0a08cd885f2df95b6330677bf9b0a9573300a394/> Renames http client example to something a bit more clear
 - <csr-id-2336eebf38fc9c64727a5350f99c00d86b6f19c9/> Updates various examples based on PR feedback

### Test

 - <csr-id-8bd1b0990caea13466cc26cd911cc84059308ae2/> add command output struct for `wash up`

### Chore (BREAKING)

 - <csr-id-63afb6b67c23aad38a51e829f0ae7bfd5c41def6/> Remove deprecated RegistryPingCommand

### New Features (BREAKING)

 - <csr-id-adbced40c06ec035f3f8b5d0fd062f20d622e0ee/> add --skip-wait option to scale subcommand
   This command changes the default for scale commands, ensuring that
   waiting is the default and a `--skip-wait` option is present.
 - <csr-id-b930cf58131215748861c1ed8a837bbb550b4f81/> wrap new wadm-client, results
 - <csr-id-894e02b2269e8e23a6430b9daeacfc98931587c8/> add custom go provider template
 - <csr-id-0403f409cc3a6c9af275a50d008b05ac4ba1c870/> support building go providers
 - <csr-id-127476643df38fdb8c8928c0e7d2eca070e1aef9/> add custom rust provider template
 - <csr-id-08b5e1e92c411d2d913537937aec3a8ca5ccb405/> Updates wash to use the new OCI spec for wasm
   This is backwards compatible in that it can still pull the old manifest
   type, but it now only pushes the new manifest type. For probably all of
   our current users, they shouldn't notice this change, but it is
   technically a breaking change to start pushing in a different way

### Bug Fixes (BREAKING)

 - <csr-id-c341171ccacc6170bf85fe0267facbb94af534ac/> Removes need for world flag
   Based on feedback from users, we found out that the world isn't actually
   needed for pushing binary wit. This was updated in the oci-wasm library
   that was also updated in this PR. This removes the world flag as it is
   no longer needed

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 31 commits contributed to the release over the course of 28 calendar days.
 - 33 days passed between releases.
 - 28 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Error when updating a component multiple hosts run ([`b521b6d`](https://github.com/wasmCloud/wasmCloud/commit/b521b6d9405322d43763be5b924d567a330df48c))
    - Serialize manifest to deploy ([`1b3c506`](https://github.com/wasmCloud/wasmCloud/commit/1b3c506b2ffceab47bbe8c23c09241600c0fac37))
    - Apply cargo fmt ([`3cd6d23`](https://github.com/wasmCloud/wasmCloud/commit/3cd6d232ed4359d69973dc6ee5a766115d0823d4))
    - Remove cloud events related to actor ([`e57d018`](https://github.com/wasmCloud/wasmCloud/commit/e57d01800606f0ba0486b20c207f8cd952181414))
    - Replace actor references by component in wash-lib crate ([`6cc63eb`](https://github.com/wasmCloud/wasmCloud/commit/6cc63eb91260bc44c79a7e7c4a208f679ac90792))
    - Bump wascap v0.15.0, wasmcloud-core v0.7.0, wash-lib v0.22.0, wasmcloud-tracing v0.5.0, wasmcloud-provider-sdk v0.6.0, wash-cli v0.29.0, safety bump 5 crates ([`2e38cd4`](https://github.com/wasmCloud/wasmCloud/commit/2e38cd45adef18d47af71b87ca456a25edb2f53a))
    - Add --skip-wait option to scale subcommand ([`adbced4`](https://github.com/wasmCloud/wasmCloud/commit/adbced40c06ec035f3f8b5d0fd062f20d622e0ee))
    - Wrap new wadm-client, results ([`b930cf5`](https://github.com/wasmCloud/wasmCloud/commit/b930cf58131215748861c1ed8a837bbb550b4f81))
    - Enable custom TLS CA usage ([`1798396`](https://github.com/wasmCloud/wasmCloud/commit/179839605f6e350e0674020d5a4b90fe620ab5f8))
    - Removes need for world flag ([`c341171`](https://github.com/wasmCloud/wasmCloud/commit/c341171ccacc6170bf85fe0267facbb94af534ac))
    - Add custom go provider template ([`894e02b`](https://github.com/wasmCloud/wasmCloud/commit/894e02b2269e8e23a6430b9daeacfc98931587c8))
    - Update nkeys to 0.4 ([`7b88001`](https://github.com/wasmCloud/wasmCloud/commit/7b8800121b7112d3ce44a7f4b939a5d654c35a61))
    - Support building go providers ([`0403f40`](https://github.com/wasmCloud/wasmCloud/commit/0403f409cc3a6c9af275a50d008b05ac4ba1c870))
    - Allows for pushing binary wit packages with wash ([`d859c74`](https://github.com/wasmCloud/wasmCloud/commit/d859c74dcded69bfbb505663ba2ee1b1429eb465))
    - Add `wash app validate` subcommand ([`10e1d72`](https://github.com/wasmCloud/wasmCloud/commit/10e1d72fd1e899b01e38f842b9a4c7c3048f2657))
    - Add support for `wash up --wadm-manifest` ([`2aa6086`](https://github.com/wasmCloud/wasmCloud/commit/2aa6086f5ef482cd596e022f8ef1649238ccb4f4))
    - Add command output struct for `wash up` ([`8bd1b09`](https://github.com/wasmCloud/wasmCloud/commit/8bd1b0990caea13466cc26cd911cc84059308ae2))
    - Replace actor references by component in crates ([`20c72ce`](https://github.com/wasmCloud/wasmCloud/commit/20c72ce0ed423561ae6dbd5a91959bec24ff7cf3))
    - Add tests to validate HTTP(S)_PROXY configuration with and without auth ([`c7d5819`](https://github.com/wasmCloud/wasmCloud/commit/c7d5819ffead001bd5e2cd5ca628ee9c4be92e08))
    - Support configuring proxy credentials for HTTP(S)_PROXY when downloading artifacts ([`ec653e0`](https://github.com/wasmCloud/wasmCloud/commit/ec653e0f91e9d72f9cf63fbf96aa26bbfbff336b))
    - Add custom rust provider template ([`1274766`](https://github.com/wasmCloud/wasmCloud/commit/127476643df38fdb8c8928c0e7d2eca070e1aef9))
    - Updates wash to use the new OCI spec for wasm ([`08b5e1e`](https://github.com/wasmCloud/wasmCloud/commit/08b5e1e92c411d2d913537937aec3a8ca5ccb405))
    - Provided Alias for -- link del as -- link delete ([`cb4f23a`](https://github.com/wasmCloud/wasmCloud/commit/cb4f23af3bab2be2488e74dc4d38c6f312b123b4))
    - Add option to skip certificate validation for the OCI registry connection ([`f9aa387`](https://github.com/wasmCloud/wasmCloud/commit/f9aa3879d273ae9b44f5ee09a724f76df9859d7a))
    - Add support for specifying multiple labels ([`4b38ddd`](https://github.com/wasmCloud/wasmCloud/commit/4b38dddf2295316677cbe75695eb4bffadfe1d18))
    - Bump oci-distribution to 0.11.0 ([`88c07bf`](https://github.com/wasmCloud/wasmCloud/commit/88c07bf3be18da4f4afac3e7e356ddc507a6d85e))
    - Remove deprecated RegistryPingCommand ([`63afb6b`](https://github.com/wasmCloud/wasmCloud/commit/63afb6b67c23aad38a51e829f0ae7bfd5c41def6))
    - Renames http client example to something a bit more clear ([`0a08cd8`](https://github.com/wasmCloud/wasmCloud/commit/0a08cd885f2df95b6330677bf9b0a9573300a394))
    - Updates various examples based on PR feedback ([`2336eeb`](https://github.com/wasmCloud/wasmCloud/commit/2336eebf38fc9c64727a5350f99c00d86b6f19c9))
    - Adds an http-client example ([`3b4e27c`](https://github.com/wasmCloud/wasmCloud/commit/3b4e27cdd43f01420ee86d58c70cf5f9ea93bf3c))
    - Setup extra_root_certificates for OCI push client ([`b0b0497`](https://github.com/wasmCloud/wasmCloud/commit/b0b0497238ff8b1858b4440f5d189b3a6d430e10))
</details>

## v0.22.0 (2024-06-11)

<csr-id-7b8800121b7112d3ce44a7f4b939a5d654c35a61/>
<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-c7d5819ffead001bd5e2cd5ca628ee9c4be92e08/>
<csr-id-88c07bf3be18da4f4afac3e7e356ddc507a6d85e/>
<csr-id-0a08cd885f2df95b6330677bf9b0a9573300a394/>
<csr-id-2336eebf38fc9c64727a5350f99c00d86b6f19c9/>
<csr-id-8bd1b0990caea13466cc26cd911cc84059308ae2/>
<csr-id-63afb6b67c23aad38a51e829f0ae7bfd5c41def6/>

### Chore

 - <csr-id-7b8800121b7112d3ce44a7f4b939a5d654c35a61/> update nkeys to 0.4
   Update to nkeys 0.4 in preparation for using xkeys in the host.
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-c7d5819ffead001bd5e2cd5ca628ee9c4be92e08/> Add tests to validate HTTP(S)_PROXY configuration with and without auth
 - <csr-id-88c07bf3be18da4f4afac3e7e356ddc507a6d85e/> Bump oci-distribution to 0.11.0

### New Features

<csr-id-2aa6086f5ef482cd596e022f8ef1649238ccb4f4/>
<csr-id-ec653e0f91e9d72f9cf63fbf96aa26bbfbff336b/>
<csr-id-4b38dddf2295316677cbe75695eb4bffadfe1d18/>
<csr-id-3b4e27cdd43f01420ee86d58c70cf5f9ea93bf3c/>

 - <csr-id-179839605f6e350e0674020d5a4b90fe620ab5f8/> enable custom TLS CA usage
 - <csr-id-d859c74dcded69bfbb505663ba2ee1b1429eb465/> Allows for pushing binary wit packages with wash
   This rounds out a feature I didn't think we'd need for a while
 - <csr-id-10e1d72fd1e899b01e38f842b9a4c7c3048f2657/> add `wash app validate` subcommand
   This commit adds a `wash app validate` subcommand which can be used to
   check and suggest fixes for WADM manifests.
   
   As the breadth of possible errors with a manifest is wide, it's
   difficult to enumerate and check every possible error, but validate
   serves as a starting point in being able to give users proactive
   advice on WADM manifests.
   
   For now, it checks:
   - interface names (ex. typos, misnamed host-supported interfaces)

### Bug Fixes

 - <csr-id-b0b0497238ff8b1858b4440f5d189b3a6d430e10/> Setup extra_root_certificates for OCI push client

### Other

 - <csr-id-0a08cd885f2df95b6330677bf9b0a9573300a394/> Renames http client example to something a bit more clear
 - <csr-id-2336eebf38fc9c64727a5350f99c00d86b6f19c9/> Updates various examples based on PR feedback

### Test

 - <csr-id-8bd1b0990caea13466cc26cd911cc84059308ae2/> add command output struct for `wash up`

### Chore (BREAKING)

 - <csr-id-63afb6b67c23aad38a51e829f0ae7bfd5c41def6/> Remove deprecated RegistryPingCommand

### New Features (BREAKING)

 - <csr-id-adbced40c06ec035f3f8b5d0fd062f20d622e0ee/> add --skip-wait option to scale subcommand
   This command changes the default for scale commands, ensuring that
   waiting is the default and a `--skip-wait` option is present.
 - <csr-id-b930cf58131215748861c1ed8a837bbb550b4f81/> wrap new wadm-client, results
 - <csr-id-894e02b2269e8e23a6430b9daeacfc98931587c8/> add custom go provider template
 - <csr-id-0403f409cc3a6c9af275a50d008b05ac4ba1c870/> support building go providers
 - <csr-id-127476643df38fdb8c8928c0e7d2eca070e1aef9/> add custom rust provider template
 - <csr-id-08b5e1e92c411d2d913537937aec3a8ca5ccb405/> Updates wash to use the new OCI spec for wasm
   This is backwards compatible in that it can still pull the old manifest
   type, but it now only pushes the new manifest type. For probably all of
   our current users, they shouldn't notice this change, but it is
   technically a breaking change to start pushing in a different way

### Bug Fixes (BREAKING)

 - <csr-id-c341171ccacc6170bf85fe0267facbb94af534ac/> Removes need for world flag
   Based on feedback from users, we found out that the world isn't actually
   needed for pushing binary wit. This was updated in the oci-wasm library
   that was also updated in this PR. This removes the world flag as it is
   no longer needed

<csr-unknown>
<csr-unknown>
<csr-unknown>
<csr-unknown>
dangling providers/components which aren’t linked to anything<csr-unknown>
 add support for wash up --wadm-manifestThis commit adds support for wash up --wadm-manifest, which deploysa WADM manifest after running wash up. If the manifest existsalready, it is not re-deployed, but it is deployed once. Support configuring proxy credentials for HTTP(S)_PROXY when downloading artifacts add support for specifying multiple labelsThis commit adds support for specifying multiple labels to wash label.Users can use wash label <host-id> key1=value1,key2=value2 to setmultiple labels on the host at the same time, in a best-effort manner Adds an http-client exampleWe’ve been missing an example of the http-client (outgoing-response)interface for a while. This adds one that fetches you a random pictureof a dog<csr-unknown/>
<csr-unknown/>
<csr-unknown/>
<csr-unknown/>
<csr-unknown/>

## v0.21.1 (2024-05-10)

<csr-id-a4a772fb475c1f76215b7fe7aece9c2335bd0c69/>
<csr-id-7ca9a3ec37a4f031ffdfbee08a110ead0cbbc435/>
<csr-id-468bad52bab3b907d0380cdf2c151298688b50d1/>
<csr-id-d3a837c839d1a340daf72315833a3e2cbd1db0f3/>
<csr-id-07a78ec397ec9bd3b742490f8f36ac4db854ca9f/>
<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-ac3ec843f22b2946df8e2b52735a13569eaa78d6/>
<csr-id-c074106584ab5330a0ac346b5a51676bd966aa3c/>
<csr-id-bfeabbefa64a969f48c05f02b336ef229d0f5b2c/>
<csr-id-57446f39762be82821bd38b6c4bd16471a9c3095/>
<csr-id-14fd9b1ad8fdbce8efd6cc9ddce52ea08ef264b7/>
<csr-id-9fdc7e52c2cfbd10fab08d34d3a7e8047eaa5432/>

### Chore

 - <csr-id-a4a772fb475c1f76215b7fe7aece9c2335bd0c69/> bump patch for release
 - <csr-id-7ca9a3ec37a4f031ffdfbee08a110ead0cbbc435/> update [actor] to [component]
 - <csr-id-468bad52bab3b907d0380cdf2c151298688b50d1/> replace references to 'actor' with 'component'
 - <csr-id-d3a837c839d1a340daf72315833a3e2cbd1db0f3/> rename actor->component build output
 - <csr-id-07a78ec397ec9bd3b742490f8f36ac4db854ca9f/> add link get alias
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### New Features

 - <csr-id-cbac8fef75bd8dda2554bd1665e75a60059ba4c3/> Adds digest and tag to output of `wash push`
   This follows a similar (but not exact) format from `docker push` and
   includes the digest and tag in JSON output.
 - <csr-id-012bfb6e6bc0e43af8a0223ddc853bd864e93816/> allow relative paths when starting providers
 - <csr-id-58ff0cba00d67f1a8d19034193002ed84aeda699/> Ensure that plugins cannot access sockets
 - <csr-id-6e64ae27517e79bd9e16fd014cf37c2757bf8caa/> Ensure that plugins cannot access sockets
 - <csr-id-d9f1982faeb6ad7365fab39a96019f95e02156e8/> Adds example for wash plugin
   This also adds a pipeline for packaging up the wash plugin wit for
   consumption. In the future we can add a bare component version as well
   for use with tools like `cargo component`
 - <csr-id-6cb20f900e1ec7dca4b1420c59b3d216014cd93f/> Adds `plugin` subcommand
   Wash now has a plugin subcommand that helps manage your plugins and can
   install from HTTP, OCI, and local files. Once we have a bit more
   scaffolding and example plugins around, we can probably build those and
   use those in an e2e test for this command. For now, I did manually
   validate all of the new commands
 - <csr-id-26d78e3b50beaa8e23d17002f4139210ef287d30/> Add env var filtering for plugins
 - <csr-id-026ecdc473e64c18105fd6f79dc2bad58814e0bf/> Adds support for a scratch space directory
   All plugins get their own directory keyed by ID and can create files
   in that space. Also updates the test to make sure it works
 - <csr-id-dd8a48c6b40f76b5e18d37bd49b9ec1b41e58431/> Adds caching to wasmtime to speed up plugin load
 - <csr-id-0c1dd15e84e9ca86a563168c5e86f32dbd8f2831/> Integrates plugins into the CLI
   This integrates plugins into the CLI and they now function properly. Next
   step is caching component compilation
 - <csr-id-3afe0aaa83989c133cfb65de5af2fb6ffeacf138/> Adds plugin functionality to wash-lib
   This only adds it to wash-lib, next commit will be adding this into the
   actual CLI
 - <csr-id-5e81571a5f0dfd08dd8aab4710b731c6f0c685e8/> re-add wash call tests
   This commit re-adds the missing `wash call` tests to the codebase,
   enhancing `wash call` to be able to invoke incoming HTTP handlers
   along the way.

### Bug Fixes

 - <csr-id-1b4faabea11ba6b77b75e34f6892f979be0adde5/> Make wash push returned digest based on the pushed manifest
 - <csr-id-42d60d20aeb80c7130b5f5f852ce0bc063cfb399/> already updated must succeed to shell
 - <csr-id-6cf9672d69ba96cb8139a2184f3eea9a0e32dc42/> fixed one of the failing tests
 - <csr-id-1cbca5904b65689ac96d88e8e7df94492a8dad79/> re-adding the changes to make sure tests pass sucessfully
 - <csr-id-8b00bd35d752e939e3d7725406dc7fdfc1d30d33/> update wash README

### Other

 - <csr-id-ac3ec843f22b2946df8e2b52735a13569eaa78d6/> release and update CHANGELOG
 - <csr-id-c074106584ab5330a0ac346b5a51676bd966aa3c/> Change plugins to support arbitrary path access
   This allows plugins to mark arguments as paths so that they can be
   preopened and allowed in the component. This tries to walk a path between
   security and flexibility. If an argument is marked as a path, wash will
   allow full access to it if it is a directory and then limited access to
   a directory and full access to the file if it is a path. It isn't
   perfect due to the limited nature of preopens, but it does mean that the
   plugin will not get access to anything outside of its scratch dir
   without the user explicitly passing the path.
   
   Once this is merged there will be two follow ups: one is a PR to this
   repo updating the example code and the other will be to the docs repo
   to update documentation on the security around paths
 - <csr-id-bfeabbefa64a969f48c05f02b336ef229d0f5b2c/> prevent component update with same image reference

### Refactor

 - <csr-id-57446f39762be82821bd38b6c4bd16471a9c3095/> ensure file open errors are more informative
 - <csr-id-14fd9b1ad8fdbce8efd6cc9ddce52ea08ef264b7/> change command output messages for update component

### Chore (BREAKING)

 - <csr-id-9fdc7e52c2cfbd10fab08d34d3a7e8047eaa5432/> remove interface generation

### New Features (BREAKING)

 - <csr-id-eb82203163249bd7d3252657e04b8d00cd397a14/> make link del interface consistent

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 32 commits contributed to the release over the course of 22 calendar days.
 - 22 days passed between releases.
 - 30 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump patch for release ([`a4a772f`](https://github.com/wasmCloud/wasmCloud/commit/a4a772fb475c1f76215b7fe7aece9c2335bd0c69))
    - Make wash push returned digest based on the pushed manifest ([`1b4faab`](https://github.com/wasmCloud/wasmCloud/commit/1b4faabea11ba6b77b75e34f6892f979be0adde5))
    - Bump provider-archive v0.10.2, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0 ([`73c0ef0`](https://github.com/wasmCloud/wasmCloud/commit/73c0ef0bbe2f6b525655939d2cd30740aef4b6bc))
    - Release and update CHANGELOG ([`ac3ec84`](https://github.com/wasmCloud/wasmCloud/commit/ac3ec843f22b2946df8e2b52735a13569eaa78d6))
    - Bump provider-archive v0.10.1, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0, safety bump 5 crates ([`75a2e52`](https://github.com/wasmCloud/wasmCloud/commit/75a2e52f52690ba143679c90237851ebd07e153f))
    - Adds digest and tag to output of `wash push` ([`cbac8fe`](https://github.com/wasmCloud/wasmCloud/commit/cbac8fef75bd8dda2554bd1665e75a60059ba4c3))
    - Change plugins to support arbitrary path access ([`c074106`](https://github.com/wasmCloud/wasmCloud/commit/c074106584ab5330a0ac346b5a51676bd966aa3c))
    - Update [actor] to [component] ([`7ca9a3e`](https://github.com/wasmCloud/wasmCloud/commit/7ca9a3ec37a4f031ffdfbee08a110ead0cbbc435))
    - Allow relative paths when starting providers ([`012bfb6`](https://github.com/wasmCloud/wasmCloud/commit/012bfb6e6bc0e43af8a0223ddc853bd864e93816))
    - Ensure file open errors are more informative ([`57446f3`](https://github.com/wasmCloud/wasmCloud/commit/57446f39762be82821bd38b6c4bd16471a9c3095))
    - Ensure that plugins cannot access sockets ([`58ff0cb`](https://github.com/wasmCloud/wasmCloud/commit/58ff0cba00d67f1a8d19034193002ed84aeda699))
    - Ensure that plugins cannot access sockets ([`6e64ae2`](https://github.com/wasmCloud/wasmCloud/commit/6e64ae27517e79bd9e16fd014cf37c2757bf8caa))
    - Adds example for wash plugin ([`d9f1982`](https://github.com/wasmCloud/wasmCloud/commit/d9f1982faeb6ad7365fab39a96019f95e02156e8))
    - Adds `plugin` subcommand ([`6cb20f9`](https://github.com/wasmCloud/wasmCloud/commit/6cb20f900e1ec7dca4b1420c59b3d216014cd93f))
    - Add env var filtering for plugins ([`26d78e3`](https://github.com/wasmCloud/wasmCloud/commit/26d78e3b50beaa8e23d17002f4139210ef287d30))
    - Adds support for a scratch space directory ([`026ecdc`](https://github.com/wasmCloud/wasmCloud/commit/026ecdc473e64c18105fd6f79dc2bad58814e0bf))
    - Adds caching to wasmtime to speed up plugin load ([`dd8a48c`](https://github.com/wasmCloud/wasmCloud/commit/dd8a48c6b40f76b5e18d37bd49b9ec1b41e58431))
    - Integrates plugins into the CLI ([`0c1dd15`](https://github.com/wasmCloud/wasmCloud/commit/0c1dd15e84e9ca86a563168c5e86f32dbd8f2831))
    - Adds plugin functionality to wash-lib ([`3afe0aa`](https://github.com/wasmCloud/wasmCloud/commit/3afe0aaa83989c133cfb65de5af2fb6ffeacf138))
    - Already updated must succeed to shell ([`42d60d2`](https://github.com/wasmCloud/wasmCloud/commit/42d60d20aeb80c7130b5f5f852ce0bc063cfb399))
    - Change command output messages for update component ([`14fd9b1`](https://github.com/wasmCloud/wasmCloud/commit/14fd9b1ad8fdbce8efd6cc9ddce52ea08ef264b7))
    - Prevent component update with same image reference ([`bfeabbe`](https://github.com/wasmCloud/wasmCloud/commit/bfeabbefa64a969f48c05f02b336ef229d0f5b2c))
    - Replace references to 'actor' with 'component' ([`468bad5`](https://github.com/wasmCloud/wasmCloud/commit/468bad52bab3b907d0380cdf2c151298688b50d1))
    - Fixed one of the failing tests ([`6cf9672`](https://github.com/wasmCloud/wasmCloud/commit/6cf9672d69ba96cb8139a2184f3eea9a0e32dc42))
    - Re-adding the changes to make sure tests pass sucessfully ([`1cbca59`](https://github.com/wasmCloud/wasmCloud/commit/1cbca5904b65689ac96d88e8e7df94492a8dad79))
    - Remove interface generation ([`9fdc7e5`](https://github.com/wasmCloud/wasmCloud/commit/9fdc7e52c2cfbd10fab08d34d3a7e8047eaa5432))
    - Update wash README ([`8b00bd3`](https://github.com/wasmCloud/wasmCloud/commit/8b00bd35d752e939e3d7725406dc7fdfc1d30d33))
    - Rename actor->component build output ([`d3a837c`](https://github.com/wasmCloud/wasmCloud/commit/d3a837c839d1a340daf72315833a3e2cbd1db0f3))
    - Add link get alias ([`07a78ec`](https://github.com/wasmCloud/wasmCloud/commit/07a78ec397ec9bd3b742490f8f36ac4db854ca9f))
    - Make link del interface consistent ([`eb82203`](https://github.com/wasmCloud/wasmCloud/commit/eb82203163249bd7d3252657e04b8d00cd397a14))
    - Re-add wash call tests ([`5e81571`](https://github.com/wasmCloud/wasmCloud/commit/5e81571a5f0dfd08dd8aab4710b731c6f0c685e8))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
</details>

## v0.21.0 (2024-05-08)

<csr-id-7ca9a3ec37a4f031ffdfbee08a110ead0cbbc435/>
<csr-id-468bad52bab3b907d0380cdf2c151298688b50d1/>
<csr-id-d3a837c839d1a340daf72315833a3e2cbd1db0f3/>
<csr-id-07a78ec397ec9bd3b742490f8f36ac4db854ca9f/>
<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-c074106584ab5330a0ac346b5a51676bd966aa3c/>
<csr-id-bfeabbefa64a969f48c05f02b336ef229d0f5b2c/>
<csr-id-57446f39762be82821bd38b6c4bd16471a9c3095/>
<csr-id-14fd9b1ad8fdbce8efd6cc9ddce52ea08ef264b7/>
<csr-id-9fdc7e52c2cfbd10fab08d34d3a7e8047eaa5432/>
<csr-id-ac3ec843f22b2946df8e2b52735a13569eaa78d6/>

### Chore

 - <csr-id-7ca9a3ec37a4f031ffdfbee08a110ead0cbbc435/> update [actor] to [component]
 - <csr-id-468bad52bab3b907d0380cdf2c151298688b50d1/> replace references to 'actor' with 'component'
 - <csr-id-d3a837c839d1a340daf72315833a3e2cbd1db0f3/> rename actor->component build output
 - <csr-id-07a78ec397ec9bd3b742490f8f36ac4db854ca9f/> add link get alias
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### Other

 - <csr-id-ac3ec843f22b2946df8e2b52735a13569eaa78d6/> release and update CHANGELOG

### New Features

 - <csr-id-cbac8fef75bd8dda2554bd1665e75a60059ba4c3/> Adds digest and tag to output of `wash push`
   This follows a similar (but not exact) format from `docker push` and
   includes the digest and tag in JSON output.
 - <csr-id-012bfb6e6bc0e43af8a0223ddc853bd864e93816/> allow relative paths when starting providers
 - <csr-id-58ff0cba00d67f1a8d19034193002ed84aeda699/> Ensure that plugins cannot access sockets
 - <csr-id-6e64ae27517e79bd9e16fd014cf37c2757bf8caa/> Ensure that plugins cannot access sockets
 - <csr-id-d9f1982faeb6ad7365fab39a96019f95e02156e8/> Adds example for wash plugin
   This also adds a pipeline for packaging up the wash plugin wit for
   consumption. In the future we can add a bare component version as well
   for use with tools like `cargo component`
 - <csr-id-6cb20f900e1ec7dca4b1420c59b3d216014cd93f/> Adds `plugin` subcommand
   Wash now has a plugin subcommand that helps manage your plugins and can
   install from HTTP, OCI, and local files. Once we have a bit more
   scaffolding and example plugins around, we can probably build those and
   use those in an e2e test for this command. For now, I did manually
   validate all of the new commands
 - <csr-id-26d78e3b50beaa8e23d17002f4139210ef287d30/> Add env var filtering for plugins
 - <csr-id-026ecdc473e64c18105fd6f79dc2bad58814e0bf/> Adds support for a scratch space directory
   All plugins get their own directory keyed by ID and can create files
   in that space. Also updates the test to make sure it works
 - <csr-id-dd8a48c6b40f76b5e18d37bd49b9ec1b41e58431/> Adds caching to wasmtime to speed up plugin load
 - <csr-id-0c1dd15e84e9ca86a563168c5e86f32dbd8f2831/> Integrates plugins into the CLI
   This integrates plugins into the CLI and they now function properly. Next
   step is caching component compilation
 - <csr-id-3afe0aaa83989c133cfb65de5af2fb6ffeacf138/> Adds plugin functionality to wash-lib
   This only adds it to wash-lib, next commit will be adding this into the
   actual CLI
 - <csr-id-5e81571a5f0dfd08dd8aab4710b731c6f0c685e8/> re-add wash call tests
   This commit re-adds the missing `wash call` tests to the codebase,
   enhancing `wash call` to be able to invoke incoming HTTP handlers
   along the way.

### Bug Fixes

 - <csr-id-42d60d20aeb80c7130b5f5f852ce0bc063cfb399/> already updated must succeed to shell
 - <csr-id-6cf9672d69ba96cb8139a2184f3eea9a0e32dc42/> fixed one of the failing tests
 - <csr-id-1cbca5904b65689ac96d88e8e7df94492a8dad79/> re-adding the changes to make sure tests pass sucessfully
 - <csr-id-8b00bd35d752e939e3d7725406dc7fdfc1d30d33/> update wash README

### Other

 - <csr-id-c074106584ab5330a0ac346b5a51676bd966aa3c/> Change plugins to support arbitrary path access
   This allows plugins to mark arguments as paths so that they can be
   preopened and allowed in the component. This tries to walk a path between
   security and flexibility. If an argument is marked as a path, wash will
   allow full access to it if it is a directory and then limited access to
   a directory and full access to the file if it is a path. It isn't
   perfect due to the limited nature of preopens, but it does mean that the
   plugin will not get access to anything outside of its scratch dir
   without the user explicitly passing the path.
   
   Once this is merged there will be two follow ups: one is a PR to this
   repo updating the example code and the other will be to the docs repo
   to update documentation on the security around paths
 - <csr-id-bfeabbefa64a969f48c05f02b336ef229d0f5b2c/> prevent component update with same image reference

### Refactor

 - <csr-id-57446f39762be82821bd38b6c4bd16471a9c3095/> ensure file open errors are more informative
 - <csr-id-14fd9b1ad8fdbce8efd6cc9ddce52ea08ef264b7/> change command output messages for update component

### Chore (BREAKING)

 - <csr-id-9fdc7e52c2cfbd10fab08d34d3a7e8047eaa5432/> remove interface generation

### New Features (BREAKING)

 - <csr-id-eb82203163249bd7d3252657e04b8d00cd397a14/> make link del interface consistent

## v0.20.0 (2024-04-17)

<csr-id-cbb7f0c96cc14af188e84f4e2b8aba412e4ce3b0/>

### Chore

 - <csr-id-cbb7f0c96cc14af188e84f4e2b8aba412e4ce3b0/> bump to v0.20.0

### Bug Fixes

 - <csr-id-2f92dde55a9e848aebf6f9934898b75de4d5b4bf/> branch reference for provider template

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 2 commits contributed to the release.
 - 4 days passed between releases.
 - 2 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Branch reference for provider template ([`2f92dde`](https://github.com/wasmCloud/wasmCloud/commit/2f92dde55a9e848aebf6f9934898b75de4d5b4bf))
    - Bump to v0.20.0 ([`cbb7f0c`](https://github.com/wasmCloud/wasmCloud/commit/cbb7f0c96cc14af188e84f4e2b8aba412e4ce3b0))
</details>

## v0.20.0-alpha.2 (2024-04-13)

<csr-id-9fb05a3d56aed3b0657d718a3cb63e173d27fbed/>
<csr-id-3611242e0712d52e1d7371b9833757f63e625655/>

### Chore

 - <csr-id-9fb05a3d56aed3b0657d718a3cb63e173d27fbed/> bump to 0.20.0-alpha.2
 - <csr-id-3611242e0712d52e1d7371b9833757f63e625655/> improve errors for missing wasmcloud.toml

### New Features

 - <csr-id-329c69bb93b7f286d7ea8642b7a187251412dff8/> change default websocket port to 4223 and enable by default

### Bug Fixes

 - <csr-id-dd891c87bdfb9c020ffb644a3c2c81f1d62f36a7/> support configuration for components
 - <csr-id-c78496759ca4703302386b7c8712c303d1f93c0a/> rename wasmcloud.toml block to component
 - <csr-id-f7582160d5bd9d7f967ada2045239bc94653cb9b/> registry image URL parsing
   When URLs are submitted to `wash push` as the first argument, unless a
   `--registry` is provided, the URL is parsed as an
   `oci_client::Reference`.
   
   It is possible for a URL like `ghcr.io/wasmCloud/img:v0.1.0` to
   correctly parse *yet* fail the the `url == image.whole()` test,
   because the lowercasing of the *supplied* URL was not used throughout
   `resolve_artifact_ref()`.
   
   This commit performs the lowercasing of the URL and registry (if
   supplied) consistently in `resolve_artifact_ref()`, ensuring that the
   comparison works, and `oci_client::Reference`s that correctly
   parse are used.

### Bug Fixes (BREAKING)

 - <csr-id-1a9b8c3586d64cff4191150bcabd10f6410eabce/> replace smithy providers with wrpc nats

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 7 commits contributed to the release over the course of 3 calendar days.
 - 3 days passed between releases.
 - 7 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump to 0.20.0-alpha.2 ([`9fb05a3`](https://github.com/wasmCloud/wasmCloud/commit/9fb05a3d56aed3b0657d718a3cb63e173d27fbed))
    - Support configuration for components ([`dd891c8`](https://github.com/wasmCloud/wasmCloud/commit/dd891c87bdfb9c020ffb644a3c2c81f1d62f36a7))
    - Rename wasmcloud.toml block to component ([`c784967`](https://github.com/wasmCloud/wasmCloud/commit/c78496759ca4703302386b7c8712c303d1f93c0a))
    - Change default websocket port to 4223 and enable by default ([`329c69b`](https://github.com/wasmCloud/wasmCloud/commit/329c69bb93b7f286d7ea8642b7a187251412dff8))
    - Replace smithy providers with wrpc nats ([`1a9b8c3`](https://github.com/wasmCloud/wasmCloud/commit/1a9b8c3586d64cff4191150bcabd10f6410eabce))
    - Improve errors for missing wasmcloud.toml ([`3611242`](https://github.com/wasmCloud/wasmCloud/commit/3611242e0712d52e1d7371b9833757f63e625655))
    - Registry image URL parsing ([`f758216`](https://github.com/wasmCloud/wasmCloud/commit/f7582160d5bd9d7f967ada2045239bc94653cb9b))
</details>

## v0.20.0-alpha.1 (2024-04-09)

<csr-id-f6e5f0e804d4a7eced93778b739bf58c30ad75e7/>
<csr-id-0e0acd728df340f4f4ae0ea31e47abaecb5b3907/>
<csr-id-fe50175294867bc8c9d109d8d610b0453fd65a1c/>
<csr-id-3a96d288714b14f1d8bab831ef4d0f9533204f56/>
<csr-id-65ff33fe473425fffb320309921dfbdcb7c8f868/>
<csr-id-ddf25d917dc241d6c5468796bca97a4c70b0d1d2/>
<csr-id-2e93989bf14b223b689f77cb4139275094debae4/>
<csr-id-c1cf682972ac9e6fa544ed79857c18d2b62ccfb8/>
<csr-id-b6dd820c45f7ea0f62c8cb91adb7074c5e8c0113/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-9018c03b0bd517c4c2f7fe643c4d510a5823bfb8/>
<csr-id-a5f9d1284d78e2dd1db1815ee2daa9d8861bd868/>
<csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/>

### Chore

 - <csr-id-f6e5f0e804d4a7eced93778b739bf58c30ad75e7/> bump wash-cli and wash-lib alpha
 - <csr-id-0e0acd728df340f4f4ae0ea31e47abaecb5b3907/> pin ctl to workspace
 - <csr-id-fe50175294867bc8c9d109d8d610b0453fd65a1c/> pin to ctl v1.0.0-alpha.2
 - <csr-id-3a96d288714b14f1d8bab831ef4d0f9533204f56/> Updates wash to use new host version
 - <csr-id-65ff33fe473425fffb320309921dfbdcb7c8f868/> address clippy warnings, simplify

### New Features

 - <csr-id-07b5e70a7f1321d184962d7197a8d98d1ecaaf71/> use native TLS roots along webpki

### Bug Fixes

 - <csr-id-ccbff56712dd96d0661538b489cb9fddff10f4ec/> use config option when getting project config
   This commit fixes the `wash push` command to ensure it uses the
   `--config` switch if provided when looking up project config.
 - <csr-id-edc660de3eb9181ebaa4fce158089a9ad625e891/> changed the variable name for a cleaner code
 - <csr-id-91c57b238c6e3aec5bd86f5c2103aaec21932725/> rename scaled ID from actor to component

### Other

 - <csr-id-ddf25d917dc241d6c5468796bca97a4c70b0d1d2/> removed debug line
 - <csr-id-2e93989bf14b223b689f77cb4139275094debae4/> modified the default key_directory to user's /home/sidconstructs directory and modified test cases

### Test

 - <csr-id-c1cf682972ac9e6fa544ed79857c18d2b62ccfb8/> expect wit-bindgen-go 0.24.0 files
 - <csr-id-b6dd820c45f7ea0f62c8cb91adb7074c5e8c0113/> update start/stop provider events

### Chore (BREAKING)

 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-9018c03b0bd517c4c2f7fe643c4d510a5823bfb8/> rename ctl actor to component

### New Features (BREAKING)

 - <csr-id-9e23be23131bbcdad746f7e85d33d5812e5f2ff9/> rename actor_scale* events
 - <csr-id-3f2d2f44470d44809fb83de2fa34b29ad1e6cb30/> Adds version to control API
   This should be the final breaking change of the API and it will require
   a two phased rollout. I'll need to cut new core and host versions first
   and then update wash to use the new host for tests.

### Bug Fixes (BREAKING)

 - <csr-id-93748a1ecd4edd785af257952f1de9497a7ea946/> remove usage of capability signing

### Refactor (BREAKING)

 - <csr-id-a5f9d1284d78e2dd1db1815ee2daa9d8861bd868/> remove capability claims
 - <csr-id-005b7073e6896f68aa64348fef44ae69305acaf7/> make providers part of the workspace

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 28 commits contributed to the release over the course of 21 calendar days.
 - 22 days passed between releases.
 - 20 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump wash-cli and wash-lib alpha ([`f6e5f0e`](https://github.com/wasmCloud/wasmCloud/commit/f6e5f0e804d4a7eced93778b739bf58c30ad75e7))
    - Use config option when getting project config ([`ccbff56`](https://github.com/wasmCloud/wasmCloud/commit/ccbff56712dd96d0661538b489cb9fddff10f4ec))
    - Remove cluster_seed/cluster_issuers ([`bc5d296`](https://github.com/wasmCloud/wasmCloud/commit/bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f))
    - Expect wit-bindgen-go 0.24.0 files ([`c1cf682`](https://github.com/wasmCloud/wasmCloud/commit/c1cf682972ac9e6fa544ed79857c18d2b62ccfb8))
    - Revert "(WIP): modified the default key_directory for wash build" ([`8b7be7e`](https://github.com/wasmCloud/wasmCloud/commit/8b7be7ee63cfe2a9b054368d9b449850e7f076c3))
    - Revert "WIP: modified the default key_directory to user's /home/sidconstructs directory and modified test cases" ([`804cadf`](https://github.com/wasmCloud/wasmCloud/commit/804cadf517523f7e38d3946793269885b19bb875))
    - Revert "WIP: removed debug line" ([`d25d906`](https://github.com/wasmCloud/wasmCloud/commit/d25d90612070c59a9accf6910743c769f8ed5cb1))
    - Revert "fix(wash-lib): changed the variable name for a cleaner code" ([`c111841`](https://github.com/wasmCloud/wasmCloud/commit/c1118415f796b5c6e2931c1f365c8ee040f5ca57))
    - Changed the variable name for a cleaner code ([`edc660d`](https://github.com/wasmCloud/wasmCloud/commit/edc660de3eb9181ebaa4fce158089a9ad625e891))
    - Removed debug line ([`ddf25d9`](https://github.com/wasmCloud/wasmCloud/commit/ddf25d917dc241d6c5468796bca97a4c70b0d1d2))
    - Modified the default key_directory to user's /home/sidconstructs directory and modified test cases ([`2e93989`](https://github.com/wasmCloud/wasmCloud/commit/2e93989bf14b223b689f77cb4139275094debae4))
    - (WIP): modified the default key_directory for wash build ([`cd901db`](https://github.com/wasmCloud/wasmCloud/commit/cd901db88344f959bbe551612f03f44a4b0a109c))
    - Rename scaled ID from actor to component ([`91c57b2`](https://github.com/wasmCloud/wasmCloud/commit/91c57b238c6e3aec5bd86f5c2103aaec21932725))
    - Update start/stop provider events ([`b6dd820`](https://github.com/wasmCloud/wasmCloud/commit/b6dd820c45f7ea0f62c8cb91adb7074c5e8c0113))
    - Pin ctl to workspace ([`0e0acd7`](https://github.com/wasmCloud/wasmCloud/commit/0e0acd728df340f4f4ae0ea31e47abaecb5b3907))
    - Rename ctl actor to component ([`9018c03`](https://github.com/wasmCloud/wasmCloud/commit/9018c03b0bd517c4c2f7fe643c4d510a5823bfb8))
    - Pin to ctl v1.0.0-alpha.2 ([`fe50175`](https://github.com/wasmCloud/wasmCloud/commit/fe50175294867bc8c9d109d8d610b0453fd65a1c))
    - Rename actor_scale* events ([`9e23be2`](https://github.com/wasmCloud/wasmCloud/commit/9e23be23131bbcdad746f7e85d33d5812e5f2ff9))
    - Cleanup and fix tests. ([`c2ceee0`](https://github.com/wasmCloud/wasmCloud/commit/c2ceee0a5ed26526b3e3b026ec3762fefe049da5))
    - Consolidate wash stop host and wash down functions. ([`4b1e420`](https://github.com/wasmCloud/wasmCloud/commit/4b1e420f866961365bf20aff3d63a7fb6cb911e3))
    - Use pid to determine if host is running. ([`13198bb`](https://github.com/wasmCloud/wasmCloud/commit/13198bb9625f32363fdfb6a541ae10b649ea3e57))
    - Remove capability claims ([`a5f9d12`](https://github.com/wasmCloud/wasmCloud/commit/a5f9d1284d78e2dd1db1815ee2daa9d8861bd868))
    - Remove usage of capability signing ([`93748a1`](https://github.com/wasmCloud/wasmCloud/commit/93748a1ecd4edd785af257952f1de9497a7ea946))
    - Updates wash to use new host version ([`3a96d28`](https://github.com/wasmCloud/wasmCloud/commit/3a96d288714b14f1d8bab831ef4d0f9533204f56))
    - Adds version to control API ([`3f2d2f4`](https://github.com/wasmCloud/wasmCloud/commit/3f2d2f44470d44809fb83de2fa34b29ad1e6cb30))
    - Use native TLS roots along webpki ([`07b5e70`](https://github.com/wasmCloud/wasmCloud/commit/07b5e70a7f1321d184962d7197a8d98d1ecaaf71))
    - Address clippy warnings, simplify ([`65ff33f`](https://github.com/wasmCloud/wasmCloud/commit/65ff33fe473425fffb320309921dfbdcb7c8f868))
    - Make providers part of the workspace ([`005b707`](https://github.com/wasmCloud/wasmCloud/commit/005b7073e6896f68aa64348fef44ae69305acaf7))
</details>

## v0.19.0 (2024-03-17)

<csr-id-30651406b56838afc9620f5fe5019a40a8908a48/>
<csr-id-888400046df8a1a636f42c9fb498d6d42331bcf2/>
<csr-id-0eeb815d9363f2979a2128593b52b3c3fd3cb699/>
<csr-id-37fbe7f3bf41ce6d290f0b28ecb7d75b7595f961/>

### Chore

 - <csr-id-30651406b56838afc9620f5fe5019a40a8908a48/> bump to 0.19
 - <csr-id-888400046df8a1a636f42c9fb498d6d42331bcf2/> rename actor to component
 - <csr-id-0eeb815d9363f2979a2128593b52b3c3fd3cb699/> add trace logs to aid in provider build debugging

### New Features

 - <csr-id-1a8d80b28a36c75424a071a4d785acf05516bc62/> validate user input component ids
 - <csr-id-76c1ed7b5c49152aabd83d27f0b8955d7f874864/> support pubsub on wRPC subjects
   Up until now, publishing and subscribing for RPC communcations on the
   NATS cluster happened on subjects that were related to the wasmbus
   protocol (i.e. 'wasmbus.rpc.*').
   
   To support the WIT-native invocations, i.e. wRPC (#1389), we must
   change the publication and subscription subjects to include also the
   subjects that are expected to be used by wprc.
   
   This commit updates the provider-sdk to listen *additionally* to
   subjects that are required/used by wrpc, though we do not yet have an
   implementation for encode/deocde.
 - <csr-id-5d19ba16a98dca9439628e8449309ccaa763ab10/> change set-target to set-link-name
   Up until the relatively low-level `wasmcloud:bus/lattice` WIT
   interface has used a function called `set-target` to aim invocations
   that occurred in compliant actors and providers.
   
   Since wRPC (#1389)
   enabled  wasmCloud 1.0 is going to be WIT-first going forward, all
   WIT-driven function executions have access to the relevant
   interface (WIT interfaces, rather than Smithy-derived ones) that they
   call, at call time.
   
   Given that actor & provider side function executions have access to
   their WIT interfaces (ex. `wasi:keyvalue/readwrite.get`), what we need
   to do is differentiate between the case where *multiple targets*
   might be responding to the same WIT interface-backed invocations.
   
   Unlike before, `set-target` only needs to really differentiate between *link
   names*.
   
   This commit updates `set-target` to perform differentiate between link
   names, building on the work already done to introduce more opaque
   targeting via Component IDs.

### Bug Fixes

 - <csr-id-b4ee385cf4633f355abe38f8e4f422bb46bffea3/> build provider tests

### Test

 - <csr-id-37fbe7f3bf41ce6d290f0b28ecb7d75b7595f961/> update tests to validate new apis

### New Features (BREAKING)

 - <csr-id-18de48d9664324916ee9aaa75478f1990d1bce25/> implement config subcommand
 - <csr-id-8cbfeef8dea590b15446ec29b66e7008e0e717f1/> update CLI and lib to to be 1.0 compatible
 - <csr-id-dde2bffb57a6a6b1d3cb8bfb987f7aa92f25ac44/> update wash-lib to 1.0 ctliface
 - <csr-id-25d8f5bc4d43fb3a05c871bf367a7ac14b247f79/> implement wash building provider for host machine
 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 15 commits contributed to the release over the course of 30 calendar days.
 - 31 days passed between releases.
 - 13 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump to 0.19 ([`3065140`](https://github.com/wasmCloud/wasmCloud/commit/30651406b56838afc9620f5fe5019a40a8908a48))
    - Implement config subcommand ([`18de48d`](https://github.com/wasmCloud/wasmCloud/commit/18de48d9664324916ee9aaa75478f1990d1bce25))
    - Validate user input component ids ([`1a8d80b`](https://github.com/wasmCloud/wasmCloud/commit/1a8d80b28a36c75424a071a4d785acf05516bc62))
    - Update tests to validate new apis ([`37fbe7f`](https://github.com/wasmCloud/wasmCloud/commit/37fbe7f3bf41ce6d290f0b28ecb7d75b7595f961))
    - Update CLI and lib to to be 1.0 compatible ([`8cbfeef`](https://github.com/wasmCloud/wasmCloud/commit/8cbfeef8dea590b15446ec29b66e7008e0e717f1))
    - Rename actor to component ([`8884000`](https://github.com/wasmCloud/wasmCloud/commit/888400046df8a1a636f42c9fb498d6d42331bcf2))
    - Update wash-lib to 1.0 ctliface ([`dde2bff`](https://github.com/wasmCloud/wasmCloud/commit/dde2bffb57a6a6b1d3cb8bfb987f7aa92f25ac44))
    - Add trace logs to aid in provider build debugging ([`0eeb815`](https://github.com/wasmCloud/wasmCloud/commit/0eeb815d9363f2979a2128593b52b3c3fd3cb699))
    - Build provider tests ([`b4ee385`](https://github.com/wasmCloud/wasmCloud/commit/b4ee385cf4633f355abe38f8e4f422bb46bffea3))
    - Implement wash building provider for host machine ([`25d8f5b`](https://github.com/wasmCloud/wasmCloud/commit/25d8f5bc4d43fb3a05c871bf367a7ac14b247f79))
    - Support pubsub on wRPC subjects ([`76c1ed7`](https://github.com/wasmCloud/wasmCloud/commit/76c1ed7b5c49152aabd83d27f0b8955d7f874864))
    - Change set-target to set-link-name ([`5d19ba1`](https://github.com/wasmCloud/wasmCloud/commit/5d19ba16a98dca9439628e8449309ccaa763ab10))
    - Update the list of modules behind the nats flag ([`1d53c2e`](https://github.com/wasmCloud/wasmCloud/commit/1d53c2e00204504c94bc65d4ae3fc16b03168e10))
    - Fix the build problem of `wash-lib` with `--no-default-features` flag. ([`5079c2e`](https://github.com/wasmCloud/wasmCloud/commit/5079c2ea9cc2c215334913fd415880c42063fa6c))
    - Updates topics to the new standard ([`42d069e`](https://github.com/wasmCloud/wasmCloud/commit/42d069eee87d1b5befff1a95b49973064f1a1d1b))
</details>

## v0.18.1 (2024-02-14)

<csr-id-2ce174ce1101c2267272ed24ada64e40104d40f7/>

### Other

 - <csr-id-2ce174ce1101c2267272ed24ada64e40104d40f7/> v0.18.1

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - V0.18.1 ([`2ce174c`](https://github.com/wasmCloud/wasmCloud/commit/2ce174ce1101c2267272ed24ada64e40104d40f7))
</details>

## v0.18.0 (2024-02-13)

<csr-id-d0131409087f6a461072b83d22e4263653eca8ba/>
<csr-id-7d263b9930a710b4d809372a8844a365a9bb2b73/>
<csr-id-18dfd452d798513c7c4b56b26191a61cd913297e/>

### Chore

 - <csr-id-d0131409087f6a461072b83d22e4263653eca8ba/> fix clippy warning
 - <csr-id-7d263b9930a710b4d809372a8844a365a9bb2b73/> fix format

### New Features

 - <csr-id-8cdd687d20a04ccbd3f812cc6748004fa2089778/> update favorites to use components
 - <csr-id-7c4a2be53a68c42af9cb36807f3acc1bd965e8f5/> Better scale message

### Bug Fixes

 - <csr-id-8b876f1533dac0b622a835d2c883d338addbb172/> windows path to target
   Verbatim paths on Windows are not well supported,
   e.g. "\\\\?\\C:\\Users..." while technically valid, causes some fs api's like `exists` to fail errantly.
   
   The fix is to use a third party lib normpath to normalize the path to the wasm
   binary.
   
   Related issue: https://github.com/rust-lang/cargo/issues/9770
 - <csr-id-f5a4ff1a580494e2b6deb3c35c9868981fae08d8/> pipe child output of custom build command to parent

### Other

 - <csr-id-18dfd452d798513c7c4b56b26191a61cd913297e/> v0.18.0

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 7 commits contributed to the release over the course of 15 calendar days.
 - 17 days passed between releases.
 - 7 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Update favorites to use components ([`8cdd687`](https://github.com/wasmCloud/wasmCloud/commit/8cdd687d20a04ccbd3f812cc6748004fa2089778))
    - Fix clippy warning ([`d013140`](https://github.com/wasmCloud/wasmCloud/commit/d0131409087f6a461072b83d22e4263653eca8ba))
    - Fix format ([`7d263b9`](https://github.com/wasmCloud/wasmCloud/commit/7d263b9930a710b4d809372a8844a365a9bb2b73))
    - Windows path to target ([`8b876f1`](https://github.com/wasmCloud/wasmCloud/commit/8b876f1533dac0b622a835d2c883d338addbb172))
    - V0.18.0 ([`18dfd45`](https://github.com/wasmCloud/wasmCloud/commit/18dfd452d798513c7c4b56b26191a61cd913297e))
    - Better scale message ([`7c4a2be`](https://github.com/wasmCloud/wasmCloud/commit/7c4a2be53a68c42af9cb36807f3acc1bd965e8f5))
    - Pipe child output of custom build command to parent ([`f5a4ff1`](https://github.com/wasmCloud/wasmCloud/commit/f5a4ff1a580494e2b6deb3c35c9868981fae08d8))
</details>

## v0.17.0 (2024-01-26)

<csr-id-1793dc9296b7e161a8efe42bd7e5717bd6687da8/>
<csr-id-7f700611a60da3848afa9007bc0d2a1b4fcab946/>
<csr-id-e0093e594fed3740bec38259c9b0c499eedf9e00/>
<csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/>

### Chore

 - <csr-id-1793dc9296b7e161a8efe42bd7e5717bd6687da8/> replace env_logger with tracing_subscriber
 - <csr-id-7f700611a60da3848afa9007bc0d2a1b4fcab946/> bump NATS server version

### New Features

 - <csr-id-9550bf1a24d7c5d1a70d03e5a6244a718c49719a/> subscribe to receive specific events
 - <csr-id-5dac7aff84e57eaf5d2f6cf5f0e3bc7848e284d6/> support other build languages
 - <csr-id-1ad43c4dfddf411107c0d63358a9c8779339bb99/> add label command to set and remove host labels

### Bug Fixes

 - <csr-id-d1082472ca70e8660faea488c99edc97a4c428f8/> listen to deprecated start/stop events
 - <csr-id-35f28ab487d0937b14f57358b41ef7cdb1b63310/> fix spelling mistake from previous PR
   This commit fixes a tiny typo that was left out of a preivous PR (#1246)
 - <csr-id-e9213de7b6d1a5584884d47a93e8d35d672bc680/> only generate tinygo when wit-dir present
   Golang projects built by wash which have
   `wasm32-wasi-preview1` set as their `wasm_target` (in
   `wasmcloud.toml`) fail to build due to go
   bindgen (i.e. `wit-bindgen-go`) being run on them.
   
   While the wasmcloud ecosystem is WIT-first, it is possible to build
   preview1/preview2 components *without* WIT (i.e. with the legacy
   Smithy ecosystem), and projects that are built in that way should not
   have bindgen run on them.
   
   This commit improves the check to use `wit_world` to determine
   whether to run go-based bindgen.

### Other

 - <csr-id-e0093e594fed3740bec38259c9b0c499eedf9e00/> v0.17.0

### New Features (BREAKING)

 - <csr-id-8863f14f00dcde3c6a299551e7dfbca7867843dc/> allow relative paths in file-based WADM manifests
   WADM does not allow non-relative file paths to be used for values like
   `image:` (which is relevant for actors and providers specified in the manifest).
   
   If a user is using a local file path, it's very likely that the host
   on which the declarative architecture will be deployed is the same
   host as the one that is running `wadm`.
   
   To enable users to more conveniently build declarative manifests, we
   can resolve `file://...` paths based on the path to the WADM file
   itself (which is known at load time).
   
   The basic scheme is to update the `AppManifest`s to store YAML structure rather
   than a simple `String`, in order to enable iterating and replacing
   paths as is necessary.
   
   This commit allows for relative paths in WADM manifests that are fed
   to commands like `wash app deploy`.
 - <csr-id-df01bbd89fd2b690c2d1bcfe68455fb827646a10/> remove singular actor events, add actor_scaled
 - <csr-id-5cca9ee0a88d63cb53e8d352c16a5d9d59966bc8/> upgrade max_instances to u32
 - <csr-id-d8eb9f3ee9df65e96d076a6ba11d2600d0513207/> rename max-concurrent to max-instances, simplify scale

### Refactor (BREAKING)

 - <csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/> rename lattice prefix to just lattice

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 14 commits contributed to the release over the course of 24 calendar days.
 - 29 days passed between releases.
 - 14 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Listen to deprecated start/stop events ([`d108247`](https://github.com/wasmCloud/wasmCloud/commit/d1082472ca70e8660faea488c99edc97a4c428f8))
    - V0.17.0 ([`e0093e5`](https://github.com/wasmCloud/wasmCloud/commit/e0093e594fed3740bec38259c9b0c499eedf9e00))
    - Subscribe to receive specific events ([`9550bf1`](https://github.com/wasmCloud/wasmCloud/commit/9550bf1a24d7c5d1a70d03e5a6244a718c49719a))
    - Allow relative paths in file-based WADM manifests ([`8863f14`](https://github.com/wasmCloud/wasmCloud/commit/8863f14f00dcde3c6a299551e7dfbca7867843dc))
    - Rename lattice prefix to just lattice ([`6e8faab`](https://github.com/wasmCloud/wasmCloud/commit/6e8faab6a6e9f9bb7327ffb71ded2a83718920f7))
    - Support other build languages ([`5dac7af`](https://github.com/wasmCloud/wasmCloud/commit/5dac7aff84e57eaf5d2f6cf5f0e3bc7848e284d6))
    - Remove singular actor events, add actor_scaled ([`df01bbd`](https://github.com/wasmCloud/wasmCloud/commit/df01bbd89fd2b690c2d1bcfe68455fb827646a10))
    - Upgrade max_instances to u32 ([`5cca9ee`](https://github.com/wasmCloud/wasmCloud/commit/5cca9ee0a88d63cb53e8d352c16a5d9d59966bc8))
    - Rename max-concurrent to max-instances, simplify scale ([`d8eb9f3`](https://github.com/wasmCloud/wasmCloud/commit/d8eb9f3ee9df65e96d076a6ba11d2600d0513207))
    - Add label command to set and remove host labels ([`1ad43c4`](https://github.com/wasmCloud/wasmCloud/commit/1ad43c4dfddf411107c0d63358a9c8779339bb99))
    - Fix spelling mistake from previous PR ([`35f28ab`](https://github.com/wasmCloud/wasmCloud/commit/35f28ab487d0937b14f57358b41ef7cdb1b63310))
    - Replace env_logger with tracing_subscriber ([`1793dc9`](https://github.com/wasmCloud/wasmCloud/commit/1793dc9296b7e161a8efe42bd7e5717bd6687da8))
    - Only generate tinygo when wit-dir present ([`e9213de`](https://github.com/wasmCloud/wasmCloud/commit/e9213de7b6d1a5584884d47a93e8d35d672bc680))
    - Bump NATS server version ([`7f70061`](https://github.com/wasmCloud/wasmCloud/commit/7f700611a60da3848afa9007bc0d2a1b4fcab946))
</details>

## v0.16.0 (2023-12-28)

<csr-id-c12eff1597e444fcd926dbfb0abab547b2efc2b0/>
<csr-id-b0fdf60a33d6866a92924b02e5e2ae8544e421a5/>
<csr-id-fc10788b9443b374c973123ba71d5b06e6c62a12/>
<csr-id-ff2e832af25c27a297435cc64d48768df5469a78/>
<csr-id-25af017f69652a98b8969609e2854636e2bc7553/>
<csr-id-7bc207bf24873e5d916edf7e8a4b56c7ed04b9a7/>
<csr-id-547ed475038a7322aae12183bafc8a7e25aa8753/>
<csr-id-9476b9100efc86c06be614bb6c263ff0ee2354d6/>
<csr-id-e1c00a3cfa6a7f226f19f6ba082d71fe70f3f5cb/>
<csr-id-087b5c326886465a3370affdbbcfcb9d5628aaf1/>
<csr-id-75c0739a4db4264996a7fa87ce3ae39f56780759/>
<csr-id-3e744b553abeff5beb7e71116ccec7c164801353/>
<csr-id-189fdf8695e62a8ba842322ccd7ff30e45dbfb5f/>
<csr-id-44509720d3eee62c05237d86d5f4baef55e35809/>
<csr-id-cfc002bf206e2507848c1b277a7cce5231c324c9/>
<csr-id-7de31820034c4b70ab6edc772713e64aafe294a9/>
<csr-id-57d014fb7fe11542d2e64068ba86e42a19f64f98/>
<csr-id-4e9bae34fe95ecaffbc81fd452bf29746b4e5856/>
<csr-id-e58d3579b9e3cd2637d8dcbe37038172d3ca4c22/>

### Chore

 - <csr-id-c12eff1597e444fcd926dbfb0abab547b2efc2b0/> update wasmcloud version to 0.81
 - <csr-id-b0fdf60a33d6866a92924b02e5e2ae8544e421a5/> pin wasmcloud version to 0.81-rc1
 - <csr-id-fc10788b9443b374c973123ba71d5b06e6c62a12/> bump wash-lib to 0.16
 - <csr-id-ff2e832af25c27a297435cc64d48768df5469a78/> revert `wash` adapter update
 - <csr-id-25af017f69652a98b8969609e2854636e2bc7553/> replace broken URLs
 - <csr-id-7bc207bf24873e5d916edf7e8a4b56c7ed04b9a7/> refactor command parsing for readability
 - <csr-id-547ed475038a7322aae12183bafc8a7e25aa8753/> do not enable new component encoding

### New Features

 - <csr-id-d91e92b7bd32a23804cafc4381e7648a151ace38/> prefix absolute path references with file://
 - <csr-id-bae6a00390e2ac10eaede2966d060477b7091697/> enable only signing actors

### Bug Fixes

 - <csr-id-c7270fd9ba3f3af0b94606dc69b6d9c4b8d27869/> claims signing shouldn't require a wasmcloud.toml file.
 - <csr-id-edc1fa5c2404d41c9d0064ece82b328c1ea016b9/> only embed metadata in tinygo modules
 - <csr-id-5f3850fca40fc037e371f2da17d35645c12f4b2c/> fix generating from git branch
 - <csr-id-a63d565aef1a4026a3bb436eb2519baf84b64b4c/> enable docs feature when building for docs.rs
 - <csr-id-7fac3db70f2cf8c794dacdfe06e4ac5b17144821/> remove object file from expected test
 - <csr-id-98b7a5522600829dcf575204381077f3efc9091d/> remove unused import

### Other

 - <csr-id-9476b9100efc86c06be614bb6c263ff0ee2354d6/> fix typo in test file; fix assert statements
 - <csr-id-e1c00a3cfa6a7f226f19f6ba082d71fe70f3f5cb/> fix unit test failling due to wrong expected value
 - <csr-id-087b5c326886465a3370affdbbcfcb9d5628aaf1/> update adapters
 - <csr-id-75c0739a4db4264996a7fa87ce3ae39f56780759/> update to wasmtime 16
   Note this uses a release branch as 16 is not out yet.

### Refactor

 - <csr-id-3e744b553abeff5beb7e71116ccec7c164801353/> project config overrides for claims commands
 - <csr-id-189fdf8695e62a8ba842322ccd7ff30e45dbfb5f/> simplify nkey directory path derivation logic
 - <csr-id-44509720d3eee62c05237d86d5f4baef55e35809/> make wash claims aware of wasmcloud.toml
 - <csr-id-cfc002bf206e2507848c1b277a7cce5231c324c9/> update golang example to wasmtime 16
   With the fast-moving development of WebAssembly ecosystem, WASI, the
   Component Model, and WIT have seen many changes in the last couple months.
   
   For example, The existing golang echo example in the repo was
   originally built when wit-bindgen version 0.13.1 was the most
   important version, and upstream wit-bindgen is now at 0.16.0. As
   wit-bindgen reflects releases of wasmtime and the ecosystem as a
   whole, there's been a lot of sales.
   
   This commit updates the golang example echo actor to use the
   WIT and related generated bindings for newer versions of wasmtime 16
   and related WIT definitions, including resources.
 - <csr-id-7de31820034c4b70ab6edc772713e64aafe294a9/> remove deprecated code related to start actor cmd
 - <csr-id-57d014fb7fe11542d2e64068ba86e42a19f64f98/> revised implementation of registry url and credentials resolution
 - <csr-id-4e9bae34fe95ecaffbc81fd452bf29746b4e5856/> some cleanup before revised implementation

### Test

 - <csr-id-e58d3579b9e3cd2637d8dcbe37038172d3ca4c22/> remove vestigial actor refresh function call in dev setup

### New Features (BREAKING)

 - <csr-id-b0e6c1f167c9c2e06750d72f10dc729d17f0b81a/> force minimum wasmCloud version to 0.81
 - <csr-id-a86415712621504b820b8c4d0b71017b7140470b/> add support for inspecting wit
 - <csr-id-57eec5cd08ec4ee589d00ee5984bf1b63abefc12/> Add support for model.status wadm command in wash-lib
 - <csr-id-023307fcb351a67fe2271862ace8657ac0e101b6/> add support for custom build command

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 31 commits contributed to the release over the course of 35 calendar days.
 - 37 days passed between releases.
 - 31 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Update wasmcloud version to 0.81 ([`c12eff1`](https://github.com/wasmCloud/wasmCloud/commit/c12eff1597e444fcd926dbfb0abab547b2efc2b0))
    - Fix typo in test file; fix assert statements ([`9476b91`](https://github.com/wasmCloud/wasmCloud/commit/9476b9100efc86c06be614bb6c263ff0ee2354d6))
    - Fix unit test failling due to wrong expected value ([`e1c00a3`](https://github.com/wasmCloud/wasmCloud/commit/e1c00a3cfa6a7f226f19f6ba082d71fe70f3f5cb))
    - Project config overrides for claims commands ([`3e744b5`](https://github.com/wasmCloud/wasmCloud/commit/3e744b553abeff5beb7e71116ccec7c164801353))
    - Claims signing shouldn't require a wasmcloud.toml file. ([`c7270fd`](https://github.com/wasmCloud/wasmCloud/commit/c7270fd9ba3f3af0b94606dc69b6d9c4b8d27869))
    - Simplify nkey directory path derivation logic ([`189fdf8`](https://github.com/wasmCloud/wasmCloud/commit/189fdf8695e62a8ba842322ccd7ff30e45dbfb5f))
    - Make wash claims aware of wasmcloud.toml ([`4450972`](https://github.com/wasmCloud/wasmCloud/commit/44509720d3eee62c05237d86d5f4baef55e35809))
    - Prefix absolute path references with file:// ([`d91e92b`](https://github.com/wasmCloud/wasmCloud/commit/d91e92b7bd32a23804cafc4381e7648a151ace38))
    - Only embed metadata in tinygo modules ([`edc1fa5`](https://github.com/wasmCloud/wasmCloud/commit/edc1fa5c2404d41c9d0064ece82b328c1ea016b9))
    - Force minimum wasmCloud version to 0.81 ([`b0e6c1f`](https://github.com/wasmCloud/wasmCloud/commit/b0e6c1f167c9c2e06750d72f10dc729d17f0b81a))
    - Pin wasmcloud version to 0.81-rc1 ([`b0fdf60`](https://github.com/wasmCloud/wasmCloud/commit/b0fdf60a33d6866a92924b02e5e2ae8544e421a5))
    - Bump wash-lib to 0.16 ([`fc10788`](https://github.com/wasmCloud/wasmCloud/commit/fc10788b9443b374c973123ba71d5b06e6c62a12))
    - Fix generating from git branch ([`5f3850f`](https://github.com/wasmCloud/wasmCloud/commit/5f3850fca40fc037e371f2da17d35645c12f4b2c))
    - Update adapters ([`087b5c3`](https://github.com/wasmCloud/wasmCloud/commit/087b5c326886465a3370affdbbcfcb9d5628aaf1))
    - Enable docs feature when building for docs.rs ([`a63d565`](https://github.com/wasmCloud/wasmCloud/commit/a63d565aef1a4026a3bb436eb2519baf84b64b4c))
    - Update golang example to wasmtime 16 ([`cfc002b`](https://github.com/wasmCloud/wasmCloud/commit/cfc002bf206e2507848c1b277a7cce5231c324c9))
    - Add support for inspecting wit ([`a864157`](https://github.com/wasmCloud/wasmCloud/commit/a86415712621504b820b8c4d0b71017b7140470b))
    - Remove object file from expected test ([`7fac3db`](https://github.com/wasmCloud/wasmCloud/commit/7fac3db70f2cf8c794dacdfe06e4ac5b17144821))
    - Revert `wash` adapter update ([`ff2e832`](https://github.com/wasmCloud/wasmCloud/commit/ff2e832af25c27a297435cc64d48768df5469a78))
    - Update to wasmtime 16 ([`75c0739`](https://github.com/wasmCloud/wasmCloud/commit/75c0739a4db4264996a7fa87ce3ae39f56780759))
    - Remove unused import ([`98b7a55`](https://github.com/wasmCloud/wasmCloud/commit/98b7a5522600829dcf575204381077f3efc9091d))
    - Remove vestigial actor refresh function call in dev setup ([`e58d357`](https://github.com/wasmCloud/wasmCloud/commit/e58d3579b9e3cd2637d8dcbe37038172d3ca4c22))
    - Remove deprecated code related to start actor cmd ([`7de3182`](https://github.com/wasmCloud/wasmCloud/commit/7de31820034c4b70ab6edc772713e64aafe294a9))
    - Add support for model.status wadm command in wash-lib ([`57eec5c`](https://github.com/wasmCloud/wasmCloud/commit/57eec5cd08ec4ee589d00ee5984bf1b63abefc12))
    - Revised implementation of registry url and credentials resolution ([`57d014f`](https://github.com/wasmCloud/wasmCloud/commit/57d014fb7fe11542d2e64068ba86e42a19f64f98))
    - Some cleanup before revised implementation ([`4e9bae3`](https://github.com/wasmCloud/wasmCloud/commit/4e9bae34fe95ecaffbc81fd452bf29746b4e5856))
    - Replace broken URLs ([`25af017`](https://github.com/wasmCloud/wasmCloud/commit/25af017f69652a98b8969609e2854636e2bc7553))
    - Refactor command parsing for readability ([`7bc207b`](https://github.com/wasmCloud/wasmCloud/commit/7bc207bf24873e5d916edf7e8a4b56c7ed04b9a7))
    - Add support for custom build command ([`023307f`](https://github.com/wasmCloud/wasmCloud/commit/023307fcb351a67fe2271862ace8657ac0e101b6))
    - Enable only signing actors ([`bae6a00`](https://github.com/wasmCloud/wasmCloud/commit/bae6a00390e2ac10eaede2966d060477b7091697))
    - Do not enable new component encoding ([`547ed47`](https://github.com/wasmCloud/wasmCloud/commit/547ed475038a7322aae12183bafc8a7e25aa8753))
</details>

## v0.15.0 (2023-11-21)

<csr-id-000299c4d3e8488bca3722ac40695d5e78bf92c8/>
<csr-id-4adbf0647f1ef987e92fbf927db9d09e64d3ecd8/>
<csr-id-267d24dcdc871bbc85c0adc0d102a632310bb9f0/>

### Documentation

 - <csr-id-20ffecb027c225fb62d60b584d6b518aff4ceb51/> update wash URLs

### New Features

 - <csr-id-91dfdfe68ddb5e65fbeb9061e82b685942c7a807/> support RISCV64

### Other

 - <csr-id-000299c4d3e8488bca3722ac40695d5e78bf92c8/> v0.15.0
 - <csr-id-4adbf0647f1ef987e92fbf927db9d09e64d3ecd8/> update `async-nats` to 0.33

### Test

 - <csr-id-267d24dcdc871bbc85c0adc0d102a632310bb9f0/> add integration test for wash-call
   This commit adds a test for `wash call` functionality, as a fix was
   recently landed that re-enabled it's use.

### New Features (BREAKING)

 - <csr-id-ce7904e6f4cc49ca92ec8dee8e263d23da26afd0/> Removes need for actor/provider/host IDs in almost all cases
   This is something that has been bugging me for a while. It has been such a
   pain to look up and copy paste all the proper IDs to run various wash commands.
   
   This PR is a breaking change for several commands (like stop provider) and makes
   it so you can pass a string that it will attempt to match on to find IDs

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 6 commits contributed to the release over the course of 6 calendar days.
 - 7 days passed between releases.
 - 6 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - V0.15.0 ([`000299c`](https://github.com/wasmCloud/wasmCloud/commit/000299c4d3e8488bca3722ac40695d5e78bf92c8))
    - Support RISCV64 ([`91dfdfe`](https://github.com/wasmCloud/wasmCloud/commit/91dfdfe68ddb5e65fbeb9061e82b685942c7a807))
    - Removes need for actor/provider/host IDs in almost all cases ([`ce7904e`](https://github.com/wasmCloud/wasmCloud/commit/ce7904e6f4cc49ca92ec8dee8e263d23da26afd0))
    - Add integration test for wash-call ([`267d24d`](https://github.com/wasmCloud/wasmCloud/commit/267d24dcdc871bbc85c0adc0d102a632310bb9f0))
    - Update wash URLs ([`20ffecb`](https://github.com/wasmCloud/wasmCloud/commit/20ffecb027c225fb62d60b584d6b518aff4ceb51))
    - Update `async-nats` to 0.33 ([`4adbf06`](https://github.com/wasmCloud/wasmCloud/commit/4adbf0647f1ef987e92fbf927db9d09e64d3ecd8))
</details>

## v0.14.0 (2023-11-14)

<csr-id-7166f540aa4c75a379720da8120d91eb1c06be8f/>
<csr-id-39a9e218418a0662de4edabbc9078268ba095842/>
<csr-id-8e071dde1a98caa7339e92882bb63c433ae2a042/>
<csr-id-9c8abf3dd1a942f01a70432abb2fb9cfc4d48914/>
<csr-id-d43d300929465a640e03e4805eb2583262e4642d/>
<csr-id-cbc9ed7008f8969312534e326cf119dbbdf89aaa/>
<csr-id-21db64c7a2fd0f07341ac795795a1615d37eb521/>
<csr-id-248e9d3ac60fdd2b380723e9bbaf1cc8023beb44/>
<csr-id-cb4d311c6d666e59c22199f950757abc65167f53/>
<csr-id-7d6155e62512e6909379bbed5e73abe219838e4b/>
<csr-id-9bf9accbcefa3e852c3b62290c14ee5e71731530/>
<csr-id-30b835d82555967b5abfc7bf3f9d000f87ed5043/>
<csr-id-9da236f1e82ca086accd30bf32d4dd8a4829a1c9/>
<csr-id-e2927c69e2f6269b14a2cb0cf6df5db4b9f5b25c/>
<csr-id-42ccacee8bd3cddf4b4354e10aabd0a345b3c62f/>

### Chore

 - <csr-id-7166f540aa4c75a379720da8120d91eb1c06be8f/> better syntax
 - <csr-id-39a9e218418a0662de4edabbc9078268ba095842/> use with_context for lazy eval
 - <csr-id-8e071dde1a98caa7339e92882bb63c433ae2a042/> remove direct `wasmbus_rpc` dependency
 - <csr-id-9c8abf3dd1a942f01a70432abb2fb9cfc4d48914/> address clippy issues

### Bug Fixes

 - <csr-id-c7b2a1dd9f96542982fd8e4f188eca374d51db7d/> allow specifying --nats-remote-url without --nats-credsfile
 - <csr-id-70ac131767572f757fca6c37cdc428f40212bc6f/> proper derivation of lattice_prefix (ie, lattice_prefix arg > context arg > $current_default context.lattice_prefix)
 - <csr-id-7da3e833b80343d0faa6fbd49906b294d0cfc5e9/> ensure expected behavior when creating/switching context
 - <csr-id-4fb8118f8fd74a4baf8019f3ab6c6cea2fd1c889/> require revision and version args on sign cmd
 - <csr-id-8240af20678f84bdafa4d91aaf4bb577c910e2f0/> correct typo and link in README

### Other

 - <csr-id-d43d300929465a640e03e4805eb2583262e4642d/> v0.14.0

### Refactor

 - <csr-id-cbc9ed7008f8969312534e326cf119dbbdf89aaa/> always have a context
 - <csr-id-21db64c7a2fd0f07341ac795795a1615d37eb521/> use write for convenience
 - <csr-id-248e9d3ac60fdd2b380723e9bbaf1cc8023beb44/> rename new_with_dir to from_dir
 - <csr-id-cb4d311c6d666e59c22199f950757abc65167f53/> use create_nats_client_from_opts from wash-lib
 - <csr-id-7d6155e62512e6909379bbed5e73abe219838e4b/> more refactoring...
 - <csr-id-9bf9accbcefa3e852c3b62290c14ee5e71731530/> moving things around, better scopring for lattice_prefix parsing on app cmds
 - <csr-id-30b835d82555967b5abfc7bf3f9d000f87ed5043/> make revision required (w/ default) on wasmcloud.toml commong config

### Test

 - <csr-id-9da236f1e82ca086accd30bf32d4dd8a4829a1c9/> exclude test run for windows; will be dealt with in another PR.
 - <csr-id-e2927c69e2f6269b14a2cb0cf6df5db4b9f5b25c/> fix test for lattice_prefix getter
 - <csr-id-42ccacee8bd3cddf4b4354e10aabd0a345b3c62f/> rebased with upstream/main to fix failing unit test

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 21 commits contributed to the release over the course of 12 calendar days.
 - 12 days passed between releases.
 - 20 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - V0.14.0 ([`d43d300`](https://github.com/wasmCloud/wasmCloud/commit/d43d300929465a640e03e4805eb2583262e4642d))
    - Allow specifying --nats-remote-url without --nats-credsfile ([`c7b2a1d`](https://github.com/wasmCloud/wasmCloud/commit/c7b2a1dd9f96542982fd8e4f188eca374d51db7d))
    - Always have a context ([`cbc9ed7`](https://github.com/wasmCloud/wasmCloud/commit/cbc9ed7008f8969312534e326cf119dbbdf89aaa))
    - Use write for convenience ([`21db64c`](https://github.com/wasmCloud/wasmCloud/commit/21db64c7a2fd0f07341ac795795a1615d37eb521))
    - Better syntax ([`7166f54`](https://github.com/wasmCloud/wasmCloud/commit/7166f540aa4c75a379720da8120d91eb1c06be8f))
    - Rename new_with_dir to from_dir ([`248e9d3`](https://github.com/wasmCloud/wasmCloud/commit/248e9d3ac60fdd2b380723e9bbaf1cc8023beb44))
    - Use with_context for lazy eval ([`39a9e21`](https://github.com/wasmCloud/wasmCloud/commit/39a9e218418a0662de4edabbc9078268ba095842))
    - Use create_nats_client_from_opts from wash-lib ([`cb4d311`](https://github.com/wasmCloud/wasmCloud/commit/cb4d311c6d666e59c22199f950757abc65167f53))
    - Refactor!(wash-cli): initialize contexts consistently ([`703283b`](https://github.com/wasmCloud/wasmCloud/commit/703283b144a97a7e41ef67cae242ae73d85067a9))
    - Exclude test run for windows; will be dealt with in another PR. ([`9da236f`](https://github.com/wasmCloud/wasmCloud/commit/9da236f1e82ca086accd30bf32d4dd8a4829a1c9))
    - Fix test for lattice_prefix getter ([`e2927c6`](https://github.com/wasmCloud/wasmCloud/commit/e2927c69e2f6269b14a2cb0cf6df5db4b9f5b25c))
    - More refactoring... ([`7d6155e`](https://github.com/wasmCloud/wasmCloud/commit/7d6155e62512e6909379bbed5e73abe219838e4b))
    - Moving things around, better scopring for lattice_prefix parsing on app cmds ([`9bf9acc`](https://github.com/wasmCloud/wasmCloud/commit/9bf9accbcefa3e852c3b62290c14ee5e71731530))
    - Proper derivation of lattice_prefix (ie, lattice_prefix arg > context arg > $current_default context.lattice_prefix) ([`70ac131`](https://github.com/wasmCloud/wasmCloud/commit/70ac131767572f757fca6c37cdc428f40212bc6f))
    - Ensure expected behavior when creating/switching context ([`7da3e83`](https://github.com/wasmCloud/wasmCloud/commit/7da3e833b80343d0faa6fbd49906b294d0cfc5e9))
    - Remove direct `wasmbus_rpc` dependency ([`8e071dd`](https://github.com/wasmCloud/wasmCloud/commit/8e071dde1a98caa7339e92882bb63c433ae2a042))
    - Address clippy issues ([`9c8abf3`](https://github.com/wasmCloud/wasmCloud/commit/9c8abf3dd1a942f01a70432abb2fb9cfc4d48914))
    - Rebased with upstream/main to fix failing unit test ([`42ccace`](https://github.com/wasmCloud/wasmCloud/commit/42ccacee8bd3cddf4b4354e10aabd0a345b3c62f))
    - Make revision required (w/ default) on wasmcloud.toml commong config ([`30b835d`](https://github.com/wasmCloud/wasmCloud/commit/30b835d82555967b5abfc7bf3f9d000f87ed5043))
    - Require revision and version args on sign cmd ([`4fb8118`](https://github.com/wasmCloud/wasmCloud/commit/4fb8118f8fd74a4baf8019f3ab6c6cea2fd1c889))
    - Correct typo and link in README ([`8240af2`](https://github.com/wasmCloud/wasmCloud/commit/8240af20678f84bdafa4d91aaf4bb577c910e2f0))
</details>

## v0.13.0 (2023-11-01)

<csr-id-ee51a176a00b3f8fe03e0d3212a9da6dbfd6044f/>
<csr-id-a1c3b9d86db14f31ef7fbebeb30e8784f974df6f/>
<csr-id-007660e96ad7472918bc25baf9d52d60e5230823/>
<csr-id-dfad0be609868cbd0f0ce97d7d9238b41996b5fc/>
<csr-id-5ef2c4c924dbc2d93a75f99b5975b321e1bad75f/>
<csr-id-9caf89a7d15a7d8ec80a490fe0f4106089c77728/>
<csr-id-5ae8fd8bad3fadb5b97be28d5e163b621938a272/>
<csr-id-70b20a12553e84697ffe9f8dbf32219162bdf946/>
<csr-id-c44f657e3bdc1e4a6679b3cc687b7039fb729f34/>
<csr-id-016c37812b8cf95615a6ad34ee49de669c66886b/>
<csr-id-bb76aec405e437c249d385e3492cb67932960125/>
<csr-id-bbf0b1a6074108a96d9534500c97c8ad5ed13dd6/>
<csr-id-10ede9e84e537fecbad3cbbb09960506b6359ef4/>
<csr-id-a1d77b0e12ebb7b4b946004b61a208482e599ce4/>
<csr-id-2aa4b041af6195ff4dbd6bf7e04f6cba281585b9/>
<csr-id-621e449a1e70f9216016b11a6ff50c7a1def10e1/>
<csr-id-b3965d7bb04e70da967bc393b9455c4c1da6b20b/>
<csr-id-4a4c148f2e1ddb3eba535b40575265f51968ffaa/>
<csr-id-b9c23d959c5fb0a1854b8f90db6a0a0e4b1cdda9/>
<csr-id-f582dc07ea768f9b52b13c7d5c618c36e4ff0a0c/>
<csr-id-0f5add0f6e2a27d76ee63c1e387929474c93751e/>
<csr-id-37978577b218cf178fa795fb9e5326df4bd52897/>
<csr-id-e67ded670e80a19e08bcb8e6b2a25f696792ef66/>
<csr-id-f4a9cd6d2f1c29b0cc7eb4c3509114ed81eb7983/>
<csr-id-a4f67e5974c6bad70cd2d473fea7ab24371f922f/>
<csr-id-ae65e85bf4b8bcbc215d48664fcf6941d25de165/>
<csr-id-0ed956f457a94ad390b847a46df9911e5ebb35a9/>
<csr-id-80b104011536c03ef3c1c58a1440992defae1351/>
<csr-id-52ef5b6b1b6b01bc5e7a2c8fe3cbb2a08d4ad864/>
<csr-id-5af1c68bf86b62b4e2f81cbf1cc9ca1d5542ac37/>
<csr-id-372e81e2da3a60ee8cbf3f2525bf27284dc62332/>
<csr-id-571a25ddb7d8f18b2bb1d3f6b22401503d31f719/>
<csr-id-ee29478631ba0df2d67a00e3f1336b4c40099489/>
<csr-id-ddd3b072e8ec4236936c2cb53af1521ab1abeded/>
<csr-id-1495c8f3e6fdda67a90fc821a731072b72fc4062/>
<csr-id-a1e8d3f09e039723d28d738d98b47bce54e4450d/>
<csr-id-d53bf1b5e3be1cd8d076939cc80460305e30d8c5/>

### Chore

 - <csr-id-ee51a176a00b3f8fe03e0d3212a9da6dbfd6044f/> release wash-lib-v0.13.0
 - <csr-id-a1c3b9d86db14f31ef7fbebeb30e8784f974df6f/> support domain, links, keys alias
 - <csr-id-007660e96ad7472918bc25baf9d52d60e5230823/> update control interface 0.31
 - <csr-id-dfad0be609868cbd0f0ce97d7d9238b41996b5fc/> integrate `wash` into the workspace
 - <csr-id-5ef2c4c924dbc2d93a75f99b5975b321e1bad75f/> remove unused var
 - <csr-id-9caf89a7d15a7d8ec80a490fe0f4106089c77728/> update test message
 - <csr-id-5ae8fd8bad3fadb5b97be28d5e163b621938a272/> bump wash-lib and wash-cli for wit-parser fix
 - <csr-id-70b20a12553e84697ffe9f8dbf32219162bdf946/> update async_nats,ctl,wasmbus_rpc to latest
 - <csr-id-c44f657e3bdc1e4a6679b3cc687b7039fb729f34/> bump to 0.21.0, wash-lib 0.12.0
 - <csr-id-016c37812b8cf95615a6ad34ee49de669c66886b/> fix lint
 - <csr-id-bb76aec405e437c249d385e3492cb67932960125/> bump to 0.10.1 to release wadm
 - <csr-id-bbf0b1a6074108a96d9534500c97c8ad5ed13dd6/> remove references to DASHBOARD_PORT
 - <csr-id-10ede9e84e537fecbad3cbbb09960506b6359ef4/> use released wasmcloud-component-adapters
 - <csr-id-a1d77b0e12ebb7b4b946004b61a208482e599ce4/> bump wash version
 - <csr-id-2aa4b041af6195ff4dbd6bf7e04f6cba281585b9/> fix clippy warnings

### New Features

<csr-id-4144f711ad2056e9334e085cbe08663065605b0c/>
<csr-id-bb454cb3ae1ff05d8381ba2ea1f48b461d059474/>
<csr-id-02b1f03e05c4ffc7b62d2438752344cd2c805d3f/>
<csr-id-f9658287e6bdb77a6991e827454951a0711bce42/>
<csr-id-e9fe020a0906cb377f6ea8bd3a9879e5bad877b7/>
<csr-id-8c96789f1c793c5565715080b84fecfbe0653b43/>
<csr-id-e58c6a60928a7157ffbbc95f9eabcc9cae3db2a7/>
<csr-id-6923ce7efb721f8678c33f42647b87ea33a7653a/>
<csr-id-4daf51be422d395bc0142d62b8d59060b89feafa/>
<csr-id-128f7603c67443f23e76c3cb4bd1468ffd8f5462/>
<csr-id-2a6c401834b4cb55ef420538e15503b98281eaf1/>
<csr-id-24bba484009be9e87bfcbd926a731534e936c339/>
<csr-id-12cae48ff806b26b6c4f583ae00337b21bc65d3c/>
<csr-id-84b95392993cbbc65da36bc8b872241cce32a63e/>
<csr-id-a62b07b8ff321c400c6debefdb6199e273445490/>
<csr-id-d0659d346a6acadf81ce8dd952262f372c738e8d/>
<csr-id-b1bf6b1ac7851dc09e6757d7c2bde4558ec48098/>

 - <csr-id-810e220173f1ee7bf96a9ade650d26c2cd4dcb6c/> apply tags in actor config during signing
   The signing process enabled by the wasmCloud ecosystem can
   confer tags on to generated artifacts. This helps in adding metadata
   to actors and other artifacts produced by wash.
   
   This commit adds the ability to specify tags in `wasmcloud.toml` to
   `wash`, so users can more easily tag generated & signed actors
 - <csr-id-17bb1aa431f951b66b15a523032b5164893a2670/> generate golang code during wash build
   Components-first golang actors require that `go generate` be run, with
   wit-bindgen as the directive. While this is easy to do, it makes the
   build workflow (i.e. calling `wash build`) require more steps.
   
   This commit adds support for running the golang wit-bindgen
   functionality as a part of `wash build`, so that users don't have to
   call wit-bindgen themselves, or add stanzas for generate to their
   code.
   
   In the future, examples can be created that assume that the
   'generated' folder is present, and import code as necssary.
 - <csr-id-462767b950d4fd23b0961bd8a5eb5499c16bc27b/> mark components built with wash as experimental
   As the component model and WASI are still maturing, the
   components-first codebases built with `wash` should reflect the
   experimental nature of support to related tooling.
   
   This commit marks both components as experimental at two levels -- a
   custom section in the Wasm metadata (as a custom section) and as a
   tag on the signed wasmCloud actor that is produced.
 - <csr-id-5c0ccc5f872ad42b6152c66c34ab73f855f82832/> query all host inventories
 - <csr-id-109e934ceaa026f81aeadaca84e7da83668dc5fd/> add scale and update integration tests
 - <csr-id-32ea9f9eb8ba63118dfd23084d413aae23226124/> polishing app manifest loader
 - <csr-id-6907c8012fd59bbcaa6234c533b62ba997b86139/> http & stdin manifest input sources support for put & deploy cmds
 - <csr-id-99262d8b1c0bdb09657407663e2d5d4a3fb7651c/> move update-actor for wash ctl update to wash-lib.
 - <csr-id-6405f6ce45d43850ca427c4d80ca50369ee10405/> add support for Android releases
 - <csr-id-78b99fde8606febf59e30f1d12ac558b29d425bf/> set default to Rust host
   - update paths to release binary

### Bug Fixes

<csr-id-2e69e12d4b78f5ea7710ba12226345440e7541ef/>
<csr-id-5cc6ebe2b8596b5fb1a56abb4d17e4e3f104b110/>

 - <csr-id-ef3e4e584fef4d597cab0215fdf3cfe864f701e9/> Configure signing keys directory for build cmd
   The keys directory can be specified via wasmcloud.toml, CLI arguments (`--keys-directory`), or environment variable (`WASH_KEYS`).
 - <csr-id-1fa7604d3347df6c0cfb71b8ea4be6bba9bceb34/> for app manifest loading, file input source check should preceed http input source.
 - <csr-id-0eb5a7cade13a87e59c27c7f6faa89234d07863d/> some cleanup relevant to app manifest input sources
 - <csr-id-2b55ae469c07af8bd94e21f606584ef67e2e0f9a/> typo
 - <csr-id-6d71c1f36111efe1942e522c8ac6b315c78d81ab/> unify rust and tinygo component target logic
 - <csr-id-3351e0a83bc92dab8b73bc88b8d03a95dfad3e0a/> move generate key message to info log
 - <csr-id-f9279294ea7602ad6bbc55a5f3dc8940f2d46d71/> update test to reflect changes from OTP to Rust host
 - <csr-id-7111b5d9a5ece7543ded436b7816974ad27910e2/> config loading for preview2 adapter path
 - <csr-id-b0e746be713d070b4400294ec401b87444bd5741/> preserve interactive terminal when checking git
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
 - <csr-id-c7643e8b777af175d23aa66771067ccc3ee38fd3/> flaky tests
 - <csr-id-89e638a8e63073800fc952c0a874e54e9996d422/> Bumps wash-lib version
   This was missed and so cargo installing from main causes issues. Also
   bumps 0.17 so that it can pick up the new version from crates. Once this
   is published we should yank 0.17.0
 - <csr-id-656ea644696ea97bdafdbf8d5fd4a5e736593fc8/> use lib.name from cargo.toml for rust wasm binary name
   * fix(rust): read wasm binary name from cargo.toml explicitly
* fix(wash-up): grant execute permission to `mac_listener` for hot-reloading

### Other

 - <csr-id-621e449a1e70f9216016b11a6ff50c7a1def10e1/> update dependencies
 - <csr-id-b3965d7bb04e70da967bc393b9455c4c1da6b20b/> wash-lib v0.11.4
 - <csr-id-4a4c148f2e1ddb3eba535b40575265f51968ffaa/> wash-lib v0.11.3
 - <csr-id-b9c23d959c5fb0a1854b8f90db6a0a0e4b1cdda9/> wash-lib v0.11.2
 - <csr-id-f582dc07ea768f9b52b13c7d5c618c36e4ff0a0c/> wash-lib v0.11.1
 - <csr-id-0f5add0f6e2a27d76ee63c1e387929474c93751e/> v0.11.0
 - <csr-id-37978577b218cf178fa795fb9e5326df4bd52897/> Bump cargo_metadata from 0.17.0 to 0.18.0
   Bumps [cargo_metadata](https://github.com/oli-obk/cargo_metadata) from 0.17.0 to 0.18.0.
   - [Release notes](https://github.com/oli-obk/cargo_metadata/releases)
   - [Changelog](https://github.com/oli-obk/cargo_metadata/blob/main/CHANGELOG.md)
   - [Commits](https://github.com/oli-obk/cargo_metadata/compare/0.17.0...0.18.0)
   
   ---
   updated-dependencies:
   - dependency-name: cargo_metadata
     dependency-type: direct:production
     update-type: version-update:semver-minor
   ...
 - <csr-id-e67ded670e80a19e08bcb8e6b2a25f696792ef66/> wash-lib v0.10.0
 - <csr-id-f4a9cd6d2f1c29b0cc7eb4c3509114ed81eb7983/> use rc2
 - <csr-id-a4f67e5974c6bad70cd2d473fea7ab24371f922f/> Bump cargo_metadata from 0.15.4 to 0.17.0
   Bumps [cargo_metadata](https://github.com/oli-obk/cargo_metadata) from 0.15.4 to 0.17.0.
   - [Release notes](https://github.com/oli-obk/cargo_metadata/releases)
   - [Changelog](https://github.com/oli-obk/cargo_metadata/blob/main/CHANGELOG.md)
   - [Commits](https://github.com/oli-obk/cargo_metadata/compare/0.15.4...0.17.0)
   
   ---
   updated-dependencies:
   - dependency-name: cargo_metadata
     dependency-type: direct:production
     update-type: version-update:semver-minor
   ...
 - <csr-id-ae65e85bf4b8bcbc215d48664fcf6941d25de165/> v0.9.2
 - <csr-id-0ed956f457a94ad390b847a46df9911e5ebb35a9/> wash v0.16.1, wash-lib v0.6.1
 - <csr-id-80b104011536c03ef3c1c58a1440992defae1351/> adopt workspace dependencies
   This simplifies maintenance of the repository and allows for easier
   audit of the dependencies
 - <csr-id-52ef5b6b1b6b01bc5e7a2c8fe3cbb2a08d4ad864/> Creates new context library
   This creates a new context library with some extendable traits for
   loading as well as a fully featured module for handling context on
   disk.
   
   Additional tests will be in the next commit

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
 - <csr-id-ee29478631ba0df2d67a00e3f1336b4c40099489/> adjustments to app manifest loader
 - <csr-id-ddd3b072e8ec4236936c2cb53af1521ab1abeded/> embed component metadata

### Test

 - <csr-id-1495c8f3e6fdda67a90fc821a731072b72fc4062/> add wit_world to test case

### Chore (BREAKING)

 - <csr-id-a1e8d3f09e039723d28d738d98b47bce54e4450d/> update ctl to 0.31.0
 - <csr-id-d53bf1b5e3be1cd8d076939cc80460305e30d8c5/> remove prov_rpc options

### New Features (BREAKING)

<csr-id-acdcd957bfedb5a86a0420c052da1e65d32e6c23/>

 - <csr-id-7851a53ab31273b04df8372662198ac6dc70f78e/> add scale and update cmds
 - <csr-id-bb69ea644d95517bfdc38779c2060096f1cec30f/> update to start/stop/scale for concurrent instances
 - <csr-id-90f79447bc0b1dc7efbef2b13af9cf715e1ea1f0/> add par command support to wash-lib
   * Added par support to wash-lib

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 191 commits contributed to the release over the course of 465 calendar days.
 - 83 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 27 unique issues were worked on: [#292](https://github.com/wasmCloud/wasmCloud/issues/292), [#294](https://github.com/wasmCloud/wasmCloud/issues/294), [#297](https://github.com/wasmCloud/wasmCloud/issues/297), [#303](https://github.com/wasmCloud/wasmCloud/issues/303), [#318](https://github.com/wasmCloud/wasmCloud/issues/318), [#327](https://github.com/wasmCloud/wasmCloud/issues/327), [#329](https://github.com/wasmCloud/wasmCloud/issues/329), [#333](https://github.com/wasmCloud/wasmCloud/issues/333), [#346](https://github.com/wasmCloud/wasmCloud/issues/346), [#353](https://github.com/wasmCloud/wasmCloud/issues/353), [#354](https://github.com/wasmCloud/wasmCloud/issues/354), [#355](https://github.com/wasmCloud/wasmCloud/issues/355), [#359](https://github.com/wasmCloud/wasmCloud/issues/359), [#363](https://github.com/wasmCloud/wasmCloud/issues/363), [#375](https://github.com/wasmCloud/wasmCloud/issues/375), [#376](https://github.com/wasmCloud/wasmCloud/issues/376), [#390](https://github.com/wasmCloud/wasmCloud/issues/390), [#393](https://github.com/wasmCloud/wasmCloud/issues/393), [#399](https://github.com/wasmCloud/wasmCloud/issues/399), [#400](https://github.com/wasmCloud/wasmCloud/issues/400), [#407](https://github.com/wasmCloud/wasmCloud/issues/407), [#452](https://github.com/wasmCloud/wasmCloud/issues/452), [#459](https://github.com/wasmCloud/wasmCloud/issues/459), [#520](https://github.com/wasmCloud/wasmCloud/issues/520), [#556](https://github.com/wasmCloud/wasmCloud/issues/556), [#560](https://github.com/wasmCloud/wasmCloud/issues/560), [#677](https://github.com/wasmCloud/wasmCloud/issues/677)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#292](https://github.com/wasmCloud/wasmCloud/issues/292)**
    - [FEATURE] Adding `wash-lib`, implementing `start` functionality ([`b77b90d`](https://github.com/wasmCloud/wasmCloud/commit/b77b90df088b37f6bdccd344e576c60407fb41b2))
 * **[#294](https://github.com/wasmCloud/wasmCloud/issues/294)**
    - `wash up` implementation ([`3104999`](https://github.com/wasmCloud/wasmCloud/commit/3104999bbbf9e86a806183d6978597a1f30140c1))
 * **[#297](https://github.com/wasmCloud/wasmCloud/issues/297)**
    - Create `wash build` command and add configuration parsing ([`f72ca88`](https://github.com/wasmCloud/wasmCloud/commit/f72ca88373870c688efb0144b796a8e67dc2aaf8))
 * **[#303](https://github.com/wasmCloud/wasmCloud/issues/303)**
    - Update wash-lib with minimum version requirement and mix releases ([`13d44c7`](https://github.com/wasmCloud/wasmCloud/commit/13d44c7085951b523427624108fd3cf1415a53b6))
 * **[#318](https://github.com/wasmCloud/wasmCloud/issues/318)**
    - Set stdin to null when starting a wasmcloud host with wash-lib ([`38e05b2`](https://github.com/wasmCloud/wasmCloud/commit/38e05b2864cadff5cf08c3896546c6b397ab5c07))
 * **[#327](https://github.com/wasmCloud/wasmCloud/issues/327)**
    - Feat/wash down ([`33cdd7d`](https://github.com/wasmCloud/wasmCloud/commit/33cdd7d763acb490a67556fbcbc2c4e42ccd907e))
 * **[#329](https://github.com/wasmCloud/wasmCloud/issues/329)**
    - Fix credentials path format for Windows ([`e81addb`](https://github.com/wasmCloud/wasmCloud/commit/e81addb26fc5ba9ec1254c330f2d391d00bb9f0a))
 * **[#333](https://github.com/wasmCloud/wasmCloud/issues/333)**
    - Parse version and name from `Cargo.toml` when not provided in `wasmcloud.toml`. ([`dfa9994`](https://github.com/wasmCloud/wasmCloud/commit/dfa99944a8a217d67dcf55417e76e7088ef5b86f))
 * **[#346](https://github.com/wasmCloud/wasmCloud/issues/346)**
    - Bump dependencies ([`0178c36`](https://github.com/wasmCloud/wasmCloud/commit/0178c36e66e4282ce42581fa26c8f0e04d634b2b))
 * **[#353](https://github.com/wasmCloud/wasmCloud/issues/353)**
    - Moved project build functionality to wash-lib ([`c31a5d4`](https://github.com/wasmCloud/wasmCloud/commit/c31a5d4d05427874fa9fc408f70a9072b4fd1ecd))
 * **[#354](https://github.com/wasmCloud/wasmCloud/issues/354)**
    - Fixed 352, added js_domain to context ([`c7f4c1d`](https://github.com/wasmCloud/wasmCloud/commit/c7f4c1d43d51582443dd657dde8c949c3e78f9de))
 * **[#355](https://github.com/wasmCloud/wasmCloud/issues/355)**
    - Moved generate module to wash-lib ([`9fa5331`](https://github.com/wasmCloud/wasmCloud/commit/9fa53311a6d674a1c532a770ea636c93562c962f))
 * **[#359](https://github.com/wasmCloud/wasmCloud/issues/359)**
    - Grant execute permission to `mac_listener` for hot-reloading ([`5cc6ebe`](https://github.com/wasmCloud/wasmCloud/commit/5cc6ebe2b8596b5fb1a56abb4d17e4e3f104b110))
 * **[#363](https://github.com/wasmCloud/wasmCloud/issues/363)**
    - Pinned to stable versions for 0.14.0 release ([`223096b`](https://github.com/wasmCloud/wasmCloud/commit/223096b5e9bba877d0bca023b1ec3021399ec32d))
 * **[#375](https://github.com/wasmCloud/wasmCloud/issues/375)**
    - Allow prerelease tags with warning ([`a3aebd2`](https://github.com/wasmCloud/wasmCloud/commit/a3aebd219d2db5d1d725a42b537d1e91d1d87bd9))
 * **[#376](https://github.com/wasmCloud/wasmCloud/issues/376)**
    - Create default context if host_config not found ([`51d4748`](https://github.com/wasmCloud/wasmCloud/commit/51d474851dbcf325cc6b422f9ee09486e43c6984))
 * **[#390](https://github.com/wasmCloud/wasmCloud/issues/390)**
    - Use lib.name from cargo.toml for rust wasm binary name ([`656ea64`](https://github.com/wasmCloud/wasmCloud/commit/656ea644696ea97bdafdbf8d5fd4a5e736593fc8))
 * **[#393](https://github.com/wasmCloud/wasmCloud/issues/393)**
    - Fix clippy lints ([`030b844`](https://github.com/wasmCloud/wasmCloud/commit/030b8449d46d880b3b9c4897870c7ea3c74ff003))
 * **[#399](https://github.com/wasmCloud/wasmCloud/issues/399)**
    - Use exact imports instead of globs ([`95851b6`](https://github.com/wasmCloud/wasmCloud/commit/95851b667bd7d23d0c2114cd550f082db6cd935b))
 * **[#400](https://github.com/wasmCloud/wasmCloud/issues/400)**
    - Remove git command output from `wash new actor` output and add message about cloning the template ([`f9a656f`](https://github.com/wasmCloud/wasmCloud/commit/f9a656fd92589687027458f8c0d1f6dd7038d7ae))
 * **[#407](https://github.com/wasmCloud/wasmCloud/issues/407)**
    - Adopt workspace dependencies ([`80b1040`](https://github.com/wasmCloud/wasmCloud/commit/80b104011536c03ef3c1c58a1440992defae1351))
 * **[#452](https://github.com/wasmCloud/wasmCloud/issues/452)**
    - Feat/wash inspect ([`0b2f0d3`](https://github.com/wasmCloud/wasmCloud/commit/0b2f0d3c1d56d1a7d2f8fed0f389a82846817051))
 * **[#459](https://github.com/wasmCloud/wasmCloud/issues/459)**
    - Removed workspace deps for wash-lib modules ([`6170336`](https://github.com/wasmCloud/wasmCloud/commit/6170336fa297162af98c10f8365cab6865c844ec))
 * **[#520](https://github.com/wasmCloud/wasmCloud/issues/520)**
    - Feat(*) wadm 0.4 support in `wash app` ([`b3e2615`](https://github.com/wasmCloud/wasmCloud/commit/b3e2615b225d4fbc5eb8b4cb58c5755df0f68bbc))
 * **[#556](https://github.com/wasmCloud/wasmCloud/issues/556)**
    - Feat(*) wash burrito support ([`812f0e0`](https://github.com/wasmCloud/wasmCloud/commit/812f0e0bc44fd9cbab4acb7be44005657234fa7c))
 * **[#560](https://github.com/wasmCloud/wasmCloud/issues/560)**
    - Bug build actor cargo workspace #wasm cloud/wash/446 ([`410d87c`](https://github.com/wasmCloud/wasmCloud/commit/410d87c1b3db07ed15bcbfd0a9f338c304014c51))
 * **[#677](https://github.com/wasmCloud/wasmCloud/issues/677)**
    - Adding the ability to inspect and inject configuration schemas ([`db3fe8d`](https://github.com/wasmCloud/wasmCloud/commit/db3fe8d7da82cd43389beaf33eed754c0d1a5f19))
 * **Uncategorized**
    - Release wash-lib-v0.13.0 ([`ee51a17`](https://github.com/wasmCloud/wasmCloud/commit/ee51a176a00b3f8fe03e0d3212a9da6dbfd6044f))
    - Support domain, links, keys alias ([`a1c3b9d`](https://github.com/wasmCloud/wasmCloud/commit/a1c3b9d86db14f31ef7fbebeb30e8784f974df6f))
    - Update control interface 0.31 ([`007660e`](https://github.com/wasmCloud/wasmCloud/commit/007660e96ad7472918bc25baf9d52d60e5230823))
    - Update ctl to 0.31.0 ([`a1e8d3f`](https://github.com/wasmCloud/wasmCloud/commit/a1e8d3f09e039723d28d738d98b47bce54e4450d))
    - Apply tags in actor config during signing ([`810e220`](https://github.com/wasmCloud/wasmCloud/commit/810e220173f1ee7bf96a9ade650d26c2cd4dcb6c))
    - Merge pull request #807 from rvolosatovs/merge/wash ([`f2bc010`](https://github.com/wasmCloud/wasmCloud/commit/f2bc010110d96fc21bc3502798543b7d5b68b1b5))
    - Integrate `wash` into the workspace ([`dfad0be`](https://github.com/wasmCloud/wasmCloud/commit/dfad0be609868cbd0f0ce97d7d9238b41996b5fc))
    - Generate golang code during wash build ([`17bb1aa`](https://github.com/wasmCloud/wasmCloud/commit/17bb1aa431f951b66b15a523032b5164893a2670))
    - Update dependencies ([`621e449`](https://github.com/wasmCloud/wasmCloud/commit/621e449a1e70f9216016b11a6ff50c7a1def10e1))
    - Configure signing keys directory for build cmd ([`ef3e4e5`](https://github.com/wasmCloud/wasmCloud/commit/ef3e4e584fef4d597cab0215fdf3cfe864f701e9))
    - `Err(anyhow!(...))` -> `bail!`, err msg capitals ([`5af1c68`](https://github.com/wasmCloud/wasmCloud/commit/5af1c68bf86b62b4e2f81cbf1cc9ca1d5542ac37))
    - Mark components built with wash as experimental ([`462767b`](https://github.com/wasmCloud/wasmCloud/commit/462767b950d4fd23b0961bd8a5eb5499c16bc27b))
    - Remove unused var ([`5ef2c4c`](https://github.com/wasmCloud/wasmCloud/commit/5ef2c4c924dbc2d93a75f99b5975b321e1bad75f))
    - Remove prov_rpc options ([`d53bf1b`](https://github.com/wasmCloud/wasmCloud/commit/d53bf1b5e3be1cd8d076939cc80460305e30d8c5))
    - Merge pull request #922 from vados-cosmonic/refactor/light-testing-code-refactor ([`0b9e1ca`](https://github.com/wasmCloud/wasmCloud/commit/0b9e1caf8143fd7688f7658db76f01b6bd4a6c5f))
    - Various fixes to testing code ([`372e81e`](https://github.com/wasmCloud/wasmCloud/commit/372e81e2da3a60ee8cbf3f2525bf27284dc62332))
    - Merge pull request #914 from connorsmith256/chore/update-test ([`516aa5e`](https://github.com/wasmCloud/wasmCloud/commit/516aa5eb7d0271795ae44af288edc80742a60ccb))
    - Update test message ([`9caf89a`](https://github.com/wasmCloud/wasmCloud/commit/9caf89a7d15a7d8ec80a490fe0f4106089c77728))
    - Bump wash-lib and wash-cli for wit-parser fix ([`5ae8fd8`](https://github.com/wasmCloud/wasmCloud/commit/5ae8fd8bad3fadb5b97be28d5e163b621938a272))
    - Merge pull request #873 from connorsmith256/feat/get-all-inventories ([`3b58fc7`](https://github.com/wasmCloud/wasmCloud/commit/3b58fc739b5ee6a8609e3d2501abfbdf604fe897))
    - Query all host inventories ([`5c0ccc5`](https://github.com/wasmCloud/wasmCloud/commit/5c0ccc5f872ad42b6152c66c34ab73f855f82832))
    - Merge pull request #875 from ahmedtadde/feat/expand-manifest-input-sources-clean ([`c25352b`](https://github.com/wasmCloud/wasmCloud/commit/c25352bb21e7ec0f733317f2e13d3e183149e679))
    - For app manifest loading, file input source check should preceed http input source. ([`1fa7604`](https://github.com/wasmCloud/wasmCloud/commit/1fa7604d3347df6c0cfb71b8ea4be6bba9bceb34))
    - Add manifest source type to use with app manifest loader. ([`571a25d`](https://github.com/wasmCloud/wasmCloud/commit/571a25ddb7d8f18b2bb1d3f6b22401503d31f719))
    - Add scale and update integration tests ([`109e934`](https://github.com/wasmCloud/wasmCloud/commit/109e934ceaa026f81aeadaca84e7da83668dc5fd))
    - Add scale and update cmds ([`7851a53`](https://github.com/wasmCloud/wasmCloud/commit/7851a53ab31273b04df8372662198ac6dc70f78e))
    - Update to start/stop/scale for concurrent instances ([`bb69ea6`](https://github.com/wasmCloud/wasmCloud/commit/bb69ea644d95517bfdc38779c2060096f1cec30f))
    - Update async_nats,ctl,wasmbus_rpc to latest ([`70b20a1`](https://github.com/wasmCloud/wasmCloud/commit/70b20a12553e84697ffe9f8dbf32219162bdf946))
    - Bump to 0.21.0, wash-lib 0.12.0 ([`c44f657`](https://github.com/wasmCloud/wasmCloud/commit/c44f657e3bdc1e4a6679b3cc687b7039fb729f34))
    - Adjustments to app manifest loader ([`ee29478`](https://github.com/wasmCloud/wasmCloud/commit/ee29478631ba0df2d67a00e3f1336b4c40099489))
    - Some cleanup relevant to app manifest input sources ([`0eb5a7c`](https://github.com/wasmCloud/wasmCloud/commit/0eb5a7cade13a87e59c27c7f6faa89234d07863d))
    - Polishing app manifest loader ([`32ea9f9`](https://github.com/wasmCloud/wasmCloud/commit/32ea9f9eb8ba63118dfd23084d413aae23226124))
    - Http & stdin manifest input sources support for put & deploy cmds ([`6907c80`](https://github.com/wasmCloud/wasmCloud/commit/6907c8012fd59bbcaa6234c533b62ba997b86139))
    - Merge pull request #864 from connorsmith256/release/wash-lib-v0.11.4 ([`79a2cef`](https://github.com/wasmCloud/wasmCloud/commit/79a2cef71fd4bcf9f5eb5f313f8087662dd25b9c))
    - Wash-lib v0.11.4 ([`b3965d7`](https://github.com/wasmCloud/wasmCloud/commit/b3965d7bb04e70da967bc393b9455c4c1da6b20b))
    - Merge pull request #758 from wasmCloud/tg_wasi_respect ([`a7df4cb`](https://github.com/wasmCloud/wasmCloud/commit/a7df4cb8b81c2028c98d8238369a4027644fa3a4))
    - Add wit_world to test case ([`1495c8f`](https://github.com/wasmCloud/wasmCloud/commit/1495c8f3e6fdda67a90fc821a731072b72fc4062))
    - Typo ([`2b55ae4`](https://github.com/wasmCloud/wasmCloud/commit/2b55ae469c07af8bd94e21f606584ef67e2e0f9a))
    - Embed component metadata ([`ddd3b07`](https://github.com/wasmCloud/wasmCloud/commit/ddd3b072e8ec4236936c2cb53af1521ab1abeded))
    - Unify rust and tinygo component target logic ([`6d71c1f`](https://github.com/wasmCloud/wasmCloud/commit/6d71c1f36111efe1942e522c8ac6b315c78d81ab))
    - Add to wasi target tinygo builder ([`3d5517c`](https://github.com/wasmCloud/wasmCloud/commit/3d5517c512b06dc47b6e395e0bc57d2022b4aabb))
    - Merge pull request #863 from connorsmith256/release/wash-lib-v0.11.3 ([`590159c`](https://github.com/wasmCloud/wasmCloud/commit/590159ca586ad654b0d21528dbd6ecf9153a5e7e))
    - Wash-lib v0.11.3 ([`4a4c148`](https://github.com/wasmCloud/wasmCloud/commit/4a4c148f2e1ddb3eba535b40575265f51968ffaa))
    - Merge pull request #861 from connorsmith256/release/wash-lib-v0.11.2 ([`f35dcad`](https://github.com/wasmCloud/wasmCloud/commit/f35dcad9a95776833c5b1bf2b2b1b34e378f84ef))
    - Wash-lib v0.11.2 ([`b9c23d9`](https://github.com/wasmCloud/wasmCloud/commit/b9c23d959c5fb0a1854b8f90db6a0a0e4b1cdda9))
    - Merge pull request #849 from vados-cosmonic/chore/fix-lint ([`894329f`](https://github.com/wasmCloud/wasmCloud/commit/894329fca42ff4e58dbdffe9a39bc90147c63727))
    - Fix lint ([`016c378`](https://github.com/wasmCloud/wasmCloud/commit/016c37812b8cf95615a6ad34ee49de669c66886b))
    - Add par command support to wash-lib ([`90f7944`](https://github.com/wasmCloud/wasmCloud/commit/90f79447bc0b1dc7efbef2b13af9cf715e1ea1f0))
    - Merge pull request #840 from wasmCloud/release/wash-lib-v0.11.1 ([`64bdebf`](https://github.com/wasmCloud/wasmCloud/commit/64bdebfc1036b14dd94badeff880935dba7fe15c))
    - Wash-lib v0.11.1 ([`f582dc0`](https://github.com/wasmCloud/wasmCloud/commit/f582dc07ea768f9b52b13c7d5c618c36e4ff0a0c))
    - Merge pull request #839 from aish-where-ya/fix/update-actor ([`6d98a6d`](https://github.com/wasmCloud/wasmCloud/commit/6d98a6d2608333661254c184d6aba8e6b81fd145))
    - Minor fix to update actor in wash-lib ([`3dbbc03`](https://github.com/wasmCloud/wasmCloud/commit/3dbbc03c22e983a0b89a681a4645ad04a0a4b7d2))
    - Merge pull request #832 from connorsmith256/release/wash-lib-v0.11.0 ([`f635d63`](https://github.com/wasmCloud/wasmCloud/commit/f635d63ee6d1bcbf7f69674a5206b2563b99b553))
    - V0.11.0 ([`0f5add0`](https://github.com/wasmCloud/wasmCloud/commit/0f5add0f6e2a27d76ee63c1e387929474c93751e))
    - Move update-actor for wash ctl update to wash-lib. ([`99262d8`](https://github.com/wasmCloud/wasmCloud/commit/99262d8b1c0bdb09657407663e2d5d4a3fb7651c))
    - Merge pull request #822 from rvolosatovs/feat/android ([`4bde6b7`](https://github.com/wasmCloud/wasmCloud/commit/4bde6b786375e540ea9a13ba6aeaad039cc448e6))
    - Add support for Android releases ([`6405f6c`](https://github.com/wasmCloud/wasmCloud/commit/6405f6ce45d43850ca427c4d80ca50369ee10405))
    - Move generate key message to info log ([`3351e0a`](https://github.com/wasmCloud/wasmCloud/commit/3351e0a83bc92dab8b73bc88b8d03a95dfad3e0a))
    - Bump cargo_metadata from 0.17.0 to 0.18.0 ([`3797857`](https://github.com/wasmCloud/wasmCloud/commit/37978577b218cf178fa795fb9e5326df4bd52897))
    - Bump to 0.10.1 to release wadm ([`bb76aec`](https://github.com/wasmCloud/wasmCloud/commit/bb76aec405e437c249d385e3492cb67932960125))
    - Remove references to DASHBOARD_PORT ([`bbf0b1a`](https://github.com/wasmCloud/wasmCloud/commit/bbf0b1a6074108a96d9534500c97c8ad5ed13dd6))
    - Merge pull request #762 from wasmCloud/release/v0.10.0 ([`308a3cb`](https://github.com/wasmCloud/wasmCloud/commit/308a3cbd09501359ce3465e8cc8a39e1278f0d8a))
    - Wash-lib v0.10.0 ([`e67ded6`](https://github.com/wasmCloud/wasmCloud/commit/e67ded670e80a19e08bcb8e6b2a25f696792ef66))
    - Merge pull request #759 from wasmCloud/rust-host-default ([`6be0162`](https://github.com/wasmCloud/wasmCloud/commit/6be0162cb89a6d030270d616bc4667c2c5cc7186))
    - Update test to reflect changes from OTP to Rust host ([`f927929`](https://github.com/wasmCloud/wasmCloud/commit/f9279294ea7602ad6bbc55a5f3dc8940f2d46d71))
    - Use rc2 ([`f4a9cd6`](https://github.com/wasmCloud/wasmCloud/commit/f4a9cd6d2f1c29b0cc7eb4c3509114ed81eb7983))
    - Set default to Rust host ([`78b99fd`](https://github.com/wasmCloud/wasmCloud/commit/78b99fde8606febf59e30f1d12ac558b29d425bf))
    - Bump cargo_metadata from 0.15.4 to 0.17.0 ([`a4f67e5`](https://github.com/wasmCloud/wasmCloud/commit/a4f67e5974c6bad70cd2d473fea7ab24371f922f))
    - Config loading for preview2 adapter path ([`7111b5d`](https://github.com/wasmCloud/wasmCloud/commit/7111b5d9a5ece7543ded436b7816974ad27910e2))
    - Preserve interactive terminal when checking git ([`b0e746b`](https://github.com/wasmCloud/wasmCloud/commit/b0e746be713d070b4400294ec401b87444bd5741))
    - Merge pull request #682 from vados-cosmonic/release/wash-lib/v0.9.2 ([`0f9df26`](https://github.com/wasmCloud/wasmCloud/commit/0f9df261ada50e4ea510631387508196cdbcd891))
    - Merge pull request #684 from vados-cosmonic/chore/use-upstream-fix-for-windows-component-adapter ([`9b42815`](https://github.com/wasmCloud/wasmCloud/commit/9b428154de006118daa774fb1fd96d47bda4df83))
    - Merge pull request #683 from wasmCloud/feat/single-host-inventory-query ([`3fe92ae`](https://github.com/wasmCloud/wasmCloud/commit/3fe92aefcf573a52f7f67a30d06daba33861427c))
    - Use released wasmcloud-component-adapters ([`10ede9e`](https://github.com/wasmCloud/wasmCloud/commit/10ede9e84e537fecbad3cbbb09960506b6359ef4))
    - Allow get inventory to query the only host ([`acdcd95`](https://github.com/wasmCloud/wasmCloud/commit/acdcd957bfedb5a86a0420c052da1e65d32e6c23))
    - V0.9.2 ([`ae65e85`](https://github.com/wasmCloud/wasmCloud/commit/ae65e85bf4b8bcbc215d48664fcf6941d25de165))
    - Merge pull request #663 from vados-cosmonic/feat/support-adapting-p2-components ([`28c4aa6`](https://github.com/wasmCloud/wasmCloud/commit/28c4aa66a5c113c08ade5da1ead303f6b932afaf))
    - Build wasi preview components from wash ([`4144f71`](https://github.com/wasmCloud/wasmCloud/commit/4144f711ad2056e9334e085cbe08663065605b0c))
    - Merge pull request #643 from lachieh/detachable-washboard ([`6402d13`](https://github.com/wasmCloud/wasmCloud/commit/6402d13de96ad18516dd5efc530b1c3f05964df1))
    - Add standalone washboard (experimental) ([`12fdad0`](https://github.com/wasmCloud/wasmCloud/commit/12fdad013f5222dd21fdf63f1c7b2f0c37098b89))
    - Add p2 target to wasmcloud.toml ([`bb454cb`](https://github.com/wasmCloud/wasmCloud/commit/bb454cb3ae1ff05d8381ba2ea1f48b461d059474))
    - Merge pull request #629 from thomastaylor312/fix/multiple_nats ([`389a702`](https://github.com/wasmCloud/wasmCloud/commit/389a7023b9a6c584d27e2b48573f21e7b09c41ba))
    - Corrected creds escaping on Windows ([`d47f2b4`](https://github.com/wasmCloud/wasmCloud/commit/d47f2b4c46aaad13033a897ef6bbacdcd9e93774))
    - Bumped cargo versions for wash-lib 0.9.1 wash 0.18.1 ([`30ca8e0`](https://github.com/wasmCloud/wasmCloud/commit/30ca8e02daec1311025997c1bd130e3cc9389675))
    - First check that git command is installed ([`02b1f03`](https://github.com/wasmCloud/wasmCloud/commit/02b1f03e05c4ffc7b62d2438752344cd2c805d3f))
    - Return an explicit error when the build tools don't exist ([`f965828`](https://github.com/wasmCloud/wasmCloud/commit/f9658287e6bdb77a6991e827454951a0711bce42))
    - Allows multiple hosts to run without sharing data ([`4900f82`](https://github.com/wasmCloud/wasmCloud/commit/4900f82caf39913e076c1664702d9e9d02836135))
    - Merge pull request #619 from vados-cosmonic/fix/flaky-tests ([`eb9de36`](https://github.com/wasmCloud/wasmCloud/commit/eb9de3645589454c89ca4cb2f043bb1e395f26f0))
    - Flaky tests ([`c7643e8`](https://github.com/wasmCloud/wasmCloud/commit/c7643e8b777af175d23aa66771067ccc3ee38fd3))
    - Merge pull request #610 from vados-cosmonic/feat/add-wash-dev ([`00e0aea`](https://github.com/wasmCloud/wasmCloud/commit/00e0aea33815b6ac5abdb4c2cf2a5815ebe35cb3))
    - Add wash dev command ([`e9fe020`](https://github.com/wasmCloud/wasmCloud/commit/e9fe020a0906cb377f6ea8bd3a9879e5bad877b7))
    - Added kvcounter template to wash favorites ([`e6b874c`](https://github.com/wasmCloud/wasmCloud/commit/e6b874c058a3a71920c8370f786a40a73ab0047b))
    - Moved registry cli things to registry cli ([`1172806`](https://github.com/wasmCloud/wasmCloud/commit/1172806ea5a7e2a24d4570d76cf53f104a0d3e30))
    - Fixed wash-lib release failure ([`0f6b5c2`](https://github.com/wasmCloud/wasmCloud/commit/0f6b5c2219bcaa35d8f29bd7296d9486b478f957))
    - Bumped to stable versions, 0.18.0 ([`811eb48`](https://github.com/wasmCloud/wasmCloud/commit/811eb482f2815374ce8dfed10a474ab33adbe320))
    - Merge pull request #612 from thomastaylor312/feat/wash_capture ([`3a14bbc`](https://github.com/wasmCloud/wasmCloud/commit/3a14bbc9999e680f5044223aff7d13c0e3b319bc))
    - Adds a new experimental `wash capture` command ([`8c96789`](https://github.com/wasmCloud/wasmCloud/commit/8c96789f1c793c5565715080b84fecfbe0653b43))
    - Merge pull request #603 from thomastaylor312/feat/wash_spy ([`213ac6b`](https://github.com/wasmCloud/wasmCloud/commit/213ac6b8e9b3d745764d8df1f20ceb41b10cd1f2))
    - Adds `wash spy` command with experimental flag support ([`e58c6a6`](https://github.com/wasmCloud/wasmCloud/commit/e58c6a60928a7157ffbbc95f9eabcc9cae3db2a7))
    - Bumps wadm to 0.4.0 stable ([`41d3d3c`](https://github.com/wasmCloud/wasmCloud/commit/41d3d3cfa2e5a285833c8ecd2a21bb6821d2f47e))
    - Flatten multiple commands into wash get ([`6923ce7`](https://github.com/wasmCloud/wasmCloud/commit/6923ce7efb721f8678c33f42647b87ea33a7653a))
    - Merge pull request #580 from vados-cosmonic/feat/ux/wash-reg-push-and-pull ([`a553348`](https://github.com/wasmCloud/wasmCloud/commit/a553348a44b430937bd3222600a477f52300fb74))
    - Flatten wash reg push/pull into wash push/pull ([`4daf51b`](https://github.com/wasmCloud/wasmCloud/commit/4daf51be422d395bc0142d62b8d59060b89feafa))
    - Merge pull request #576 from vados-cosmonic/feat/ux/flatten-wash-stop ([`7b66d65`](https://github.com/wasmCloud/wasmCloud/commit/7b66d6575e8f1b360ff331e171bc784d96e3681a))
    - Flatten `wash ctl stop` into `wash stop` ([`128f760`](https://github.com/wasmCloud/wasmCloud/commit/128f7603c67443f23e76c3cb4bd1468ffd8f5462))
    - Merge pull request #573 from vados-cosmonic/feat/ux/flatten-wash-start ([`612951b`](https://github.com/wasmCloud/wasmCloud/commit/612951ba8ac5078f4234677c842b41c729f08985))
    - Flatten `wash ctl start` into `wash start` ([`2a6c401`](https://github.com/wasmCloud/wasmCloud/commit/2a6c401834b4cb55ef420538e15503b98281eaf1))
    - Merge pull request #569 from vados-cosmonic/feat/ux/flatten-wash-link ([`def34b6`](https://github.com/wasmCloud/wasmCloud/commit/def34b60b5fea48a3747b661a7a7daf2fb8daff7))
    - Flatten `wash ctl link` into `wash link` ([`24bba48`](https://github.com/wasmCloud/wasmCloud/commit/24bba484009be9e87bfcbd926a731534e936c339))
    - Removed error in generate ([`ec4e20b`](https://github.com/wasmCloud/wasmCloud/commit/ec4e20ba0b69636c62fe0d646ea79b5d1314235f))
    - Bumped wadm to 0.4.0-alpha.3 ([`a01b605`](https://github.com/wasmCloud/wasmCloud/commit/a01b605041e9b2041944a939ae00f9d38e782f26))
    - Fixed ci, ensured wadm doesn't connect to default nats ([`b348399`](https://github.com/wasmCloud/wasmCloud/commit/b34839902832bfa6f6426b3d8ff0b3b57ca4247c))
    - Set up 0.18.0 alpha release for testing ([`3320ee7`](https://github.com/wasmCloud/wasmCloud/commit/3320ee7c9eac549c8fe1bb0c6d1bcb9f5574d98d))
    - #466 Update toml crate, which required updating weld-codegen. ([`1915f2d`](https://github.com/wasmCloud/wasmCloud/commit/1915f2d474736f39682679487298d3c18a8a627b))
    - Patched start wasmcloud to accept dashboard port ([`b68bbfc`](https://github.com/wasmCloud/wasmCloud/commit/b68bbfcfc3e0df5f7b6876e326f2a36a677846a4))
    - Merge pull request #522 from thomastaylor312/chore/bump_wash_lib ([`5b8441b`](https://github.com/wasmCloud/wasmCloud/commit/5b8441b1f526e799e2609525d19a1950d4dec0a1))
    - Bumps wash-lib version ([`89e638a`](https://github.com/wasmCloud/wasmCloud/commit/89e638a8e63073800fc952c0a874e54e9996d422))
    - Merge pull request #513 from connorsmith256/feat/allow-file-upload ([`bf4e46c`](https://github.com/wasmCloud/wasmCloud/commit/bf4e46cf816fc3385540ca752dfdaa1fd13ae78e))
    - Satisfy clippy ([`4f5afad`](https://github.com/wasmCloud/wasmCloud/commit/4f5afadbb9324216d64eeb95ea2eef5f986592e9))
    - Merge pull request #508 from aish-where-ya/main ([`6fd026c`](https://github.com/wasmCloud/wasmCloud/commit/6fd026ce1670a75f23bc93fdc9325d5bc756050d))
    - Refactoring based on review comments ([`448211e`](https://github.com/wasmCloud/wasmCloud/commit/448211e55f8491fb9a12611e6c61615411cd47fd))
    - Wash up waits for washboard to be up ([`efaacd7`](https://github.com/wasmCloud/wasmCloud/commit/efaacd7d67bef6873980d9b8575dd268e13f941f))
    - Merge pull request #379 from ceejimus/bug/latest-tags-w-no-allow-latest ([`ec5240b`](https://github.com/wasmCloud/wasmCloud/commit/ec5240bb0ee9e061d6a56c519d677f5551d60c9d))
    - Merge pull request #477 from connorsmith256/bump/wasmcloud-host-version ([`7dbd961`](https://github.com/wasmCloud/wasmCloud/commit/7dbd961378a314a0647e812b819abf014e08c004))
    - Bump to v0.61.0 of wasmcloud host ([`3d80c4e`](https://github.com/wasmCloud/wasmCloud/commit/3d80c4e1ce3bcc7e71cc4dbffe927ca87c524f42))
    - [fix] make regex required ([`fb5f5d2`](https://github.com/wasmCloud/wasmCloud/commit/fb5f5d28d6cd18b7a57f512fa9ea79a415066ba1))
    - [fix] add better error handling for empty tags when --allow-latest is false ([`98faa4a`](https://github.com/wasmCloud/wasmCloud/commit/98faa4a9a748532a11dcb322f75424ca1ac7ecbe))
    - Merge pull request #467 from connorsmith256/bump/versions ([`423c0ad`](https://github.com/wasmCloud/wasmCloud/commit/423c0ad736b2757aa58e7db601dd9e1ecc565719))
    - Bump versions to same commit ([`6df3165`](https://github.com/wasmCloud/wasmCloud/commit/6df31657af85a1d8bf9be58f8e347ef8e06ecd3b))
    - Merge branch 'main' into fix/nextest-usage-in-makefile ([`03c02f2`](https://github.com/wasmCloud/wasmCloud/commit/03c02f270faed157c95dd01ee42069610662314b))
    - Merge pull request #450 from vados-cosmonic/release/wash-lib/v0.6.1 ([`8a3e9c7`](https://github.com/wasmCloud/wasmCloud/commit/8a3e9c7bc75c898f8b8108f8d4dd9293474196d3))
    - Wash v0.16.1, wash-lib v0.6.1 ([`0ed956f`](https://github.com/wasmCloud/wasmCloud/commit/0ed956f457a94ad390b847a46df9911e5ebb35a9))
    - Merge pull request #420 from thomastaylor312/fml/less_flakes_by_making_it_nap ([`bbba36f`](https://github.com/wasmCloud/wasmCloud/commit/bbba36f1e9d7a867866812bf60a8dcb61e95f701))
    - Makes sure we wait for the NATS server to be up before continuing with the host ([`51e63e4`](https://github.com/wasmCloud/wasmCloud/commit/51e63e436fbe08c152a013081b5bb90eb3963c8d))
    - Adds more error messaging around some flakes ([`e3e3c0a`](https://github.com/wasmCloud/wasmCloud/commit/e3e3c0a1c2582ee473ab07daee5b9e4286566f6e))
    - Merge pull request #381 from wasmCloud/bump/0.15.0-wasmcloud-0.60.0 ([`b06b71b`](https://github.com/wasmCloud/wasmCloud/commit/b06b71b68ba78405a321a9bbd6968f1ad8b461b7))
    - Bumps wash lib version, as the semver gods intended ([`e3c423b`](https://github.com/wasmCloud/wasmCloud/commit/e3c423b8c16c4ef805991dcee8082fd4063fdb38))
    - Addresses PR comment ([`1609b0d`](https://github.com/wasmCloud/wasmCloud/commit/1609b0d9604106f4f5bf6e62e88eff94683ed2f9))
    - Makes sure that wash downloads different versions of wasmcloud ([`2e69e12`](https://github.com/wasmCloud/wasmCloud/commit/2e69e12d4b78f5ea7710ba12226345440e7541ef))
    - Merge pull request #368 from connorsmith256/add-echo-messaging-template ([`2808632`](https://github.com/wasmCloud/wasmCloud/commit/28086323245395260aeafccf3aaf449b7970596e))
    - Bump wash-lib to v0.5.0 ([`7baa633`](https://github.com/wasmCloud/wasmCloud/commit/7baa633adda1ae6ace7889af7bdf267f64b6ba9e))
    - Add echo-messaging to default templates ([`fc38533`](https://github.com/wasmCloud/wasmCloud/commit/fc385336cc1643f79dfb5196d234bd1c2f6bcb7a))
    - Merge pull request #361 from ricochet/bump-wascap ([`eba79d4`](https://github.com/wasmCloud/wasmCloud/commit/eba79d4dcf18709a559aa5052219f22635145d55))
    - Merge branch 'main' into bump-wascap ([`cd35ff9`](https://github.com/wasmCloud/wasmCloud/commit/cd35ff9a4994469b45318a34fed8b13e6312cf95))
    - Consume new wascap and hashing ([`12cae48`](https://github.com/wasmCloud/wasmCloud/commit/12cae48ff806b26b6c4f583ae00337b21bc65d3c))
    - Merge pull request #345 from thomastaylor312/lib/claims ([`b0e385d`](https://github.com/wasmCloud/wasmCloud/commit/b0e385d1d4198614ce19299f0d71531225d85a96))
    - Bring over to_lowercase ([`6cab2aa`](https://github.com/wasmCloud/wasmCloud/commit/6cab2aa508a6184fc818af29346ec77c2d56efd3))
    - Moves claims and registry code into wash lib ([`84b9539`](https://github.com/wasmCloud/wasmCloud/commit/84b95392993cbbc65da36bc8b872241cce32a63e))
    - Merge pull request #344 from thomastaylor312/lib/keys ([`08bbb0f`](https://github.com/wasmCloud/wasmCloud/commit/08bbb0f2b9693d1c53842e454c83129e8c7bdaa3))
    - Adds new keys module to wash-lib ([`a62b07b`](https://github.com/wasmCloud/wasmCloud/commit/a62b07b8ff321c400c6debefdb6199e273445490))
    - Merge pull request #339 from thomastaylor312/lib/context ([`10f9c1b`](https://github.com/wasmCloud/wasmCloud/commit/10f9c1bb06e0b413c4c5fd579f015e32dae86f69))
    - Fixes issue with creating initial context ([`92f448e`](https://github.com/wasmCloud/wasmCloud/commit/92f448e69fdaa415ab6fa2fdfd3dce638ac2572d))
    - Adds deleting of default context ([`d658dc4`](https://github.com/wasmCloud/wasmCloud/commit/d658dc42f487c08bcd780e70a9331e9139dfc5d6))
    - Adds new context tests ([`d0659d3`](https://github.com/wasmCloud/wasmCloud/commit/d0659d346a6acadf81ce8dd952262f372c738e8d))
    - Creates new context library ([`52ef5b6`](https://github.com/wasmCloud/wasmCloud/commit/52ef5b6b1b6b01bc5e7a2c8fe3cbb2a08d4ad864))
    - Merge pull request #337 from thomastaylor312/feat/wash-lib ([`06cea91`](https://github.com/wasmCloud/wasmCloud/commit/06cea91e6541583a46ab306ad871e4a7781274cf))
    - Addresses PR comments ([`2fa41d5`](https://github.com/wasmCloud/wasmCloud/commit/2fa41d50750e3beab90d1ca62d518d7df50f469e))
    - Adds drain command to wash lib ([`b1bf6b1`](https://github.com/wasmCloud/wasmCloud/commit/b1bf6b1ac7851dc09e6757d7c2bde4558ec48098))
    - Merge pull request #330 from connorsmith256/fix/running-host-check ([`c023d59`](https://github.com/wasmCloud/wasmCloud/commit/c023d592dd652ac6d3bb4552646dba1eda18b98e))
    - Pass env vars when checking for running host ([`f2c2276`](https://github.com/wasmCloud/wasmCloud/commit/f2c2276d3408c81a1cf02c18fade1b4a00a1e876))
    - Merge pull request #321 from thomastaylor312/chore/0.13_update ([`38fbf3a`](https://github.com/wasmCloud/wasmCloud/commit/38fbf3a12ca77cbaa610890771ef8ef74f367a50))
    - Bump wash version ([`a1d77b0`](https://github.com/wasmCloud/wasmCloud/commit/a1d77b0e12ebb7b4b946004b61a208482e599ce4))
    - Merge pull request #317 from ricochet/chore/clap-v4 ([`c6ab554`](https://github.com/wasmCloud/wasmCloud/commit/c6ab554fc18de4525a6a90e8b94559f704e5c0b3))
    - Fix clippy warnings ([`2aa4b04`](https://github.com/wasmCloud/wasmCloud/commit/2aa4b041af6195ff4dbd6bf7e04f6cba281585b9))
</details>


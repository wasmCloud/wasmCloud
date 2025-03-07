# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.38.0 (2025-01-14)

### Chore

 - <csr-id-b47fc3357bc4f07358a6801114ff47c2b67d2357/> add wasmCloud/typescript repo as source for washboard downloads
 - <csr-id-da5da3686be4f750435d859250abe5db7fb5d4f6/> fix clippy
 - <csr-id-174d9628f3297884d8815d86c00f622045859fef/> name default padding variable for commands
 - <csr-id-eedfc79794cdfd5d366dabc6e1d782f32e96e8ea/> increase verbosity to trace in example providers
 - <csr-id-eb52eca817fe24b33e7f1a65c1ba5c46c50bef4e/> removed unused dependencies
   A batch scanning all crates and remove unused dependencies by running 'cargo machete'.
 - <csr-id-bc23421b78dcf5d9f017fe2f877ae026b5f96269/> release package updates
 - <csr-id-57ff6d1067a6584aacfc614e594ce9c20e9c92b1/> add curve to keys gen help
 - <csr-id-6210e5de93001cc9611e81bd89f7e2b5ab26d4d3/> reword some help text
 - <csr-id-9622399aeff74d2a79c3bed5980780fc57d98967/> Remove weld-codegen dependency

### Documentation

 - <csr-id-184bf2a59d6ce7e5d022f8b48379eaec85ef0aca/> update wasmcloudOpts and wadmOpts for wash up command

### New Features

 - <csr-id-da3a0f39914e3d2bc96100d118daeb66c24a9132/> update wadm to 0.19
 - <csr-id-563f2dd5d0558fce33445bb3f0f80a494a722e51/> regression test for buffered stdout writes
 - <csr-id-f251e8fd773a6896b51040317e26fd3c70df2010/> test wash dev uses buffered writes after ctrl-c
 - <csr-id-8a3a47298647bbcd1d3a46f6b29df542f5dc4a7c/> add support for deleting all links
 - <csr-id-bdaa78f92872bce526b515785f3fe66984a68e6d/> add extended wkg config  git repo support
 - <csr-id-401ec48c169bbd88f624122a1d9b43b30971e694/> implement pulling from HTTP
 - <csr-id-cbd84402474d3db44641267d470e573c71059288/> support enhanced wkg pull config
   This commit adds support to `wash` for using the "enhanced" wkg config
   -- the new configuration method breaks out configuration for pulling
   into it's own sub-section, and adds support for different kinds of
   WIT pulling that we'd want to see upstream but want to implement
   locally for now.
 - <csr-id-13e0053c19bec4d6775dabc816d068ec1911a0cf/> add version to ui command output
 - <csr-id-5a377bcba1bf8a1907a457b78ca503a81202debf/> wash up is trying to fetch wadm and wasmcloud patch versions to download
 - <csr-id-8bd6b09fb3bcd956c7ab2b7ded3fe91813800fa0/> configure nats-kv policy for secrets
 - <csr-id-286aa22076d4fe58592c85f381954923a55c9e3b/> support wildcard interface overrides
 - <csr-id-a3bff20215cea20116e427da92bb5ad7abf85690/> support interface wildcard overrides
 - <csr-id-70bc37b9628e0f21648285c18c752ce966d0e167/> use spinner to indicate build progress

### Bug Fixes

 - <csr-id-8eb0865699e9d45bf2769df84a5bfe2e2ec4e579/> deserialize snap_data as borrowed slice
 - <csr-id-7ae88df089785ebd85dff23f936a4d90bc5f69c9/> remove extra print statement
 - <csr-id-3d086ab544bd82973a9206940e77edf86970bc8a/> ignore clippy zombie process warning
   This commit ignores the clippy process warning but also adds a wait to
   ensure that tests will hang if processes don't get terminated properly.
 - <csr-id-fd3d77646f8fc8c644cd225303ab1e2855c44c70/> convert usages of wasm32-wasi to wasm32-wasip1
 - <csr-id-24789dff476f3ed3a64faefa2222d8ea4fe77bdd/> improve tests for help usage
 - <csr-id-02b001c5b221c9748f9c969911a5ec16d47c2ca7/> spinner rendering error
   Fixed the spinner rendering inconsistencies by separating out the logic to the match statement
 - <csr-id-d11aae89f227308786153e85e876f7166a0f1e0a/> correct regex and add tests
 - <csr-id-1540eb02a9cf5acba52bce1a84003141fd43adda/> detect host_id from structured logs
 - <csr-id-13026fdd3cf6348b72a8cb0d046fdd49307280fb/> remove query params from git repo URL if present
 - <csr-id-ed17160abcfabbe9dc4d1e5702a26f5398e6f2e3/> dont require NATS to start
 - <csr-id-ed28fa64363508ecc29c91e99a3d57af56645f2d/> corrected formatting
 - <csr-id-d51e9f24292b850188b5212cc06169a4464a6545/> wash dev with JSON output will panic when piped into jq
 - <csr-id-0245805d560aed6cd667c891a245cee9c08e0d77/> use existing get_download_client method
 - <csr-id-844589f49ce16876a067ac580a0e59c84f751cb9/> add new url to check for washboard releases
 - <csr-id-709d9b1e35e051b5b987e4d4205548af37174048/> wadm fetch new version can fail
 - <csr-id-36d6dfdb619b40dc581e353038b5dc07f026ff7e/> don't fail on removing host pid
 - <csr-id-0005eb551f5692fdce83cc9cc0613d4fef85fc33/> kill wadm even if CTRL+c doesn't propagate
 - <csr-id-5fcdfca3373cef428d9820d2ed015b1addc5a81a/> ensure dev uses a single running host

### Other

 - <csr-id-6d7b07dd411e2b5f9692c95c3ee807f7206da5dd/> Removes network calls from unit tests
   We've had several tests for a while that were in unit tests doing things
   like network calls. All network call tests have been moved to integration
   tests and some cleanup and refactoring was done as well.
   
   I also changed the plugin test to just use a committed wash plugin binary.
   The code is still in the repo (and updated to use the new wkg fetching),
   but it added a significant delay because it compiled both `wash` and the
   plugin. With a committed binary, it becomes much faster
 - <csr-id-d075c5518a4089938e1c316881846e04f931743b/> add more tests for wkg fetching

### Refactor

 - <csr-id-d07aded12c5672d136209cbd81c3fe7f3c6c8aed/> refactor link management commands
   This commit moves the link management commands (get, put, query) into
   the new cmd/ structure.
 - <csr-id-40d48e707ff5e9d2cb71f9eed574264e592830bc/> update deps to use RepoRef
 - <csr-id-0b47bfecfe649d4776ec8758a927f3db9ea20dc0/> create helper for file override update
 - <csr-id-122a3d4c6979d53e4e62dcb3db7eae0bd4659687/> manipulate wkg config directly
 - <csr-id-f65c2aa4d2238b46365ec8d449a547b17fa5eef0/> rename interface to target in config
 - <csr-id-6b52904092f2218567e8eeb0d7dea7f6ae79805f/> remove redundant string mapping of version opts
 - <csr-id-caabfee08e8d61a162c19030fd039f5da9553a27/> get wadm_version the same way as wasmcloud_version
 - <csr-id-7ac230063453c2f4b4e3c3c48af1895af6b508d0/> use wildcard-supporting core parser
 - <csr-id-e5c38a85f4625a97eaea0863b90b0e9c7eac455a/> add help-markdown
 - <csr-id-45428ea8dc9f66c99ce53f50b4d40e0a63a3e7bd/> reorder dev printlns for consistent output

### Style

 - <csr-id-1f7ddbfaa2eb2cbb6098ef1676ecca45c8b8bb26/> top level help is colorized with dynamic length

### Test

 - <csr-id-16fba55577311b3030a93c2c7d870903283d1cce/> add tests for wash help short and help markdown
 - <csr-id-8afedb358b84da36a4760c908dc72de60619cf58/> override multiple interface integration
 - <csr-id-59c10a0f16540368e473bc47aef447881376d76e/> dev integration tests for multiple hosts

### Bug Fixes (BREAKING)

 - <csr-id-408bfb002630e8742fe64e2773d5149f28cacc3d/> use extended interface overrides everywhere
 - <csr-id-4be0d9d47190b34dbd20590a70fc40cb7482ba1f/> disable loading from stdin
   This option was evidently confusing, and would cause `wash app deploy`
   to hang for a bit while waiting for STDIN. It is likely better to
   force users to pass `-` directly to read from STDIN.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 59 commits contributed to the release over the course of 55 calendar days.
 - 62 days passed between releases.
 - 59 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Deserialize snap_data as borrowed slice ([`8eb0865`](https://github.com/wasmCloud/wasmCloud/commit/8eb0865699e9d45bf2769df84a5bfe2e2ec4e579))
    - Update wadm to 0.19 ([`da3a0f3`](https://github.com/wasmCloud/wasmCloud/commit/da3a0f39914e3d2bc96100d118daeb66c24a9132))
    - Remove extra print statement ([`7ae88df`](https://github.com/wasmCloud/wasmCloud/commit/7ae88df089785ebd85dff23f936a4d90bc5f69c9))
    - Ignore clippy zombie process warning ([`3d086ab`](https://github.com/wasmCloud/wasmCloud/commit/3d086ab544bd82973a9206940e77edf86970bc8a))
    - Regression test for buffered stdout writes ([`563f2dd`](https://github.com/wasmCloud/wasmCloud/commit/563f2dd5d0558fce33445bb3f0f80a494a722e51))
    - Test wash dev uses buffered writes after ctrl-c ([`f251e8f`](https://github.com/wasmCloud/wasmCloud/commit/f251e8fd773a6896b51040317e26fd3c70df2010))
    - Convert usages of wasm32-wasi to wasm32-wasip1 ([`fd3d776`](https://github.com/wasmCloud/wasmCloud/commit/fd3d77646f8fc8c644cd225303ab1e2855c44c70))
    - Add support for deleting all links ([`8a3a472`](https://github.com/wasmCloud/wasmCloud/commit/8a3a47298647bbcd1d3a46f6b29df542f5dc4a7c))
    - Refactor link management commands ([`d07aded`](https://github.com/wasmCloud/wasmCloud/commit/d07aded12c5672d136209cbd81c3fe7f3c6c8aed))
    - Use extended interface overrides everywhere ([`408bfb0`](https://github.com/wasmCloud/wasmCloud/commit/408bfb002630e8742fe64e2773d5149f28cacc3d))
    - Add wasmCloud/typescript repo as source for washboard downloads ([`b47fc33`](https://github.com/wasmCloud/wasmCloud/commit/b47fc3357bc4f07358a6801114ff47c2b67d2357))
    - Removes network calls from unit tests ([`6d7b07d`](https://github.com/wasmCloud/wasmCloud/commit/6d7b07dd411e2b5f9692c95c3ee807f7206da5dd))
    - Improve tests for help usage ([`24789df`](https://github.com/wasmCloud/wasmCloud/commit/24789dff476f3ed3a64faefa2222d8ea4fe77bdd))
    - Add tests for wash help short and help markdown ([`16fba55`](https://github.com/wasmCloud/wasmCloud/commit/16fba55577311b3030a93c2c7d870903283d1cce))
    - Spinner rendering error ([`02b001c`](https://github.com/wasmCloud/wasmCloud/commit/02b001c5b221c9748f9c969911a5ec16d47c2ca7))
    - Correct regex and add tests ([`d11aae8`](https://github.com/wasmCloud/wasmCloud/commit/d11aae89f227308786153e85e876f7166a0f1e0a))
    - Detect host_id from structured logs ([`1540eb0`](https://github.com/wasmCloud/wasmCloud/commit/1540eb02a9cf5acba52bce1a84003141fd43adda))
    - Fix clippy ([`da5da36`](https://github.com/wasmCloud/wasmCloud/commit/da5da3686be4f750435d859250abe5db7fb5d4f6))
    - Remove query params from git repo URL if present ([`13026fd`](https://github.com/wasmCloud/wasmCloud/commit/13026fdd3cf6348b72a8cb0d046fdd49307280fb))
    - Add more tests for wkg fetching ([`d075c55`](https://github.com/wasmCloud/wasmCloud/commit/d075c5518a4089938e1c316881846e04f931743b))
    - Update deps to use RepoRef ([`40d48e7`](https://github.com/wasmCloud/wasmCloud/commit/40d48e707ff5e9d2cb71f9eed574264e592830bc))
    - Create helper for file override update ([`0b47bfe`](https://github.com/wasmCloud/wasmCloud/commit/0b47bfecfe649d4776ec8758a927f3db9ea20dc0))
    - Manipulate wkg config directly ([`122a3d4`](https://github.com/wasmCloud/wasmCloud/commit/122a3d4c6979d53e4e62dcb3db7eae0bd4659687))
    - Rename interface to target in config ([`f65c2aa`](https://github.com/wasmCloud/wasmCloud/commit/f65c2aa4d2238b46365ec8d449a547b17fa5eef0))
    - Add extended wkg config  git repo support ([`bdaa78f`](https://github.com/wasmCloud/wasmCloud/commit/bdaa78f92872bce526b515785f3fe66984a68e6d))
    - Implement pulling from HTTP ([`401ec48`](https://github.com/wasmCloud/wasmCloud/commit/401ec48c169bbd88f624122a1d9b43b30971e694))
    - Support enhanced wkg pull config ([`cbd8440`](https://github.com/wasmCloud/wasmCloud/commit/cbd84402474d3db44641267d470e573c71059288))
    - Disable loading from stdin ([`4be0d9d`](https://github.com/wasmCloud/wasmCloud/commit/4be0d9d47190b34dbd20590a70fc40cb7482ba1f))
    - Dont require NATS to start ([`ed17160`](https://github.com/wasmCloud/wasmCloud/commit/ed17160abcfabbe9dc4d1e5702a26f5398e6f2e3))
    - Name default padding variable for commands ([`174d962`](https://github.com/wasmCloud/wasmCloud/commit/174d9628f3297884d8815d86c00f622045859fef))
    - Top level help is colorized with dynamic length ([`1f7ddbf`](https://github.com/wasmCloud/wasmCloud/commit/1f7ddbfaa2eb2cbb6098ef1676ecca45c8b8bb26))
    - Corrected formatting ([`ed28fa6`](https://github.com/wasmCloud/wasmCloud/commit/ed28fa64363508ecc29c91e99a3d57af56645f2d))
    - Wash dev with JSON output will panic when piped into jq ([`d51e9f2`](https://github.com/wasmCloud/wasmCloud/commit/d51e9f24292b850188b5212cc06169a4464a6545))
    - Add version to ui command output ([`13e0053`](https://github.com/wasmCloud/wasmCloud/commit/13e0053c19bec4d6775dabc816d068ec1911a0cf))
    - Use existing get_download_client method ([`0245805`](https://github.com/wasmCloud/wasmCloud/commit/0245805d560aed6cd667c891a245cee9c08e0d77))
    - Add new url to check for washboard releases ([`844589f`](https://github.com/wasmCloud/wasmCloud/commit/844589f49ce16876a067ac580a0e59c84f751cb9))
    - Remove redundant string mapping of version opts ([`6b52904`](https://github.com/wasmCloud/wasmCloud/commit/6b52904092f2218567e8eeb0d7dea7f6ae79805f))
    - Get wadm_version the same way as wasmcloud_version ([`caabfee`](https://github.com/wasmCloud/wasmCloud/commit/caabfee08e8d61a162c19030fd039f5da9553a27))
    - Update wasmcloudOpts and wadmOpts for wash up command ([`184bf2a`](https://github.com/wasmCloud/wasmCloud/commit/184bf2a59d6ce7e5d022f8b48379eaec85ef0aca))
    - Increase verbosity to trace in example providers ([`eedfc79`](https://github.com/wasmCloud/wasmCloud/commit/eedfc79794cdfd5d366dabc6e1d782f32e96e8ea))
    - Wadm fetch new version can fail ([`709d9b1`](https://github.com/wasmCloud/wasmCloud/commit/709d9b1e35e051b5b987e4d4205548af37174048))
    - Wash up is trying to fetch wadm and wasmcloud patch versions to download ([`5a377bc`](https://github.com/wasmCloud/wasmCloud/commit/5a377bcba1bf8a1907a457b78ca503a81202debf))
    - Removed unused dependencies ([`eb52eca`](https://github.com/wasmCloud/wasmCloud/commit/eb52eca817fe24b33e7f1a65c1ba5c46c50bef4e))
    - Override multiple interface integration ([`8afedb3`](https://github.com/wasmCloud/wasmCloud/commit/8afedb358b84da36a4760c908dc72de60619cf58))
    - Use wildcard-supporting core parser ([`7ac2300`](https://github.com/wasmCloud/wasmCloud/commit/7ac230063453c2f4b4e3c3c48af1895af6b508d0))
    - Configure nats-kv policy for secrets ([`8bd6b09`](https://github.com/wasmCloud/wasmCloud/commit/8bd6b09fb3bcd956c7ab2b7ded3fe91813800fa0))
    - Support wildcard interface overrides ([`286aa22`](https://github.com/wasmCloud/wasmCloud/commit/286aa22076d4fe58592c85f381954923a55c9e3b))
    - Support interface wildcard overrides ([`a3bff20`](https://github.com/wasmCloud/wasmCloud/commit/a3bff20215cea20116e427da92bb5ad7abf85690))
    - Add help-markdown ([`e5c38a8`](https://github.com/wasmCloud/wasmCloud/commit/e5c38a85f4625a97eaea0863b90b0e9c7eac455a))
    - Release package updates ([`bc23421`](https://github.com/wasmCloud/wasmCloud/commit/bc23421b78dcf5d9f017fe2f877ae026b5f96269))
    - Add curve to keys gen help ([`57ff6d1`](https://github.com/wasmCloud/wasmCloud/commit/57ff6d1067a6584aacfc614e594ce9c20e9c92b1))
    - Dev integration tests for multiple hosts ([`59c10a0`](https://github.com/wasmCloud/wasmCloud/commit/59c10a0f16540368e473bc47aef447881376d76e))
    - Don't fail on removing host pid ([`36d6dfd`](https://github.com/wasmCloud/wasmCloud/commit/36d6dfdb619b40dc581e353038b5dc07f026ff7e))
    - Kill wadm even if CTRL+c doesn't propagate ([`0005eb5`](https://github.com/wasmCloud/wasmCloud/commit/0005eb551f5692fdce83cc9cc0613d4fef85fc33))
    - Use spinner to indicate build progress ([`70bc37b`](https://github.com/wasmCloud/wasmCloud/commit/70bc37b9628e0f21648285c18c752ce966d0e167))
    - Ensure dev uses a single running host ([`5fcdfca`](https://github.com/wasmCloud/wasmCloud/commit/5fcdfca3373cef428d9820d2ed015b1addc5a81a))
    - Reorder dev printlns for consistent output ([`45428ea`](https://github.com/wasmCloud/wasmCloud/commit/45428ea8dc9f66c99ce53f50b4d40e0a63a3e7bd))
    - Reword some help text ([`6210e5d`](https://github.com/wasmCloud/wasmCloud/commit/6210e5de93001cc9611e81bd89f7e2b5ab26d4d3))
    - Remove weld-codegen dependency ([`9622399`](https://github.com/wasmCloud/wasmCloud/commit/9622399aeff74d2a79c3bed5980780fc57d98967))
</details>

## v0.37.1 (2024-10-23)

<csr-id-8e24533a1eed7acda3ccd4e8cf3694e63b6ab680/>
<csr-id-859d27aea99af1ba655714abb112b1662d9f9b64/>
<csr-id-c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33/>
<csr-id-754c9d0cbd28daeb8902196056a35a3b0796e004/>
<csr-id-b5de009a0c04926d5a953f7eefe6889315cfe30a/>
<csr-id-46a7f2850a548a6be27e1262cbde105fe02d37be/>
<csr-id-d86dfe09ba6dbe53fc57e3828ad718bc0d42d679/>
<csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/>
<csr-id-28be2bc7f5cba796f46be3ae9e63e89a4614d2a4/>
<csr-id-ba4f6f407aa5819d18c5a1688fc6cd9a7daef1ee/>
<csr-id-9df2bb1754fbffc36ed03a00098831eca49f3171/>
<csr-id-bff14fac85ce4673a1abe432e152067f506fb994/>
<csr-id-1b52b0f8fd58724929e5239f265641448beb03b6/>
<csr-id-da7b2cd26cdc4c929e45dc4aee4e5b092c3b26f9/>
<csr-id-30a7dacf19254c7e0e0762f5c6b007cfc27ad1f0/>
<csr-id-1592e7d57c08adc420dcbc8407234b84bbf6f3f6/>
<csr-id-68ce98bfa00a7b270489dd206aa0f237b9e3af77/>
<csr-id-4d4c8818d1d607ba64d21ca3115d199f3176de77/>
<csr-id-de73278b2730f19e71d3d08996d00f205d9559cf/>

### Chore

 - <csr-id-8e24533a1eed7acda3ccd4e8cf3694e63b6ab680/> skipping dependency upgrade test
 - <csr-id-859d27aea99af1ba655714abb112b1662d9f9b64/> bump washboard version
 - <csr-id-c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33/> bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, wasmcloud-host v0.22.0, wasmcloud-runtime v0.6.0, wasmcloud-test-util v0.14.0
 - <csr-id-754c9d0cbd28daeb8902196056a35a3b0796e004/> update wasmcloud to 1.4
 - <csr-id-b5de009a0c04926d5a953f7eefe6889315cfe30a/> NATS doesn't listen on http
 - <csr-id-46a7f2850a548a6be27e1262cbde105fe02d37be/> add cargo-binstall support for wash-cli
 - <csr-id-d86dfe09ba6dbe53fc57e3828ad718bc0d42d679/> remove TODOs from wash dev
 - <csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/> bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, wasmcloud-host v0.21.0, wasmcloud-runtime v0.5.0, wasmcloud-test-util v0.13.0
 - <csr-id-28be2bc7f5cba796f46be3ae9e63e89a4614d2a4/> remove async-nats v0.33.0
 - <csr-id-ba4f6f407aa5819d18c5a1688fc6cd9a7daef1ee/> dedup wit deps
 - <csr-id-9df2bb1754fbffc36ed03a00098831eca49f3171/> standardize emoji usage

### New Features

 - <csr-id-347d1e8ad24ba36b7d5f8514f851f94dec177798/> dev ignore gen directory
 - <csr-id-1b70cfafed45553f62226d8b9644e96ef1e1e3ec/> update wadm to 0.18
 - <csr-id-c0520722f5b4543f702c0f13fa75630be8008d9c/> Adds support for wasmcloud named things for packages
   This also integrates the wkg.toml stuff directly into wasmcloud.toml
 - <csr-id-93f5bc247da89de1fe3fcb1f1ae7efd2af9a4e05/> add wit_path, path, and reg insecure to wasmcloud.toml
 - <csr-id-f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2/> Updates tests and examples to support the new wkg deps
   This updates all dependencies to have a wkg.lock but I didn't add to the
   gitignore for convenience. The deps are still committed in tree for backwards
   compatibility and they all use the new versioned logging. This looks
   really chunky bust is mostly dep updates/deletes
 - <csr-id-c08409adfeac3f6f0b28f3d24a1346899a636b57/> implement interface driven overrides
 - <csr-id-900e881b0e59457c17a9ef1e8d53ac0f09c046f9/> Adds a wit publish command
   This is almost exactly the same as `wkg publish` but exposed for
   convenience within wash
 - <csr-id-7e3d2d06ec542293b8ed3c86734b5401b4e080cb/> Adds monkey patching for wasi-logging
   Yep, this is ugly. There is no way to sugar coat it. But this is probably
   the easiest way around the fact that we can't work with unversioned deps.
   Good news is we can remove it once we hit 2.0
 - <csr-id-be905ac395fe42e3c4ac3796ee7acc4d8d76df53/> support loading existing manifests for `wash dev`
   This commit adds support for loading and using a manifest for use
   during development with `wash dev`.
   
   When manifest overrides are specified, the existing manifest will be
   used, and no new manifests will be generated.
 - <csr-id-63896d3b7860c7213d7173cd213498f9f1b87e01/> bump wadm 0.16.1, wasmcloud 1.3, NATS 2.10.20
 - <csr-id-9769a3c293a70b3b1f326d4589eaab5d1d177ef6/> log NATS config and log path

### Bug Fixes

 - <csr-id-344eeabe210963a5d3e4dbbae1f762941a2aa7ec/> delete manifest when dev stops
 - <csr-id-08205e519b9a0bb5f08f3d0f7a626db670aa1c08/> ensure dev reloads when deps change
 - <csr-id-92b210fdfb89be59c45665aa62c130d111748f4f/> use package as package
 - <csr-id-21bedaf7f1cd45bb19f86ed97ec812426c82ebe3/> Mark doctests for crate-visible methods as ignored
 - <csr-id-a3a53ea958807a43dc6f288b5d4321923ed19d7c/> Fix leaked wadm process from integration tests
 - <csr-id-1d3347f342bfb189ea0baf17604438a6ff9411d1/> honor nats settings, change session ID check
 - <csr-id-f30e6cd11097f3f061de77c0dc2439c4de9d971a/> ignore failed generation of components in `wash dev`
   This commit allows failure of WADM manifest component generation in wash dev,
   ignoring it if it occurs.
   
   Generation of WADM manifest components is likely to fail for *new*
   interfaces that may be imported or exported -- likely unknown
   interfaces that we can't possibly generate a known component for.
 - <csr-id-c8f39c43ea3d6a72612033e82bc2d974bd142035/> support messaging-nats dev
 - <csr-id-1a07544c5f8959b4dcb2c7e4078984681ba72437/> differentiate config, support fast-reload
 - <csr-id-873faa61250bbb2f7913cf1d6213efaf75c94f7f/> continue dev loop when build fails
 - <csr-id-a59201a2226a3d005c8c98c8a6874f240eb9eaf3/> combine links

### Other

 - <csr-id-bff14fac85ce4673a1abe432e152067f506fb994/> correct tests with new functionality
 - <csr-id-1b52b0f8fd58724929e5239f265641448beb03b6/> upgrade to 0.17.0

### Refactor

 - <csr-id-da7b2cd26cdc4c929e45dc4aee4e5b092c3b26f9/> address PR nits and clippy warnings
 - <csr-id-30a7dacf19254c7e0e0762f5c6b007cfc27ad1f0/> remove raw structs, simplify deserialization
 - <csr-id-1592e7d57c08adc420dcbc8407234b84bbf6f3f6/> refactor the code for wash dev
 - <csr-id-68ce98bfa00a7b270489dd206aa0f237b9e3af77/> manifest editing
 - <csr-id-4d4c8818d1d607ba64d21ca3115d199f3176de77/> adjust for control-interface v2.2.0

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

### Bug Fixes (BREAKING)

 - <csr-id-8fbaab298a5ac5749d89259871d829bf2cbfb1f8/> remove duplicate components, ignore generated paths

## v0.37.0 (2024-11-12)

<csr-id-7bcf972e9403b53c0ae113def6a6e9546157a9d4/>
<csr-id-71d2610ff1689fd3dd0278d3164ca6c7703a7ad7/>
<csr-id-6d250ffa473385baae59b6f83b35ff38f119c054/>
<csr-id-98cd06c1dd448073d8eadef790ee7709a2ccae36/>
<csr-id-993771423527f8c1e15acf0b5a202206228f790e/>
<csr-id-1682afe130cbd2be41cbdb6d3a8e89a2d63e59fc/>
<csr-id-8c96324fc0b28a475073974b23451e19736070ae/>
<csr-id-ec4ed44b01564f423cc7a615c9ee2860d4f55925/>
<csr-id-9863a724c43207e282452b09c8f51fdacac44ec0/>
<csr-id-9bb420c1ff7a706b6ab33b09eb5af6461b63d166/>
<csr-id-599c52b358e4ff940fcc9b3d6b0ec27a6bb30ad2/>

### Chore

 - <csr-id-7bcf972e9403b53c0ae113def6a6e9546157a9d4/> bump dev http-server to v0.24.0
 - <csr-id-71d2610ff1689fd3dd0278d3164ca6c7703a7ad7/> bump wasmCloud to v1.4.1
 - <csr-id-6d250ffa473385baae59b6f83b35ff38f119c054/> update `wrpc-transport-nats`
 - <csr-id-98cd06c1dd448073d8eadef790ee7709a2ccae36/> fix clippy lint
 - <csr-id-993771423527f8c1e15acf0b5a202206228f790e/> update http-server to v0.23.2
 - <csr-id-1682afe130cbd2be41cbdb6d3a8e89a2d63e59fc/> Deprecate docker job
 - <csr-id-8c96324fc0b28a475073974b23451e19736070ae/> remove snap building
   This commit removes the building of the wash-cli snap -- it was out of
   use and had fallen behind/was broken, so for now we'll remove it in
   favor of users downloading and installing `wash-cli` via other means.
 - <csr-id-ec4ed44b01564f423cc7a615c9ee2860d4f55925/> update all cargo wash-cli install commands to --locked

### Other

 - <csr-id-641c34e063faf30308e1bdaf914cb312456fa55f/> download wasmcloud 1.4.2
 - <csr-id-ef45f597710929d41be989110fc3c51621c9ee62/> bump wascap v0.15.2, provider-archive v0.14.0, wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wasmcloud-secrets-types v0.5.0, wash-cli v0.37.0, safety bump 9 crates
   SAFETY BUMP: wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wash-cli v0.37.0, wasmcloud-host v0.23.0, wasmcloud-runtime v0.7.0, wasmcloud-test-util v0.15.0, wasmcloud-secrets-client v0.6.0

### Chore

 - <csr-id-e555a1e8291e2282cda1b25893c78a129c230a52/> Remove unnecessary dependency on wasmcloud-provider-sdk

### New Features

 - <csr-id-22ac2b0cc123f00267dc8be4a55b9fd5f9a00713/> tls-first option for command line
 - <csr-id-6e113b097402e059ef7e2d5a01d7971e5b139cfa/> Add support for monolithic push

### Bug Fixes

 - <csr-id-940a06d42476a501b090a4bf4eecc90ccbe6c3ce/> Fixes wasmcloud.lock on windows
   This is a simple dependency bump to pick up the fix from wkg on locking
   files on windows
 - <csr-id-be4ea5617776a3c7d4976bd0bc5f901049e021d7/> avoid an immediate reload
 - <csr-id-b1f9e9a14c2bfed241d741d590fe926b940cfaa2/> prevent duplicate reloads
 - <csr-id-02bd8dfe82b839732b96d128b008dd83de3ed61a/> wash build tests for tinygo

### Refactor

 - <csr-id-9863a724c43207e282452b09c8f51fdacac44ec0/> deprecate wash app list in favor of wash app get
 - <csr-id-9bb420c1ff7a706b6ab33b09eb5af6461b63d166/> disallow unnamed links del when multiple present
   This commit restricts `wash link del` from deleting a link when there
   are multiple stored that have similar targeting properties except for
   link name.
   
   This situation *can* lead to deleting the *wrong* link or deleting one
   intentionally, so users must now specify the link name, rather than
   getting `"default"` set for them.

### Test

 - <csr-id-599c52b358e4ff940fcc9b3d6b0ec27a6bb30ad2/> correct separate path test

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 20 commits contributed to the release over the course of 19 calendar days.
 - 19 days passed between releases.
 - 20 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Remove unnecessary dependency on wasmcloud-provider-sdk ([`e555a1e`](https://github.com/wasmCloud/wasmCloud/commit/e555a1e8291e2282cda1b25893c78a129c230a52))
    - Download wasmcloud 1.4.2 ([`641c34e`](https://github.com/wasmCloud/wasmCloud/commit/641c34e063faf30308e1bdaf914cb312456fa55f))
    - Bump wascap v0.15.2, provider-archive v0.14.0, wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wasmcloud-secrets-types v0.5.0, wash-cli v0.37.0, safety bump 9 crates ([`ef45f59`](https://github.com/wasmCloud/wasmCloud/commit/ef45f597710929d41be989110fc3c51621c9ee62))
    - Bump dev http-server to v0.24.0 ([`7bcf972`](https://github.com/wasmCloud/wasmCloud/commit/7bcf972e9403b53c0ae113def6a6e9546157a9d4))
    - Bump wasmCloud to v1.4.1 ([`71d2610`](https://github.com/wasmCloud/wasmCloud/commit/71d2610ff1689fd3dd0278d3164ca6c7703a7ad7))
    - Update `wrpc-transport-nats` ([`6d250ff`](https://github.com/wasmCloud/wasmCloud/commit/6d250ffa473385baae59b6f83b35ff38f119c054))
    - Fixes wasmcloud.lock on windows ([`940a06d`](https://github.com/wasmCloud/wasmCloud/commit/940a06d42476a501b090a4bf4eecc90ccbe6c3ce))
    - Avoid an immediate reload ([`be4ea56`](https://github.com/wasmCloud/wasmCloud/commit/be4ea5617776a3c7d4976bd0bc5f901049e021d7))
    - Fix clippy lint ([`98cd06c`](https://github.com/wasmCloud/wasmCloud/commit/98cd06c1dd448073d8eadef790ee7709a2ccae36))
    - Tls-first option for command line ([`22ac2b0`](https://github.com/wasmCloud/wasmCloud/commit/22ac2b0cc123f00267dc8be4a55b9fd5f9a00713))
    - Prevent duplicate reloads ([`b1f9e9a`](https://github.com/wasmCloud/wasmCloud/commit/b1f9e9a14c2bfed241d741d590fe926b940cfaa2))
    - Correct separate path test ([`599c52b`](https://github.com/wasmCloud/wasmCloud/commit/599c52b358e4ff940fcc9b3d6b0ec27a6bb30ad2))
    - Update http-server to v0.23.2 ([`9937714`](https://github.com/wasmCloud/wasmCloud/commit/993771423527f8c1e15acf0b5a202206228f790e))
    - Deprecate wash app list in favor of wash app get ([`9863a72`](https://github.com/wasmCloud/wasmCloud/commit/9863a724c43207e282452b09c8f51fdacac44ec0))
    - Disallow unnamed links del when multiple present ([`9bb420c`](https://github.com/wasmCloud/wasmCloud/commit/9bb420c1ff7a706b6ab33b09eb5af6461b63d166))
    - Wash build tests for tinygo ([`02bd8df`](https://github.com/wasmCloud/wasmCloud/commit/02bd8dfe82b839732b96d128b008dd83de3ed61a))
    - Deprecate docker job ([`1682afe`](https://github.com/wasmCloud/wasmCloud/commit/1682afe130cbd2be41cbdb6d3a8e89a2d63e59fc))
    - Add support for monolithic push ([`6e113b0`](https://github.com/wasmCloud/wasmCloud/commit/6e113b097402e059ef7e2d5a01d7971e5b139cfa))
    - Remove snap building ([`8c96324`](https://github.com/wasmCloud/wasmCloud/commit/8c96324fc0b28a475073974b23451e19736070ae))
    - Update all cargo wash-cli install commands to --locked ([`ec4ed44`](https://github.com/wasmCloud/wasmCloud/commit/ec4ed44b01564f423cc7a615c9ee2860d4f55925))
</details>

## v0.36.1 (2024-10-23)

<csr-id-8e24533a1eed7acda3ccd4e8cf3694e63b6ab680/>
<csr-id-859d27aea99af1ba655714abb112b1662d9f9b64/>
<csr-id-c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33/>
<csr-id-754c9d0cbd28daeb8902196056a35a3b0796e004/>
<csr-id-b5de009a0c04926d5a953f7eefe6889315cfe30a/>
<csr-id-46a7f2850a548a6be27e1262cbde105fe02d37be/>
<csr-id-d86dfe09ba6dbe53fc57e3828ad718bc0d42d679/>
<csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/>
<csr-id-28be2bc7f5cba796f46be3ae9e63e89a4614d2a4/>
<csr-id-ba4f6f407aa5819d18c5a1688fc6cd9a7daef1ee/>
<csr-id-9df2bb1754fbffc36ed03a00098831eca49f3171/>
<csr-id-452af3e086d8067c5342f545719daf567a2c1c01/>
<csr-id-bff14fac85ce4673a1abe432e152067f506fb994/>
<csr-id-1b52b0f8fd58724929e5239f265641448beb03b6/>
<csr-id-da7b2cd26cdc4c929e45dc4aee4e5b092c3b26f9/>
<csr-id-30a7dacf19254c7e0e0762f5c6b007cfc27ad1f0/>
<csr-id-1592e7d57c08adc420dcbc8407234b84bbf6f3f6/>
<csr-id-68ce98bfa00a7b270489dd206aa0f237b9e3af77/>
<csr-id-4d4c8818d1d607ba64d21ca3115d199f3176de77/>
<csr-id-de73278b2730f19e71d3d08996d00f205d9559cf/>

### Chore

 - <csr-id-8e24533a1eed7acda3ccd4e8cf3694e63b6ab680/> skipping dependency upgrade test
 - <csr-id-859d27aea99af1ba655714abb112b1662d9f9b64/> bump washboard version
 - <csr-id-c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33/> bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, wasmcloud-host v0.22.0, wasmcloud-runtime v0.6.0, wasmcloud-test-util v0.14.0
 - <csr-id-754c9d0cbd28daeb8902196056a35a3b0796e004/> update wasmcloud to 1.4
 - <csr-id-b5de009a0c04926d5a953f7eefe6889315cfe30a/> NATS doesn't listen on http
 - <csr-id-46a7f2850a548a6be27e1262cbde105fe02d37be/> add cargo-binstall support for wash-cli
 - <csr-id-d86dfe09ba6dbe53fc57e3828ad718bc0d42d679/> remove TODOs from wash dev
 - <csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/> bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, wasmcloud-host v0.21.0, wasmcloud-runtime v0.5.0, wasmcloud-test-util v0.13.0
 - <csr-id-28be2bc7f5cba796f46be3ae9e63e89a4614d2a4/> remove async-nats v0.33.0
 - <csr-id-ba4f6f407aa5819d18c5a1688fc6cd9a7daef1ee/> dedup wit deps
 - <csr-id-9df2bb1754fbffc36ed03a00098831eca49f3171/> standardize emoji usage

### New Features

 - <csr-id-347d1e8ad24ba36b7d5f8514f851f94dec177798/> dev ignore gen directory
 - <csr-id-1b70cfafed45553f62226d8b9644e96ef1e1e3ec/> update wadm to 0.18
 - <csr-id-c0520722f5b4543f702c0f13fa75630be8008d9c/> Adds support for wasmcloud named things for packages
   This also integrates the wkg.toml stuff directly into wasmcloud.toml
 - <csr-id-93f5bc247da89de1fe3fcb1f1ae7efd2af9a4e05/> add wit_path, path, and reg insecure to wasmcloud.toml
 - <csr-id-f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2/> Updates tests and examples to support the new wkg deps
   This updates all dependencies to have a wkg.lock but I didn't add to the
   gitignore for convenience. The deps are still committed in tree for backwards
   compatibility and they all use the new versioned logging. This looks
   really chunky bust is mostly dep updates/deletes
 - <csr-id-c08409adfeac3f6f0b28f3d24a1346899a636b57/> implement interface driven overrides
 - <csr-id-900e881b0e59457c17a9ef1e8d53ac0f09c046f9/> Adds a wit publish command
   This is almost exactly the same as `wkg publish` but exposed for
   convenience within wash
 - <csr-id-7e3d2d06ec542293b8ed3c86734b5401b4e080cb/> Adds monkey patching for wasi-logging
   Yep, this is ugly. There is no way to sugar coat it. But this is probably
   the easiest way around the fact that we can't work with unversioned deps.
   Good news is we can remove it once we hit 2.0
 - <csr-id-be905ac395fe42e3c4ac3796ee7acc4d8d76df53/> support loading existing manifests for `wash dev`
   This commit adds support for loading and using a manifest for use
   during development with `wash dev`.
   
   When manifest overrides are specified, the existing manifest will be
   used, and no new manifests will be generated.
 - <csr-id-63896d3b7860c7213d7173cd213498f9f1b87e01/> bump wadm 0.16.1, wasmcloud 1.3, NATS 2.10.20
 - <csr-id-9769a3c293a70b3b1f326d4589eaab5d1d177ef6/> log NATS config and log path

### Bug Fixes

 - <csr-id-344eeabe210963a5d3e4dbbae1f762941a2aa7ec/> delete manifest when dev stops
 - <csr-id-08205e519b9a0bb5f08f3d0f7a626db670aa1c08/> ensure dev reloads when deps change
 - <csr-id-92b210fdfb89be59c45665aa62c130d111748f4f/> use package as package
 - <csr-id-21bedaf7f1cd45bb19f86ed97ec812426c82ebe3/> Mark doctests for crate-visible methods as ignored
 - <csr-id-a3a53ea958807a43dc6f288b5d4321923ed19d7c/> Fix leaked wadm process from integration tests
 - <csr-id-1d3347f342bfb189ea0baf17604438a6ff9411d1/> honor nats settings, change session ID check
 - <csr-id-f30e6cd11097f3f061de77c0dc2439c4de9d971a/> ignore failed generation of components in `wash dev`
   This commit allows failure of WADM manifest component generation in wash dev,
   ignoring it if it occurs.
   
   Generation of WADM manifest components is likely to fail for *new*
   interfaces that may be imported or exported -- likely unknown
   interfaces that we can't possibly generate a known component for.
 - <csr-id-c8f39c43ea3d6a72612033e82bc2d974bd142035/> support messaging-nats dev
 - <csr-id-1a07544c5f8959b4dcb2c7e4078984681ba72437/> differentiate config, support fast-reload
 - <csr-id-873faa61250bbb2f7913cf1d6213efaf75c94f7f/> continue dev loop when build fails
 - <csr-id-a59201a2226a3d005c8c98c8a6874f240eb9eaf3/> combine links

### Other

 - <csr-id-452af3e086d8067c5342f545719daf567a2c1c01/> wash-cli v0.36.1
 - <csr-id-bff14fac85ce4673a1abe432e152067f506fb994/> correct tests with new functionality
 - <csr-id-1b52b0f8fd58724929e5239f265641448beb03b6/> upgrade to 0.17.0

### Refactor

 - <csr-id-da7b2cd26cdc4c929e45dc4aee4e5b092c3b26f9/> address PR nits and clippy warnings
 - <csr-id-30a7dacf19254c7e0e0762f5c6b007cfc27ad1f0/> remove raw structs, simplify deserialization
 - <csr-id-1592e7d57c08adc420dcbc8407234b84bbf6f3f6/> refactor the code for wash dev
 - <csr-id-68ce98bfa00a7b270489dd206aa0f237b9e3af77/> manifest editing
 - <csr-id-4d4c8818d1d607ba64d21ca3115d199f3176de77/> adjust for control-interface v2.2.0

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

### Bug Fixes (BREAKING)

 - <csr-id-8fbaab298a5ac5749d89259871d829bf2cbfb1f8/> remove duplicate components, ignore generated paths

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 45 commits contributed to the release over the course of 21 calendar days.
 - 23 days passed between releases.
 - 45 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Wash-cli v0.36.1 ([`452af3e`](https://github.com/wasmCloud/wasmCloud/commit/452af3e086d8067c5342f545719daf567a2c1c01))
    - Skipping dependency upgrade test ([`8e24533`](https://github.com/wasmCloud/wasmCloud/commit/8e24533a1eed7acda3ccd4e8cf3694e63b6ab680))
    - Delete manifest when dev stops ([`344eeab`](https://github.com/wasmCloud/wasmCloud/commit/344eeabe210963a5d3e4dbbae1f762941a2aa7ec))
    - Bump washboard version ([`859d27a`](https://github.com/wasmCloud/wasmCloud/commit/859d27aea99af1ba655714abb112b1662d9f9b64))
    - Bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates ([`c5ba85c`](https://github.com/wasmCloud/wasmCloud/commit/c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33))
    - Ensure dev reloads when deps change ([`08205e5`](https://github.com/wasmCloud/wasmCloud/commit/08205e519b9a0bb5f08f3d0f7a626db670aa1c08))
    - Use package as package ([`92b210f`](https://github.com/wasmCloud/wasmCloud/commit/92b210fdfb89be59c45665aa62c130d111748f4f))
    - Dev ignore gen directory ([`347d1e8`](https://github.com/wasmCloud/wasmCloud/commit/347d1e8ad24ba36b7d5f8514f851f94dec177798))
    - Update wadm to 0.18 ([`1b70cfa`](https://github.com/wasmCloud/wasmCloud/commit/1b70cfafed45553f62226d8b9644e96ef1e1e3ec))
    - Update wasmcloud to 1.4 ([`754c9d0`](https://github.com/wasmCloud/wasmCloud/commit/754c9d0cbd28daeb8902196056a35a3b0796e004))
    - Adds support for wasmcloud named things for packages ([`c052072`](https://github.com/wasmCloud/wasmCloud/commit/c0520722f5b4543f702c0f13fa75630be8008d9c))
    - Mark doctests for crate-visible methods as ignored ([`21bedaf`](https://github.com/wasmCloud/wasmCloud/commit/21bedaf7f1cd45bb19f86ed97ec812426c82ebe3))
    - Fix leaked wadm process from integration tests ([`a3a53ea`](https://github.com/wasmCloud/wasmCloud/commit/a3a53ea958807a43dc6f288b5d4321923ed19d7c))
    - Address PR nits and clippy warnings ([`da7b2cd`](https://github.com/wasmCloud/wasmCloud/commit/da7b2cd26cdc4c929e45dc4aee4e5b092c3b26f9))
    - Correct tests with new functionality ([`bff14fa`](https://github.com/wasmCloud/wasmCloud/commit/bff14fac85ce4673a1abe432e152067f506fb994))
    - Wash build override build/wit/project dir ([`cf09c44`](https://github.com/wasmCloud/wasmCloud/commit/cf09c44d4d08bdb9039f51e95320bf2183e3454e))
    - Add wit_path, path, and reg insecure to wasmcloud.toml ([`93f5bc2`](https://github.com/wasmCloud/wasmCloud/commit/93f5bc247da89de1fe3fcb1f1ae7efd2af9a4e05))
    - Remove raw structs, simplify deserialization ([`30a7dac`](https://github.com/wasmCloud/wasmCloud/commit/30a7dacf19254c7e0e0762f5c6b007cfc27ad1f0))
    - NATS doesn't listen on http ([`b5de009`](https://github.com/wasmCloud/wasmCloud/commit/b5de009a0c04926d5a953f7eefe6889315cfe30a))
    - Updates tests and examples to support the new wkg deps ([`f0f3fd7`](https://github.com/wasmCloud/wasmCloud/commit/f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2))
    - Refactor the code for wash dev ([`1592e7d`](https://github.com/wasmCloud/wasmCloud/commit/1592e7d57c08adc420dcbc8407234b84bbf6f3f6))
    - Add test for overriding interface ([`de73278`](https://github.com/wasmCloud/wasmCloud/commit/de73278b2730f19e71d3d08996d00f205d9559cf))
    - Implement interface driven overrides ([`c08409a`](https://github.com/wasmCloud/wasmCloud/commit/c08409adfeac3f6f0b28f3d24a1346899a636b57))
    - Adds a wit publish command ([`900e881`](https://github.com/wasmCloud/wasmCloud/commit/900e881b0e59457c17a9ef1e8d53ac0f09c046f9))
    - Add cargo-binstall support for wash-cli ([`46a7f28`](https://github.com/wasmCloud/wasmCloud/commit/46a7f2850a548a6be27e1262cbde105fe02d37be))
    - Adds monkey patching for wasi-logging ([`7e3d2d0`](https://github.com/wasmCloud/wasmCloud/commit/7e3d2d06ec542293b8ed3c86734b5401b4e080cb))
    - Adds support for fetching dependencies automatically ([`644bd35`](https://github.com/wasmCloud/wasmCloud/commit/644bd35df2a24d1f12aba4d0613a3b667db9c70b))
    - Upgrade to 0.17.0 ([`1b52b0f`](https://github.com/wasmCloud/wasmCloud/commit/1b52b0f8fd58724929e5239f265641448beb03b6))
    - Remove TODOs from wash dev ([`d86dfe0`](https://github.com/wasmCloud/wasmCloud/commit/d86dfe09ba6dbe53fc57e3828ad718bc0d42d679))
    - Manifest editing ([`68ce98b`](https://github.com/wasmCloud/wasmCloud/commit/68ce98bfa00a7b270489dd206aa0f237b9e3af77))
    - Honor nats settings, change session ID check ([`1d3347f`](https://github.com/wasmCloud/wasmCloud/commit/1d3347f342bfb189ea0baf17604438a6ff9411d1))
    - Support loading existing manifests for `wash dev` ([`be905ac`](https://github.com/wasmCloud/wasmCloud/commit/be905ac395fe42e3c4ac3796ee7acc4d8d76df53))
    - Bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates ([`44bf4c8`](https://github.com/wasmCloud/wasmCloud/commit/44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a))
    - Bump wadm 0.16.1, wasmcloud 1.3, NATS 2.10.20 ([`63896d3`](https://github.com/wasmCloud/wasmCloud/commit/63896d3b7860c7213d7173cd213498f9f1b87e01))
    - Log NATS config and log path ([`9769a3c`](https://github.com/wasmCloud/wasmCloud/commit/9769a3c293a70b3b1f326d4589eaab5d1d177ef6))
    - Adjust for control-interface v2.2.0 ([`4d4c881`](https://github.com/wasmCloud/wasmCloud/commit/4d4c8818d1d607ba64d21ca3115d199f3176de77))
    - Remove async-nats v0.33.0 ([`28be2bc`](https://github.com/wasmCloud/wasmCloud/commit/28be2bc7f5cba796f46be3ae9e63e89a4614d2a4))
    - Ignore failed generation of components in `wash dev` ([`f30e6cd`](https://github.com/wasmCloud/wasmCloud/commit/f30e6cd11097f3f061de77c0dc2439c4de9d971a))
    - Dedup wit deps ([`ba4f6f4`](https://github.com/wasmCloud/wasmCloud/commit/ba4f6f407aa5819d18c5a1688fc6cd9a7daef1ee))
    - Standardize emoji usage ([`9df2bb1`](https://github.com/wasmCloud/wasmCloud/commit/9df2bb1754fbffc36ed03a00098831eca49f3171))
    - Support messaging-nats dev ([`c8f39c4`](https://github.com/wasmCloud/wasmCloud/commit/c8f39c43ea3d6a72612033e82bc2d974bd142035))
    - Differentiate config, support fast-reload ([`1a07544`](https://github.com/wasmCloud/wasmCloud/commit/1a07544c5f8959b4dcb2c7e4078984681ba72437))
    - Continue dev loop when build fails ([`873faa6`](https://github.com/wasmCloud/wasmCloud/commit/873faa61250bbb2f7913cf1d6213efaf75c94f7f))
    - Combine links ([`a59201a`](https://github.com/wasmCloud/wasmCloud/commit/a59201a2226a3d005c8c98c8a6874f240eb9eaf3))
    - Remove duplicate components, ignore generated paths ([`8fbaab2`](https://github.com/wasmCloud/wasmCloud/commit/8fbaab298a5ac5749d89259871d829bf2cbfb1f8))
</details>

## v0.36.0 (2024-10-23)

<csr-id-754c9d0cbd28daeb8902196056a35a3b0796e004/>
<csr-id-b5de009a0c04926d5a953f7eefe6889315cfe30a/>
<csr-id-46a7f2850a548a6be27e1262cbde105fe02d37be/>
<csr-id-d86dfe09ba6dbe53fc57e3828ad718bc0d42d679/>
<csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/>
<csr-id-28be2bc7f5cba796f46be3ae9e63e89a4614d2a4/>
<csr-id-ba4f6f407aa5819d18c5a1688fc6cd9a7daef1ee/>
<csr-id-9df2bb1754fbffc36ed03a00098831eca49f3171/>
<csr-id-bff14fac85ce4673a1abe432e152067f506fb994/>
<csr-id-1b52b0f8fd58724929e5239f265641448beb03b6/>
<csr-id-da7b2cd26cdc4c929e45dc4aee4e5b092c3b26f9/>
<csr-id-30a7dacf19254c7e0e0762f5c6b007cfc27ad1f0/>
<csr-id-1592e7d57c08adc420dcbc8407234b84bbf6f3f6/>
<csr-id-68ce98bfa00a7b270489dd206aa0f237b9e3af77/>
<csr-id-4d4c8818d1d607ba64d21ca3115d199f3176de77/>
<csr-id-de73278b2730f19e71d3d08996d00f205d9559cf/>

### Chore

 - <csr-id-754c9d0cbd28daeb8902196056a35a3b0796e004/> update wasmcloud to 1.4
 - <csr-id-b5de009a0c04926d5a953f7eefe6889315cfe30a/> NATS doesn't listen on http
 - <csr-id-46a7f2850a548a6be27e1262cbde105fe02d37be/> add cargo-binstall support for wash-cli
 - <csr-id-d86dfe09ba6dbe53fc57e3828ad718bc0d42d679/> remove TODOs from wash dev
 - <csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/> bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, wasmcloud-host v0.21.0, wasmcloud-runtime v0.5.0, wasmcloud-test-util v0.13.0
 - <csr-id-28be2bc7f5cba796f46be3ae9e63e89a4614d2a4/> remove async-nats v0.33.0
 - <csr-id-ba4f6f407aa5819d18c5a1688fc6cd9a7daef1ee/> dedup wit deps
 - <csr-id-9df2bb1754fbffc36ed03a00098831eca49f3171/> standardize emoji usage

### New Features

 - <csr-id-347d1e8ad24ba36b7d5f8514f851f94dec177798/> dev ignore gen directory
 - <csr-id-1b70cfafed45553f62226d8b9644e96ef1e1e3ec/> update wadm to 0.18
 - <csr-id-c0520722f5b4543f702c0f13fa75630be8008d9c/> Adds support for wasmcloud named things for packages
   This also integrates the wkg.toml stuff directly into wasmcloud.toml
 - <csr-id-93f5bc247da89de1fe3fcb1f1ae7efd2af9a4e05/> add wit_path, path, and reg insecure to wasmcloud.toml
 - <csr-id-f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2/> Updates tests and examples to support the new wkg deps
   This updates all dependencies to have a wkg.lock but I didn't add to the
   gitignore for convenience. The deps are still committed in tree for backwards
   compatibility and they all use the new versioned logging. This looks
   really chunky bust is mostly dep updates/deletes
 - <csr-id-c08409adfeac3f6f0b28f3d24a1346899a636b57/> implement interface driven overrides
 - <csr-id-900e881b0e59457c17a9ef1e8d53ac0f09c046f9/> Adds a wit publish command
   This is almost exactly the same as `wkg publish` but exposed for
   convenience within wash
 - <csr-id-7e3d2d06ec542293b8ed3c86734b5401b4e080cb/> Adds monkey patching for wasi-logging
   Yep, this is ugly. There is no way to sugar coat it. But this is probably
   the easiest way around the fact that we can't work with unversioned deps.
   Good news is we can remove it once we hit 2.0
 - <csr-id-be905ac395fe42e3c4ac3796ee7acc4d8d76df53/> support loading existing manifests for `wash dev`
   This commit adds support for loading and using a manifest for use
   during development with `wash dev`.
   
   When manifest overrides are specified, the existing manifest will be
   used, and no new manifests will be generated.
 - <csr-id-63896d3b7860c7213d7173cd213498f9f1b87e01/> bump wadm 0.16.1, wasmcloud 1.3, NATS 2.10.20
 - <csr-id-9769a3c293a70b3b1f326d4589eaab5d1d177ef6/> log NATS config and log path

### Bug Fixes

 - <csr-id-08205e519b9a0bb5f08f3d0f7a626db670aa1c08/> ensure dev reloads when deps change
 - <csr-id-92b210fdfb89be59c45665aa62c130d111748f4f/> use package as package
 - <csr-id-21bedaf7f1cd45bb19f86ed97ec812426c82ebe3/> Mark doctests for crate-visible methods as ignored
 - <csr-id-a3a53ea958807a43dc6f288b5d4321923ed19d7c/> Fix leaked wadm process from integration tests
 - <csr-id-1d3347f342bfb189ea0baf17604438a6ff9411d1/> honor nats settings, change session ID check
 - <csr-id-f30e6cd11097f3f061de77c0dc2439c4de9d971a/> ignore failed generation of components in `wash dev`
   This commit allows failure of WADM manifest component generation in wash dev,
   ignoring it if it occurs.
   
   Generation of WADM manifest components is likely to fail for *new*
   interfaces that may be imported or exported -- likely unknown
   interfaces that we can't possibly generate a known component for.
 - <csr-id-c8f39c43ea3d6a72612033e82bc2d974bd142035/> support messaging-nats dev
 - <csr-id-1a07544c5f8959b4dcb2c7e4078984681ba72437/> differentiate config, support fast-reload
 - <csr-id-873faa61250bbb2f7913cf1d6213efaf75c94f7f/> continue dev loop when build fails
 - <csr-id-a59201a2226a3d005c8c98c8a6874f240eb9eaf3/> combine links

### Other

 - <csr-id-bff14fac85ce4673a1abe432e152067f506fb994/> correct tests with new functionality
 - <csr-id-1b52b0f8fd58724929e5239f265641448beb03b6/> upgrade to 0.17.0

### Refactor

 - <csr-id-da7b2cd26cdc4c929e45dc4aee4e5b092c3b26f9/> address PR nits and clippy warnings
 - <csr-id-30a7dacf19254c7e0e0762f5c6b007cfc27ad1f0/> remove raw structs, simplify deserialization
 - <csr-id-1592e7d57c08adc420dcbc8407234b84bbf6f3f6/> refactor the code for wash dev
 - <csr-id-68ce98bfa00a7b270489dd206aa0f237b9e3af77/> manifest editing
 - <csr-id-4d4c8818d1d607ba64d21ca3115d199f3176de77/> adjust for control-interface v2.2.0

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

### Bug Fixes (BREAKING)

 - <csr-id-8fbaab298a5ac5749d89259871d829bf2cbfb1f8/> remove duplicate components, ignore generated paths

## v0.35.0 (2024-10-09)

<csr-id-28be2bc7f5cba796f46be3ae9e63e89a4614d2a4/>
<csr-id-ba4f6f407aa5819d18c5a1688fc6cd9a7daef1ee/>
<csr-id-9df2bb1754fbffc36ed03a00098831eca49f3171/>
<csr-id-4d4c8818d1d607ba64d21ca3115d199f3176de77/>

### Chore

 - <csr-id-28be2bc7f5cba796f46be3ae9e63e89a4614d2a4/> remove async-nats v0.33.0
 - <csr-id-ba4f6f407aa5819d18c5a1688fc6cd9a7daef1ee/> dedup wit deps
 - <csr-id-9df2bb1754fbffc36ed03a00098831eca49f3171/> standardize emoji usage

### New Features

 - <csr-id-63896d3b7860c7213d7173cd213498f9f1b87e01/> bump wadm 0.16.1, wasmcloud 1.3, NATS 2.10.20
 - <csr-id-9769a3c293a70b3b1f326d4589eaab5d1d177ef6/> log NATS config and log path

### Bug Fixes

 - <csr-id-f30e6cd11097f3f061de77c0dc2439c4de9d971a/> ignore failed generation of components in `wash dev`
   This commit allows failure of WADM manifest component generation in wash dev,
   ignoring it if it occurs.
   
   Generation of WADM manifest components is likely to fail for *new*
   interfaces that may be imported or exported -- likely unknown
   interfaces that we can't possibly generate a known component for.
 - <csr-id-c8f39c43ea3d6a72612033e82bc2d974bd142035/> support messaging-nats dev
 - <csr-id-1a07544c5f8959b4dcb2c7e4078984681ba72437/> differentiate config, support fast-reload
 - <csr-id-873faa61250bbb2f7913cf1d6213efaf75c94f7f/> continue dev loop when build fails
 - <csr-id-a59201a2226a3d005c8c98c8a6874f240eb9eaf3/> combine links

### Refactor

 - <csr-id-4d4c8818d1d607ba64d21ca3115d199f3176de77/> adjust for control-interface v2.2.0

### Bug Fixes (BREAKING)

 - <csr-id-8fbaab298a5ac5749d89259871d829bf2cbfb1f8/> remove duplicate components, ignore generated paths

## v0.34.1 (2024-09-30)

<csr-id-ce1d5d1ed7c503e2cf357047a4a849f74be7b51e/>
<csr-id-d28ebe1783c7535ac30cccad1dacc70cb921f371/>
<csr-id-215eeadc555dfc8a57cee96856febf63113f62a1/>
<csr-id-e2b6287bde6c9b905d96f93f93a1a7d73940283c/>
<csr-id-e6936ce5113db8434ffb2cc927e779e677ad0bd4/>
<csr-id-b382e8669dd31e44855142d2b478421884710e3d/>
<csr-id-f8de84b251122b3319f7b1c8d86cb749cb4a8e37/>
<csr-id-6cdcf8ae093a0214787ee99132844d5cb92d8b3d/>
<csr-id-71fc4b8f60a1f5f469912b712452f1c96a7744ef/>
<csr-id-8d4c48bed132fd33cc7f4d2aaa33b1999e2ea67a/>

### Chore

 - <csr-id-ce1d5d1ed7c503e2cf357047a4a849f74be7b51e/> update blobstore-fs provider references
 - <csr-id-d28ebe1783c7535ac30cccad1dacc70cb921f371/> update messaging-nats provider references
 - <csr-id-215eeadc555dfc8a57cee96856febf63113f62a1/> update keyvalue-nats provider references
 - <csr-id-e2b6287bde6c9b905d96f93f93a1a7d73940283c/> update http-server provider references
 - <csr-id-e6936ce5113db8434ffb2cc927e779e677ad0bd4/> update http-client provider references
 - <csr-id-b382e8669dd31e44855142d2b478421884710e3d/> update wash dev kv provider to keyvalue-nats 0.3.0
 - <csr-id-f8de84b251122b3319f7b1c8d86cb749cb4a8e37/> update version of keyvalue-nats used to 0.2.0

### Other

 - <csr-id-8d4c48bed132fd33cc7f4d2aaa33b1999e2ea67a/> wash-cli v0.34.1

### New Features

<csr-id-4d299c3f00ab862e0f3deed29b8f29be7099956f/>
<csr-id-ce1569cb6423f3bc42ff645e8c062287d8b3b78f/>
<csr-id-f0cb812a2eb719a3f69c14805ab89cdc273754e4/>

 - <csr-id-095b9f448d3ec37fc55050d08f1d861f127336b9/> enable undeploying and deleting multiple apps
   This commit enables wash to:
   
   - Undeploy all applications at the same time

### Bug Fixes

 - <csr-id-9243e5248d73df5b47dff38723027c36bb6be264/> improve error handling, dependency resolution

### Refactor

 - <csr-id-6cdcf8ae093a0214787ee99132844d5cb92d8b3d/> reorganize `wash up` and `wash config`
   This commit starts the reorganization of `wash-cli` code into the
   `src/cmd` hierarchy, which should make it easier to separate shared
   functionality, (sub)command invocation and specific logic.

### Test

 - <csr-id-71fc4b8f60a1f5f469912b712452f1c96a7744ef/> add test for wash app undeploy --all

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 15 commits contributed to the release over the course of 9 calendar days.
 - 10 days passed between releases.
 - 15 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Wash-cli v0.34.1 ([`8d4c48b`](https://github.com/wasmCloud/wasmCloud/commit/8d4c48bed132fd33cc7f4d2aaa33b1999e2ea67a))
    - Improve error handling, dependency resolution ([`9243e52`](https://github.com/wasmCloud/wasmCloud/commit/9243e5248d73df5b47dff38723027c36bb6be264))
    - Add test for wash app undeploy --all ([`71fc4b8`](https://github.com/wasmCloud/wasmCloud/commit/71fc4b8f60a1f5f469912b712452f1c96a7744ef))
    - Enable undeploying and deleting multiple apps ([`095b9f4`](https://github.com/wasmCloud/wasmCloud/commit/095b9f448d3ec37fc55050d08f1d861f127336b9))
    - Update blobstore-fs provider references ([`ce1d5d1`](https://github.com/wasmCloud/wasmCloud/commit/ce1d5d1ed7c503e2cf357047a4a849f74be7b51e))
    - Update messaging-nats provider references ([`d28ebe1`](https://github.com/wasmCloud/wasmCloud/commit/d28ebe1783c7535ac30cccad1dacc70cb921f371))
    - Update keyvalue-nats provider references ([`215eead`](https://github.com/wasmCloud/wasmCloud/commit/215eeadc555dfc8a57cee96856febf63113f62a1))
    - Update http-server provider references ([`e2b6287`](https://github.com/wasmCloud/wasmCloud/commit/e2b6287bde6c9b905d96f93f93a1a7d73940283c))
    - Update http-client provider references ([`e6936ce`](https://github.com/wasmCloud/wasmCloud/commit/e6936ce5113db8434ffb2cc927e779e677ad0bd4))
    - Enable using custom wasmCloud host path ([`4d299c3`](https://github.com/wasmCloud/wasmCloud/commit/4d299c3f00ab862e0f3deed29b8f29be7099956f))
    - Reorganize `wash up` and `wash config` ([`6cdcf8a`](https://github.com/wasmCloud/wasmCloud/commit/6cdcf8ae093a0214787ee99132844d5cb92d8b3d))
    - Update wash dev kv provider to keyvalue-nats 0.3.0 ([`b382e86`](https://github.com/wasmCloud/wasmCloud/commit/b382e8669dd31e44855142d2b478421884710e3d))
    - Update version of keyvalue-nats used to 0.2.0 ([`f8de84b`](https://github.com/wasmCloud/wasmCloud/commit/f8de84b251122b3319f7b1c8d86cb749cb4a8e37))
    - Implemented Humantime duration input for --watch flags ([`ce1569c`](https://github.com/wasmCloud/wasmCloud/commit/ce1569cb6423f3bc42ff645e8c062287d8b3b78f))
    - Allow wash to re-use registry credentials from Docker ([`f0cb812`](https://github.com/wasmCloud/wasmCloud/commit/f0cb812a2eb719a3f69c14805ab89cdc273754e4))
</details>

## v0.34.0 (2024-09-19)

<csr-id-84633c20117cab57890a62b7b2e734df78133933/>

### Other

 - <csr-id-84633c20117cab57890a62b7b2e734df78133933/> wash-cli: 0.33.1

### Bug Fixes

 - <csr-id-4680de9d2f1e3a2e672833ccac9c2356ef208145/> multiple generated dependencies collision in project
   This commit fixes a bug in the workspace-aware project dependency
   gathering that caused dependencies to override each other under  the
   same project.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 2 commits contributed to the release.
 - 1 day passed between releases.
 - 2 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Wash-cli: 0.33.1 ([`84633c2`](https://github.com/wasmCloud/wasmCloud/commit/84633c20117cab57890a62b7b2e734df78133933))
    - Multiple generated dependencies collision in project ([`4680de9`](https://github.com/wasmCloud/wasmCloud/commit/4680de9d2f1e3a2e672833ccac9c2356ef208145))
</details>

## v0.33.0 (2024-09-18)

<csr-id-e18efc72cc56ae5ce5f929eb17660a0d211c0e06/>
<csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/>
<csr-id-9ac2e29babcaa3e9789c42d05d9d3ad4ccd5fcc7/>
<csr-id-c65d9cab4cc8917eedcad1672812bafad0311ee0/>
<csr-id-2ee92718a7d4dcef9a31cca42761672b2b69c5dd/>
<csr-id-1ff476dcd61675a81d747091a1a94f1a4cd5fedb/>
<csr-id-69039793fe275c35ebf647d52f117c0bbf3bf675/>

### Chore

 - <csr-id-e18efc72cc56ae5ce5f929eb17660a0d211c0e06/> note wash-cli move to wash

### Other

 - <csr-id-1ff476dcd61675a81d747091a1a94f1a4cd5fedb/> tracing v0.8.0, provider-sdk v0.9.0, wash-cli v0.33.0

### Chore

 - <csr-id-69039793fe275c35ebf647d52f117c0bbf3bf675/> Replace dirs dependency with home

### New Features

 - <csr-id-029e2b859ed864707a6780acb3bb08f6b166d288/> enable wash dev for providers
 - <csr-id-e7f85fd9e1afad79d6dc3512390fefb8642f32b0/> add --host-log-path option
   This commit adds a `--host-log-path` option to the wasmcloud-related
   options for `wash up`. With this command, a custom path can be
   specified to write to when the host is producing logs.
 - <csr-id-31b4e24ebb11a3b7622825b9c0f1310b69290998/> add hints for default-configured dependencies
   This commit adds support for printing help text when dependencies are
   configured close to default ways, to help people easier know how to
   get started using the relevant application(s).
 - <csr-id-fc7724c1f0c2046c1a3318477ce58731c3ae2fd6/> write out generated WADM manifest
   This commit adds support for writing out generated WADM manifests live
   as they are generated.
 - <csr-id-b592801565a2e421da653ab5a1b69028522e1efc/> add session management to wash dev
   This commit adds session management and the ability to run `wash dev`
   with more than one host in the background.
 - <csr-id-fd0bbd0a0cb41053d2e90cec8151d0e7c4b0add3/> convert dependencies to WADM manifests
   This commit implements conversion of detected dependencies during
   `wash dev` to WADM manifests that can be run when a component is being
   developed.
 - <csr-id-7738695b405d20261b92c730329387886d1ba04a/> add ability to check and validate OCI image references in WADM manifests

### Bug Fixes

 - <csr-id-5df02de7b1051d4966e3e94e1ec679d6e5faa637/> boolean flag set incorrectly
 - <csr-id-7347fa3f4c6d4dde13a27ad112dd7975e59bd1db/> return put_link errors, better link table
 - <csr-id-9a8ec3d434fecf52181f8853846bf77f4ee46125/> fix hot reloading for wash dev

### Other

 - <csr-id-1af6e05f1a47be4e62a4c21d1704aff2e09bef89/> bump wasmcloud-core v0.10.0, safety bump 5 crates
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-tracing v0.8.0, wasmcloud-provider-sdk v0.9.0, wash-cli v0.33.0, wash-lib v0.26.0
 - <csr-id-9ac2e29babcaa3e9789c42d05d9d3ad4ccd5fcc7/> add links integration test
 - <csr-id-c65d9cab4cc8917eedcad1672812bafad0311ee0/> upgrade to 0.36

### Refactor

 - <csr-id-2ee92718a7d4dcef9a31cca42761672b2b69c5dd/> wash startup process
   This commit reworks how we manage wasmcloud hosts when running `wash
   dev`.
   
   Due to the nature of signal passing on unix/windows, unlike
   `wash up --detached`, when a SIGINT is triggered by a
   terminal (Ctrl-c), the signal is sent to *all* child processes. This
   means that spawned subprocesses for WADM and NATS immediately exit,
   making it it impossible for the host to even properly stop.
   
   To fix this `wash-lib` and `wash-cli` were updated to do the
   following:
   
   - Use command groups to prevent signal passthrough
   - Manage `wadm` and `nats` subprocesses much more similarly to `wash up`
   
   This commit utilizes the updates to `wash-lib` to ensure that `wash
   dev` can properly stop started hosts.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 17 commits contributed to the release over the course of 12 calendar days.
 - 13 days passed between releases.
 - 17 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Tracing v0.8.0, provider-sdk v0.9.0, wash-cli v0.33.0 ([`1ff476d`](https://github.com/wasmCloud/wasmCloud/commit/1ff476dcd61675a81d747091a1a94f1a4cd5fedb))
    - Replace dirs dependency with home ([`6903979`](https://github.com/wasmCloud/wasmCloud/commit/69039793fe275c35ebf647d52f117c0bbf3bf675))
    - Bump wasmcloud-core v0.10.0, safety bump 5 crates ([`1af6e05`](https://github.com/wasmCloud/wasmCloud/commit/1af6e05f1a47be4e62a4c21d1704aff2e09bef89))
    - Enable wash dev for providers ([`029e2b8`](https://github.com/wasmCloud/wasmCloud/commit/029e2b859ed864707a6780acb3bb08f6b166d288))
    - Boolean flag set incorrectly ([`5df02de`](https://github.com/wasmCloud/wasmCloud/commit/5df02de7b1051d4966e3e94e1ec679d6e5faa637))
    - Add --host-log-path option ([`e7f85fd`](https://github.com/wasmCloud/wasmCloud/commit/e7f85fd9e1afad79d6dc3512390fefb8642f32b0))
    - Wash startup process ([`2ee9271`](https://github.com/wasmCloud/wasmCloud/commit/2ee92718a7d4dcef9a31cca42761672b2b69c5dd))
    - Note wash-cli move to wash ([`e18efc7`](https://github.com/wasmCloud/wasmCloud/commit/e18efc72cc56ae5ce5f929eb17660a0d211c0e06))
    - Add links integration test ([`9ac2e29`](https://github.com/wasmCloud/wasmCloud/commit/9ac2e29babcaa3e9789c42d05d9d3ad4ccd5fcc7))
    - Return put_link errors, better link table ([`7347fa3`](https://github.com/wasmCloud/wasmCloud/commit/7347fa3f4c6d4dde13a27ad112dd7975e59bd1db))
    - Add hints for default-configured dependencies ([`31b4e24`](https://github.com/wasmCloud/wasmCloud/commit/31b4e24ebb11a3b7622825b9c0f1310b69290998))
    - Fix hot reloading for wash dev ([`9a8ec3d`](https://github.com/wasmCloud/wasmCloud/commit/9a8ec3d434fecf52181f8853846bf77f4ee46125))
    - Write out generated WADM manifest ([`fc7724c`](https://github.com/wasmCloud/wasmCloud/commit/fc7724c1f0c2046c1a3318477ce58731c3ae2fd6))
    - Add session management to wash dev ([`b592801`](https://github.com/wasmCloud/wasmCloud/commit/b592801565a2e421da653ab5a1b69028522e1efc))
    - Convert dependencies to WADM manifests ([`fd0bbd0`](https://github.com/wasmCloud/wasmCloud/commit/fd0bbd0a0cb41053d2e90cec8151d0e7c4b0add3))
    - Upgrade to 0.36 ([`c65d9ca`](https://github.com/wasmCloud/wasmCloud/commit/c65d9cab4cc8917eedcad1672812bafad0311ee0))
    - Add ability to check and validate OCI image references in WADM manifests ([`7738695`](https://github.com/wasmCloud/wasmCloud/commit/7738695b405d20261b92c730329387886d1ba04a))
</details>

## v0.32.1 (2024-09-05)

<csr-id-2ab6c9a1aa42e79e6ee20d6598a6f97b856af57e/>

### New Features

 - <csr-id-14810d1de464811d76e6a3527b5f159d105803f2/> add basic dependency detection by WIT iface usage
   This commit introduces basic dependency recognition based on WIT
   interface usage in `wash dev`.
 - <csr-id-5dab9201b51f1ca808abc491316772c30686f79b/> implement sorting for outputs of wash get commands
 - <csr-id-cee789f1b4a04076c38b40bf14cc39be46ad08fe/> Add --watch flag to view live changes in host inventory

### Bug Fixes

 - <csr-id-f9b60e8b7e635c2db6fe7c9a7cb916c013670479/> return a string as the body from wash call
   This commit changes `wash call` to return the bytes from a request as
   a string, rather than a `bytes::Bytes`, which was causing output like
   `"body": [123, 123, 123, 123]` rather than the actual stringified bytes.
   
   Since requests that can be satisfied by `wash call` of wasi:http are
   expected to be quite simple, they can be expected to be strings --
   when JSON is expected `--extract-json` can be specified.

### Other

 - <csr-id-2ab6c9a1aa42e79e6ee20d6598a6f97b856af57e/> v0.32.1

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 5 commits contributed to the release over the course of 6 calendar days.
 - 6 days passed between releases.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - V0.32.1 ([`2ab6c9a`](https://github.com/wasmCloud/wasmCloud/commit/2ab6c9a1aa42e79e6ee20d6598a6f97b856af57e))
    - Return a string as the body from wash call ([`f9b60e8`](https://github.com/wasmCloud/wasmCloud/commit/f9b60e8b7e635c2db6fe7c9a7cb916c013670479))
    - Add basic dependency detection by WIT iface usage ([`14810d1`](https://github.com/wasmCloud/wasmCloud/commit/14810d1de464811d76e6a3527b5f159d105803f2))
    - Implement sorting for outputs of wash get commands ([`5dab920`](https://github.com/wasmCloud/wasmCloud/commit/5dab9201b51f1ca808abc491316772c30686f79b))
    - Add --watch flag to view live changes in host inventory ([`cee789f`](https://github.com/wasmCloud/wasmCloud/commit/cee789f1b4a04076c38b40bf14cc39be46ad08fe))
</details>

## v0.32.0 (2024-08-29)

<csr-id-e0d4c09ba7c1176f76a994f32f4c1e3147a3e59b/>
<csr-id-7448729a1927e4ea48738bbf153533dd60ba2ad1/>

### Chore

 - <csr-id-e0d4c09ba7c1176f76a994f32f4c1e3147a3e59b/> help styling to streamline cli markdown

### Other

 - <csr-id-7448729a1927e4ea48738bbf153533dd60ba2ad1/> wash-lib v0.25.0, wash-cli v0.32.0

### New Features

 - <csr-id-66ac0d86d36509fda0db37fffbf8ce32d81c92c5/> give a nicer error to wash app for no responders
 - <csr-id-1ac883f9daad78fb2af6842a15fa5bc2ae69c35f/> add CLI styling to wash

### Bug Fixes

 - <csr-id-fa945c6bcc094afda0babfc2255b38a25a129e1b/> wash dev on non-xkeys component IDs
   This commit fixes an issue wher `wash dev` assumed that component IDs
   had to be `ModuleId`s (i.e. nkeys).
   
   While in the past component IDs *were* nkeys, they are no longer
   required to be, and can be user-friendly names.
 - <csr-id-5efa281da43f2b6f4ae29d5ec8c90822b0bc27f5/> remove misleading creds error message
 - <csr-id-1d7d7c19fc50f1749d9463fcd7cb15f92af3f75e/> add missing feature flag
 - <csr-id-d021a3bcc2def64f1ec002e4248718f5794fb7c2/> fix lints
   This commit fixes lints that seem to *only* show up when trying to
   `cargo publish`

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 8 commits contributed to the release over the course of 2 calendar days.
 - 6 days passed between releases.
 - 8 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Add missing feature flag ([`1d7d7c1`](https://github.com/wasmCloud/wasmCloud/commit/1d7d7c19fc50f1749d9463fcd7cb15f92af3f75e))
    - Fix lints ([`d021a3b`](https://github.com/wasmCloud/wasmCloud/commit/d021a3bcc2def64f1ec002e4248718f5794fb7c2))
    - Wash-lib v0.25.0, wash-cli v0.32.0 ([`7448729`](https://github.com/wasmCloud/wasmCloud/commit/7448729a1927e4ea48738bbf153533dd60ba2ad1))
    - Add CLI styling to wash ([`1ac883f`](https://github.com/wasmCloud/wasmCloud/commit/1ac883f9daad78fb2af6842a15fa5bc2ae69c35f))
    - Wash dev on non-xkeys component IDs ([`fa945c6`](https://github.com/wasmCloud/wasmCloud/commit/fa945c6bcc094afda0babfc2255b38a25a129e1b))
    - Give a nicer error to wash app for no responders ([`66ac0d8`](https://github.com/wasmCloud/wasmCloud/commit/66ac0d86d36509fda0db37fffbf8ce32d81c92c5))
    - Remove misleading creds error message ([`5efa281`](https://github.com/wasmCloud/wasmCloud/commit/5efa281da43f2b6f4ae29d5ec8c90822b0bc27f5))
    - Help styling to streamline cli markdown ([`e0d4c09`](https://github.com/wasmCloud/wasmCloud/commit/e0d4c09ba7c1176f76a994f32f4c1e3147a3e59b))
</details>

## v0.31.0 (2024-08-23)

<csr-id-82927d995dabea1fdd08b14f10dd2b584b7f393b/>
<csr-id-4264e444de95e7af88c04dfa48a2ecd072b93fb3/>
<csr-id-4c41168230f8b78f142f40adf24aaf41c8ae90ca/>
<csr-id-325a1b038cfb239384f2d433acaf2bb8e43fce58/>
<csr-id-13fd60edd0c25f38577524d0a950f039a4beb73a/>
<csr-id-8403350432a2387d4a2bce9c096f002005ba54be/>

### Chore

 - <csr-id-82927d995dabea1fdd08b14f10dd2b584b7f393b/> wasmcloud v1.2.0
 - <csr-id-4264e444de95e7af88c04dfa48a2ecd072b93fb3/> wadm 0.14.0, wasmcloud v1.2.0-rc.1
 - <csr-id-4c41168230f8b78f142f40adf24aaf41c8ae90ca/> update NATS to 2.10.18
 - <csr-id-325a1b038cfb239384f2d433acaf2bb8e43fce58/> consistent casing for FROM/AS in Dockerfiles
 - <csr-id-13fd60edd0c25f38577524d0a950f039a4beb73a/> more explicit error for failing to build provider

### Other

 - <csr-id-8403350432a2387d4a2bce9c096f002005ba54be/> bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates
   SAFETY BUMP: wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wash-cli v0.31.0, wasmcloud-secrets-client v0.4.0

### New Features

<csr-id-cab9a620c34ae8fc3b9173c46921ad1273d03789/>

 - <csr-id-fe2963134f2770b63e613edf289f27ff2a9cb495/> restore changes lost in rebase
 - <csr-id-6f99ed677647a305f7d5ebefc157a377ba60d408/> move clap_markdown logic to avoid double parse
 - <csr-id-556a6cfb70a6be173a9baf7cb82f2765b12e0395/> exit after help-markdown, conflict with help
 - <csr-id-cb7b4d67f2cc5adcd5dba5d4f64d994aec93739e/> exit after help-markdown, conflict with help
 - <csr-id-ee67b1f8281c40ea682c01924510cca5ff7d28bd/> Add clap-markdown to generate CLI docs in Markdown
 - <csr-id-dc921bd69859327e788aa250778bf258a954377a/> Adds --watch flag to wash app list to show live info
 - <csr-id-756168465a34f484adaf37ecdb5f677ce82843bd/> Added secrets-topic and policy-topic flags to wash up
 - <csr-id-4b08262301d6845a71343b4bf0e2928eb956f7d6/> disallow wash drain when host or wadm is running
   - prevents wash drain all and wash drain downloads when wasmcloud.pid or wadm.pid are present on disk

### New Features (BREAKING)

 - <csr-id-301043bb0f86d15e3afb93e410a3a40242c6317a/> display detailed app status
 - <csr-id-6f31a97d9aa3b990a59a5b6813ebc87dd6e1ad3d/> show version of to-download binaries

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 17 commits contributed to the release over the course of 17 calendar days.
 - 21 days passed between releases.
 - 17 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump wasmcloud-core v0.9.0, wash-lib v0.24.0, wasmcloud-tracing v0.7.0, wasmcloud-provider-sdk v0.8.0, wasmcloud-secrets-types v0.4.0, wash-cli v0.31.0, safety bump 5 crates ([`8403350`](https://github.com/wasmCloud/wasmCloud/commit/8403350432a2387d4a2bce9c096f002005ba54be))
    - Restore changes lost in rebase ([`fe29631`](https://github.com/wasmCloud/wasmCloud/commit/fe2963134f2770b63e613edf289f27ff2a9cb495))
    - Move clap_markdown logic to avoid double parse ([`6f99ed6`](https://github.com/wasmCloud/wasmCloud/commit/6f99ed677647a305f7d5ebefc157a377ba60d408))
    - Exit after help-markdown, conflict with help ([`556a6cf`](https://github.com/wasmCloud/wasmCloud/commit/556a6cfb70a6be173a9baf7cb82f2765b12e0395))
    - Exit after help-markdown, conflict with help ([`cb7b4d6`](https://github.com/wasmCloud/wasmCloud/commit/cb7b4d67f2cc5adcd5dba5d4f64d994aec93739e))
    - Add clap-markdown to generate CLI docs in Markdown ([`ee67b1f`](https://github.com/wasmCloud/wasmCloud/commit/ee67b1f8281c40ea682c01924510cca5ff7d28bd))
    - Adds --watch flag to wash app list to show live info ([`dc921bd`](https://github.com/wasmCloud/wasmCloud/commit/dc921bd69859327e788aa250778bf258a954377a))
    - Wasmcloud v1.2.0 ([`82927d9`](https://github.com/wasmCloud/wasmCloud/commit/82927d995dabea1fdd08b14f10dd2b584b7f393b))
    - Display detailed app status ([`301043b`](https://github.com/wasmCloud/wasmCloud/commit/301043bb0f86d15e3afb93e410a3a40242c6317a))
    - Show version of to-download binaries ([`6f31a97`](https://github.com/wasmCloud/wasmCloud/commit/6f31a97d9aa3b990a59a5b6813ebc87dd6e1ad3d))
    - Wadm 0.14.0, wasmcloud v1.2.0-rc.1 ([`4264e44`](https://github.com/wasmCloud/wasmCloud/commit/4264e444de95e7af88c04dfa48a2ecd072b93fb3))
    - Update NATS to 2.10.18 ([`4c41168`](https://github.com/wasmCloud/wasmCloud/commit/4c41168230f8b78f142f40adf24aaf41c8ae90ca))
    - Added secrets-topic and policy-topic flags to wash up ([`7561684`](https://github.com/wasmCloud/wasmCloud/commit/756168465a34f484adaf37ecdb5f677ce82843bd))
    - Disallow wash drain when host or wadm is running ([`4b08262`](https://github.com/wasmCloud/wasmCloud/commit/4b08262301d6845a71343b4bf0e2928eb956f7d6))
    - Limit terminal to width ([`cab9a62`](https://github.com/wasmCloud/wasmCloud/commit/cab9a620c34ae8fc3b9173c46921ad1273d03789))
    - Consistent casing for FROM/AS in Dockerfiles ([`325a1b0`](https://github.com/wasmCloud/wasmCloud/commit/325a1b038cfb239384f2d433acaf2bb8e43fce58))
    - More explicit error for failing to build provider ([`13fd60e`](https://github.com/wasmCloud/wasmCloud/commit/13fd60edd0c25f38577524d0a950f039a4beb73a))
</details>

## v0.30.0 (2024-08-02)

<csr-id-e39430bbdba29d70ee0afbb0f62270189d8e74c7/>
<csr-id-1ec12a7fa8af603a850bb1dbaca03c32d5f36ddd/>
<csr-id-8199616e5e77d32137a319b54c2e7ee83e3c04b7/>
<csr-id-7e26ac135ff6e6f9678f21e44e9631734311c264/>
<csr-id-82cbef77367f1728773268c1ee52b98ac7f31dcf/>
<csr-id-11cd438cc029c79495a4e50c9dbb3acb73be3df6/>
<csr-id-94bfb0e23d4f1f58b70500eaa635717a6ba83484/>
<csr-id-d3c0ca7643beab0fa002c6a4dedf724303bffcaa/>
<csr-id-24e459251eaff69820180c8aaf7663ecc4e76b35/>
<csr-id-a886882ae688dc4955c0d74188388f178f3b13dd/>
<csr-id-af742d076970001af13eaa241927db2ab8f0a9bb/>
<csr-id-6724e2c3d1e5f1de91827a6e542415cef09a278c/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-702514d8db4366c29df8e6e20018af3b22c0446a/>
<csr-id-aa460011c243f363158f80952b386bb33992d3ea/>
<csr-id-c666ef50fecc1ee248bf78d486a915ee077e3b4a/>
<csr-id-2ea22a28ca9fd1838fc03451f33d75690fc28f2a/>
<csr-id-b56982f437209ecaff4fa6946f8fe4c3068a62cd/>
<csr-id-03f0ce7d5269c6230ae49812c7d3e71ab30310f9/>
<csr-id-daed5047dd8b76f248d2d63c18d3f7ff3773f8f6/>
<csr-id-176150022cc86e98f871a6fcd05df725bd9e3419/>
<csr-id-1346aa09cabc0418fba1e929c3f6eac6508ee533/>
<csr-id-1582729d2da6763217ab3106e56f2f2353ae4398/>

### Chore

 - <csr-id-e39430bbdba29d70ee0afbb0f62270189d8e74c7/> Add host_pid_file convenience function for locating wasmcloud host pid
 - <csr-id-1ec12a7fa8af603a850bb1dbaca03c32d5f36ddd/> use http-server v0.22.0
 - <csr-id-8199616e5e77d32137a319b54c2e7ee83e3c04b7/> suppress warning for new rust cfg check
 - <csr-id-7e26ac135ff6e6f9678f21e44e9631734311c264/> update washboard version
 - <csr-id-82cbef77367f1728773268c1ee52b98ac7f31dcf/> remove unused imports
 - <csr-id-11cd438cc029c79495a4e50c9dbb3acb73be3df6/> comment-out HttpResponse until `call` is reenabled
 - <csr-id-94bfb0e23d4f1f58b70500eaa635717a6ba83484/> partially update to NATS 0.35.1
 - <csr-id-d3c0ca7643beab0fa002c6a4dedf724303bffcaa/> disable `call` functionality
 - <csr-id-24e459251eaff69820180c8aaf7663ecc4e76b35/> remove warnings on windows
 - <csr-id-a886882ae688dc4955c0d74188388f178f3b13dd/> Replace actor by component

### Documentation

 - <csr-id-56ef16b2a6d1e5f853e9e6dbe246d37535c2c61e/> Include proxy auth information in wash cli

### New Features

<csr-id-8e4b40218360e4b033f367c413eb6b53c786aca7/>
<csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/>
<csr-id-4eba7f8b738ee83c53040cb22494f5b249cd79af/>

 - <csr-id-832dced17224fc6a8e8cde6cb30cf42ee2c3c2e0/> include simple invocation in wash call tests
 - <csr-id-1870276d4e99987dd7ba6804c1df078fa44e289e/> support new secret reference structure
 - <csr-id-abd804417409bf79e41d4d310af1947efa31eca7/> add secrets subcommand for managing secrets references
   This adds a set of commands for manually managing secrets references in
   a wasmCloud deployment. It is effectively a wrapper around configuration
   that properly formats and writes the data type to the config bucket.
   
   It also attempts to prevent you from interacting with secrets with the
   config subcommands by checking the name of the configuration key and
   redirecting you to the correct subcommand.
 - <csr-id-77e0db9281aa94fbb253f869356942671c90f7fc/> add test for building rust provider in debug mode
 - <csr-id-a570a3565e129fc13b437327eb1ba18835c69f57/> add Host level configurability for max_execution_time by flag and env variables
   - Introduce humantime::Duration for capturing human readable input time.

### Bug Fixes

 - <csr-id-2cee7ba6619a3b861abca87722f462294b78042b/> fix build.rs cfg setting
 - <csr-id-8491de37f699677f1ebea0f702cf660a177df5c2/> transposed args in call.rs
   Noticed these were transposed when the output of the
   `No component responded to your request...` error
   message claimed the lattice was the component and the
   component was the lattice.
 - <csr-id-8329214d91f7b8876b8ced318ca380d262fb44e1/> correct integration test
 - <csr-id-ef93bef6ee8a6822a07301557f8b18541489071b/> secrets reference representation
 - <csr-id-82b6a4bcec6d94db3ad89cec3c1653f12b2fb90e/> add comment about use of multiple async-nats
 - <csr-id-ec9659a915631134064d8e252b6c7d8b6bf322e1/> re-add wash call
   This commit re-adds `wash call` with the existing functionality (no
   improvements) that existed prior to the recent wrpc update.
   
   With this update, invoking simple components and HTTP components both
   work, and tests have been re-enabled.
 - <csr-id-e31d964421bfd2ca5246b9a2eed633c8cc49d7d6/> generate xkey with curve
 - <csr-id-cc7b37df4892025fc5e6f1ca36d57284dd1c3a18/> better format of optional in error
 - <csr-id-439ae1b2d38406647cbfe3a5a8effc0df173b890/> install nextest via normal means
   The "nextest" branch of taiki-e/install-action does nothing but add
   "nextest" to the default list of tools (see:
   https://github.com/taiki-e/install-action/commit/cdf91dadbfbcd09d5ec3fe131167d4c7eb6e350b).
   
   Rather than maintain a completely different pinned hash, we should
   just install nextest normally like any other tool.
 - <csr-id-5df19307560354d3575edd3c6e902c02be6c5aba/> better failure output, optimistic lock file delete
 - <csr-id-94188ea344d0507510f50f0f8d4e72fd2a204500/> enable `std` feature for `clap`
 - <csr-id-8562aef2adac2f8471542ac6866bf7867049cae7/> increase windows stack size
   `clap` has a few bugs that cause stacks to overflow on windows when
   doing commands like walking the full arg tree (i.e. `--help`).
   
   Adopting a fix from one of the
   issues (https://github.com/clap-rs/clap/issues/5134), we increase the
   size of stack during build.
 - <csr-id-81719ba5442b3422880768f9e8fec3f14b4ceede/> remove help about deprecated wash reg #2404
 - <csr-id-759764db606a168104ef085bc64b947730140980/> avoid repeated downloads of wadm binary #2308
 - <csr-id-c211e7c3422893ffabfd0cf2d67768644617ab7e/> remove wash up with manifest cleanup
   This commit removes the cleanup that happens after `wash up
   --wadm-manifest`, because in the single-host situation by the time
   ctrl-c is received by `wash` it is *also* receieved by the child
   process host. When this happens, most of the time the host is *already
   gone* so all you get is "no responders" when trying to issue an
   undeploy.
   
   There are a few ways around this but they all will likely require
   modifying the host/other pieces so first we should probably remove the
   application cleanup code and update the documentation to note that the
   manifest can be undeployed/deleted manually before shutting down.

### Other

 - <csr-id-af742d076970001af13eaa241927db2ab8f0a9bb/> update wasmcloud to 1.1.0
 - <csr-id-6724e2c3d1e5f1de91827a6e542415cef09a278c/> update wadm to 0.13.0
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
 - <csr-id-702514d8db4366c29df8e6e20018af3b22c0446a/> use instances for examples
   Replicas will soon be deprecated.

### Refactor

 - <csr-id-aa460011c243f363158f80952b386bb33992d3ea/> build.rs var naming for host
 - <csr-id-c666ef50fecc1ee248bf78d486a915ee077e3b4a/> include name with secret config
 - <csr-id-2ea22a28ca9fd1838fc03451f33d75690fc28f2a/> move SecretConfig into crate
 - <csr-id-b56982f437209ecaff4fa6946f8fe4c3068a62cd/> address feedback, application name optional
 - <csr-id-03f0ce7d5269c6230ae49812c7d3e71ab30310f9/> pass timeout to wrpc client
 - <csr-id-daed5047dd8b76f248d2d63c18d3f7ff3773f8f6/> remove unused Arc

### Test

 - <csr-id-176150022cc86e98f871a6fcd05df725bd9e3419/> fix version assertion
 - <csr-id-1346aa09cabc0418fba1e929c3f6eac6508ee533/> test config and secrets
 - <csr-id-1582729d2da6763217ab3106e56f2f2353ae4398/> add test to ensure inventory labels are ordered

### New Features (BREAKING)

 - <csr-id-918d4dd341e33d2a88ef5c7453f735d03b6198a4/> support field on SecretRequest
 - <csr-id-f027ce1a393f46fbf663277a11a1d81307af1267/> support generating xkeys

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 49 commits contributed to the release over the course of 45 calendar days.
 - 46 days passed between releases.
 - 49 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Add host_pid_file convenience function for locating wasmcloud host pid ([`e39430b`](https://github.com/wasmCloud/wasmCloud/commit/e39430bbdba29d70ee0afbb0f62270189d8e74c7))
    - Update wasmcloud to 1.1.0 ([`af742d0`](https://github.com/wasmCloud/wasmCloud/commit/af742d076970001af13eaa241927db2ab8f0a9bb))
    - Update wadm to 0.13.0 ([`6724e2c`](https://github.com/wasmCloud/wasmCloud/commit/6724e2c3d1e5f1de91827a6e542415cef09a278c))
    - Include simple invocation in wash call tests ([`832dced`](https://github.com/wasmCloud/wasmCloud/commit/832dced17224fc6a8e8cde6cb30cf42ee2c3c2e0))
    - Bump for test-util release ([`7cd2e71`](https://github.com/wasmCloud/wasmCloud/commit/7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4))
    - Support field on SecretRequest ([`918d4dd`](https://github.com/wasmCloud/wasmCloud/commit/918d4dd341e33d2a88ef5c7453f735d03b6198a4))
    - Fix build.rs cfg setting ([`2cee7ba`](https://github.com/wasmCloud/wasmCloud/commit/2cee7ba6619a3b861abca87722f462294b78042b))
    - Build.rs var naming for host ([`aa46001`](https://github.com/wasmCloud/wasmCloud/commit/aa460011c243f363158f80952b386bb33992d3ea))
    - Use http-server v0.22.0 ([`1ec12a7`](https://github.com/wasmCloud/wasmCloud/commit/1ec12a7fa8af603a850bb1dbaca03c32d5f36ddd))
    - Suppress warning for new rust cfg check ([`8199616`](https://github.com/wasmCloud/wasmCloud/commit/8199616e5e77d32137a319b54c2e7ee83e3c04b7))
    - Transposed args in call.rs ([`8491de3`](https://github.com/wasmCloud/wasmCloud/commit/8491de37f699677f1ebea0f702cf660a177df5c2))
    - Correct integration test ([`8329214`](https://github.com/wasmCloud/wasmCloud/commit/8329214d91f7b8876b8ced318ca380d262fb44e1))
    - Include name with secret config ([`c666ef5`](https://github.com/wasmCloud/wasmCloud/commit/c666ef50fecc1ee248bf78d486a915ee077e3b4a))
    - Move SecretConfig into crate ([`2ea22a2`](https://github.com/wasmCloud/wasmCloud/commit/2ea22a28ca9fd1838fc03451f33d75690fc28f2a))
    - Address feedback, application name optional ([`b56982f`](https://github.com/wasmCloud/wasmCloud/commit/b56982f437209ecaff4fa6946f8fe4c3068a62cd))
    - Secrets reference representation ([`ef93bef`](https://github.com/wasmCloud/wasmCloud/commit/ef93bef6ee8a6822a07301557f8b18541489071b))
    - Pass timeout to wrpc client ([`03f0ce7`](https://github.com/wasmCloud/wasmCloud/commit/03f0ce7d5269c6230ae49812c7d3e71ab30310f9))
    - Remove unused Arc ([`daed504`](https://github.com/wasmCloud/wasmCloud/commit/daed5047dd8b76f248d2d63c18d3f7ff3773f8f6))
    - Add comment about use of multiple async-nats ([`82b6a4b`](https://github.com/wasmCloud/wasmCloud/commit/82b6a4bcec6d94db3ad89cec3c1653f12b2fb90e))
    - Re-add wash call ([`ec9659a`](https://github.com/wasmCloud/wasmCloud/commit/ec9659a915631134064d8e252b6c7d8b6bf322e1))
    - Fix version assertion ([`1761500`](https://github.com/wasmCloud/wasmCloud/commit/176150022cc86e98f871a6fcd05df725bd9e3419))
    - Generate xkey with curve ([`e31d964`](https://github.com/wasmCloud/wasmCloud/commit/e31d964421bfd2ca5246b9a2eed633c8cc49d7d6))
    - Support generating xkeys ([`f027ce1`](https://github.com/wasmCloud/wasmCloud/commit/f027ce1a393f46fbf663277a11a1d81307af1267))
    - Test config and secrets ([`1346aa0`](https://github.com/wasmCloud/wasmCloud/commit/1346aa09cabc0418fba1e929c3f6eac6508ee533))
    - Support new secret reference structure ([`1870276`](https://github.com/wasmCloud/wasmCloud/commit/1870276d4e99987dd7ba6804c1df078fa44e289e))
    - Add secrets subcommand for managing secrets references ([`abd8044`](https://github.com/wasmCloud/wasmCloud/commit/abd804417409bf79e41d4d310af1947efa31eca7))
    - Use instances for examples ([`702514d`](https://github.com/wasmCloud/wasmCloud/commit/702514d8db4366c29df8e6e20018af3b22c0446a))
    - Add test for building rust provider in debug mode ([`77e0db9`](https://github.com/wasmCloud/wasmCloud/commit/77e0db9281aa94fbb253f869356942671c90f7fc))
    - Better format of optional in error ([`cc7b37d`](https://github.com/wasmCloud/wasmCloud/commit/cc7b37df4892025fc5e6f1ca36d57284dd1c3a18))
    - Add Host level configurability for max_execution_time by flag and env variables ([`a570a35`](https://github.com/wasmCloud/wasmCloud/commit/a570a3565e129fc13b437327eb1ba18835c69f57))
    - Improved the output of wash get inventory command, as outlined in issue #2398. Signed-off-by: ossfellow <masoudbahar@gmail.com> ([`8e4b402`](https://github.com/wasmCloud/wasmCloud/commit/8e4b40218360e4b033f367c413eb6b53c786aca7))
    - Update washboard version ([`7e26ac1`](https://github.com/wasmCloud/wasmCloud/commit/7e26ac135ff6e6f9678f21e44e9631734311c264))
    - Install nextest via normal means ([`439ae1b`](https://github.com/wasmCloud/wasmCloud/commit/439ae1b2d38406647cbfe3a5a8effc0df173b890))
    - Remove unused imports ([`82cbef7`](https://github.com/wasmCloud/wasmCloud/commit/82cbef77367f1728773268c1ee52b98ac7f31dcf))
    - Comment-out HttpResponse until `call` is reenabled ([`11cd438`](https://github.com/wasmCloud/wasmCloud/commit/11cd438cc029c79495a4e50c9dbb3acb73be3df6))
    - Better failure output, optimistic lock file delete ([`5df1930`](https://github.com/wasmCloud/wasmCloud/commit/5df19307560354d3575edd3c6e902c02be6c5aba))
    - Partially update to NATS 0.35.1 ([`94bfb0e`](https://github.com/wasmCloud/wasmCloud/commit/94bfb0e23d4f1f58b70500eaa635717a6ba83484))
    - Disable `call` functionality ([`d3c0ca7`](https://github.com/wasmCloud/wasmCloud/commit/d3c0ca7643beab0fa002c6a4dedf724303bffcaa))
    - Enable `std` feature for `clap` ([`94188ea`](https://github.com/wasmCloud/wasmCloud/commit/94188ea344d0507510f50f0f8d4e72fd2a204500))
    - Upgrade `wrpc`, `async-nats`, `wasmtime` ([`9cb1b78`](https://github.com/wasmCloud/wasmCloud/commit/9cb1b784fe7a8892d73bdb40d1172b1879fcd932))
    - Increase windows stack size ([`8562aef`](https://github.com/wasmCloud/wasmCloud/commit/8562aef2adac2f8471542ac6866bf7867049cae7))
    - Adds flag to wash up to allow reading custom NATS config ([`4eba7f8`](https://github.com/wasmCloud/wasmCloud/commit/4eba7f8b738ee83c53040cb22494f5b249cd79af))
    - Remove help about deprecated wash reg #2404 ([`81719ba`](https://github.com/wasmCloud/wasmCloud/commit/81719ba5442b3422880768f9e8fec3f14b4ceede))
    - Avoid repeated downloads of wadm binary #2308 ([`759764d`](https://github.com/wasmCloud/wasmCloud/commit/759764db606a168104ef085bc64b947730140980))
    - Add test to ensure inventory labels are ordered ([`1582729`](https://github.com/wasmCloud/wasmCloud/commit/1582729d2da6763217ab3106e56f2f2353ae4398))
    - Remove wash up with manifest cleanup ([`c211e7c`](https://github.com/wasmCloud/wasmCloud/commit/c211e7c3422893ffabfd0cf2d67768644617ab7e))
    - Remove warnings on windows ([`24e4592`](https://github.com/wasmCloud/wasmCloud/commit/24e459251eaff69820180c8aaf7663ecc4e76b35))
    - Include proxy auth information in wash cli ([`56ef16b`](https://github.com/wasmCloud/wasmCloud/commit/56ef16b2a6d1e5f853e9e6dbe246d37535c2c61e))
    - Replace actor by component ([`a886882`](https://github.com/wasmCloud/wasmCloud/commit/a886882ae688dc4955c0d74188388f178f3b13dd))
</details>

## v0.29.2 (2024-06-17)

<csr-id-4100f5841caa80e23db787380e3e64748016e928/>
<csr-id-8e73b54804e290f24499397f0dd7fc54dbd83a01/>
<csr-id-353e0ca7761757fbd8f6e7b992d6aaa1d1fa15bd/>

### Chore

 - <csr-id-4100f5841caa80e23db787380e3e64748016e928/> update wadm to v0.12.1
 - <csr-id-8e73b54804e290f24499397f0dd7fc54dbd83a01/> update to latest provider references
 - <csr-id-353e0ca7761757fbd8f6e7b992d6aaa1d1fa15bd/> bump to v0.29.2 for wadm-client

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release over the course of 3 calendar days.
 - 3 days passed between releases.
 - 3 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Wash up should inform user that wasmcloud is already running rather than bailing ([`904b499`](https://github.com/wasmCloud/wasmCloud/commit/904b4996544084d22029bbf4142482a1d05a1358))
    - Update wadm to v0.12.1 ([`4100f58`](https://github.com/wasmCloud/wasmCloud/commit/4100f5841caa80e23db787380e3e64748016e928))
    - Update to latest provider references ([`8e73b54`](https://github.com/wasmCloud/wasmCloud/commit/8e73b54804e290f24499397f0dd7fc54dbd83a01))
    - Bump to v0.29.2 for wadm-client ([`353e0ca`](https://github.com/wasmCloud/wasmCloud/commit/353e0ca7761757fbd8f6e7b992d6aaa1d1fa15bd))
</details>

## v0.29.1 (2024-06-13)

<csr-id-105fdb63b67485bf3de2b49ebc20a1d406a769e7/>
<csr-id-ac0f3399de80770fd97c1cd2f622697228ddf2b3/>
<csr-id-3cd6d232ed4359d69973dc6ee5a766115d0823d4/>
<csr-id-6cc63eb91260bc44c79a7e7c4a208f679ac90792/>

### Chore

 - <csr-id-105fdb63b67485bf3de2b49ebc20a1d406a769e7/> bump wasmcloud to v1.0.4
 - <csr-id-ac0f3399de80770fd97c1cd2f622697228ddf2b3/> bump to v0.29.1 for release
 - <csr-id-3cd6d232ed4359d69973dc6ee5a766115d0823d4/> Apply cargo fmt
 - <csr-id-6cc63eb91260bc44c79a7e7c4a208f679ac90792/> Replace actor references by component in wash-lib crate

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release.
 - 4 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump wasmcloud to v1.0.4 ([`105fdb6`](https://github.com/wasmCloud/wasmCloud/commit/105fdb63b67485bf3de2b49ebc20a1d406a769e7))
    - Bump to v0.29.1 for release ([`ac0f339`](https://github.com/wasmCloud/wasmCloud/commit/ac0f3399de80770fd97c1cd2f622697228ddf2b3))
    - Apply cargo fmt ([`3cd6d23`](https://github.com/wasmCloud/wasmCloud/commit/3cd6d232ed4359d69973dc6ee5a766115d0823d4))
    - Replace actor references by component in wash-lib crate ([`6cc63eb`](https://github.com/wasmCloud/wasmCloud/commit/6cc63eb91260bc44c79a7e7c4a208f679ac90792))
</details>

## v0.29.0 (2024-06-12)

<csr-id-7e7cbd74ba6713ebf6b9fab60ef533f46b6a840d/>
<csr-id-7f0ac03e86a010cbeef3dc4410a40ad58e0c5d88/>
<csr-id-c26f2096d9f5ad8dd3aaec6de56395dfab1a5a04/>
<csr-id-d3188d0c9990e39d9f63e0ed154fac7568956af5/>
<csr-id-ed98b8981bea38b43cf05195adf081dd38f275a1/>
<csr-id-7b8800121b7112d3ce44a7f4b939a5d654c35a61/>
<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-df6551c7990795b92c48b2b8a0062b8949410e7d/>
<csr-id-c5b3ad049817edab6d809ffafd51d4ca6fe4db79/>

### Chore

 - <csr-id-7e7cbd74ba6713ebf6b9fab60ef533f46b6a840d/> Add an explicit note about leftover wadm processes
   Sometimes, if a wadm process is leftover after a host exits, it can
   block the usage of wadm in future starts of an existing host. The
   symptom for this is wadm failing during wash startup with a "Couldn't
   download wadm: Text file busy" error.
   
   The quickest fix for this issue is stopping the running wadm processes,
   but it's hard to *know* that might be the problem without intuiting
   what text file might be busy/what might be wrong.
   
   This commit adds a message to the failure to download just in case
   this specific case occurs.
 - <csr-id-7f0ac03e86a010cbeef3dc4410a40ad58e0c5d88/> update washboard version
 - <csr-id-c26f2096d9f5ad8dd3aaec6de56395dfab1a5a04/> bump wasmcloud v1.0.3
 - <csr-id-d3188d0c9990e39d9f63e0ed154fac7568956af5/> removed Name field from get inventory output and washboard output
 - <csr-id-ed98b8981bea38b43cf05195adf081dd38f275a1/> update wadm to v0.12.0
 - <csr-id-7b8800121b7112d3ce44a7f4b939a5d654c35a61/> update nkeys to 0.4
   Update to nkeys 0.4 in preparation for using xkeys in the host.
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-df6551c7990795b92c48b2b8a0062b8949410e7d/> bump wadm to v0.11.2
 - <csr-id-c5b3ad049817edab6d809ffafd51d4ca6fe4db79/> Remove deprecated registry_ping

### Documentation

 - <csr-id-edb7fc7b2bfdb68ef2ecc5effdfcc25ae2ef35dd/> update DEVELOPMENT.md with steps for local testing
   Added detailed steps to the DEVELOPMENT.md file to guide new contributors on how to build and test their changes locally.

### New Features

<csr-id-2aa6086f5ef482cd596e022f8ef1649238ccb4f4/>
<csr-id-358a616f4b0e542228ba143aa8c238adf35ad483/>
<csr-id-6018d3730b3d78e21b064b7c71d5478ed86399b6/>
<csr-id-4b38dddf2295316677cbe75695eb4bffadfe1d18/>
<csr-id-ffbd0310c5e35f9e10b29674b1d0f63473687be7/>

 - <csr-id-fd79e99ef8d8ef14f3e7efae0b3904dade4d7fc3/> display max instances count in washboard output
   feat(wash-cli): display max instances count in wash get inventory output
 - <csr-id-89e0f5ba79fe4c9d1c2485c2f1e28c28885caa4c/> enable custom TLS CA usage
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

 - <csr-id-bf10b48299bb26a51b0c9181230c672e72a2282c/> continue down if pidfile is missing
 - <csr-id-5089dd67da8a9b70201ed8d07cc0d6b22c6d5872/> fix reference to http-jsonify-rust component

### New Features (BREAKING)

 - <csr-id-adbced40c06ec035f3f8b5d0fd062f20d622e0ee/> add --skip-wait option to scale subcommand
   This command changes the default for scale commands, ensuring that
   waiting is the default and a `--skip-wait` option is present.
 - <csr-id-ca98cd65d943b3ba5b39d7ece6983635f5a300e4/> fix #1740, support purging with wash down
 - <csr-id-cd8fc8fcb8c35c18522101736df00804fcb2e56b/> use new wadm-client, update texts
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

 - 29 commits contributed to the release over the course of 27 calendar days.
 - 32 days passed between releases.
 - 26 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Enable undeploying models by file name ([`ffbd031`](https://github.com/wasmCloud/wasmCloud/commit/ffbd0310c5e35f9e10b29674b1d0f63473687be7))
    - Bump wasmcloud-tracing v0.5.0, wasmcloud-provider-sdk v0.6.0, wash-cli v0.29.0 ([`b22d338`](https://github.com/wasmCloud/wasmCloud/commit/b22d338d0d61f8a438c4d6ea5e8e5cd26116ade5))
    - Bump wascap v0.15.0, wasmcloud-core v0.7.0, wash-lib v0.22.0, wasmcloud-tracing v0.5.0, wasmcloud-provider-sdk v0.6.0, wash-cli v0.29.0, safety bump 5 crates ([`2e38cd4`](https://github.com/wasmCloud/wasmCloud/commit/2e38cd45adef18d47af71b87ca456a25edb2f53a))
    - Add --skip-wait option to scale subcommand ([`adbced4`](https://github.com/wasmCloud/wasmCloud/commit/adbced40c06ec035f3f8b5d0fd062f20d622e0ee))
    - Continue down if pidfile is missing ([`bf10b48`](https://github.com/wasmCloud/wasmCloud/commit/bf10b48299bb26a51b0c9181230c672e72a2282c))
    - Fix #1740, support purging with wash down ([`ca98cd6`](https://github.com/wasmCloud/wasmCloud/commit/ca98cd65d943b3ba5b39d7ece6983635f5a300e4))
    - Use new wadm-client, update texts ([`cd8fc8f`](https://github.com/wasmCloud/wasmCloud/commit/cd8fc8fcb8c35c18522101736df00804fcb2e56b))
    - Add an explicit note about leftover wadm processes ([`7e7cbd7`](https://github.com/wasmCloud/wasmCloud/commit/7e7cbd74ba6713ebf6b9fab60ef533f46b6a840d))
    - Update washboard version ([`7f0ac03`](https://github.com/wasmCloud/wasmCloud/commit/7f0ac03e86a010cbeef3dc4410a40ad58e0c5d88))
    - Bump wasmcloud v1.0.3 ([`c26f209`](https://github.com/wasmCloud/wasmCloud/commit/c26f2096d9f5ad8dd3aaec6de56395dfab1a5a04))
    - Removed Name field from get inventory output and washboard output ([`d3188d0`](https://github.com/wasmCloud/wasmCloud/commit/d3188d0c9990e39d9f63e0ed154fac7568956af5))
    - Display max instances count in washboard output ([`fd79e99`](https://github.com/wasmCloud/wasmCloud/commit/fd79e99ef8d8ef14f3e7efae0b3904dade4d7fc3))
    - Enable custom TLS CA usage ([`89e0f5b`](https://github.com/wasmCloud/wasmCloud/commit/89e0f5ba79fe4c9d1c2485c2f1e28c28885caa4c))
    - Update wadm to v0.12.0 ([`ed98b89`](https://github.com/wasmCloud/wasmCloud/commit/ed98b8981bea38b43cf05195adf081dd38f275a1))
    - Removes need for world flag ([`c341171`](https://github.com/wasmCloud/wasmCloud/commit/c341171ccacc6170bf85fe0267facbb94af534ac))
    - Update nkeys to 0.4 ([`7b88001`](https://github.com/wasmCloud/wasmCloud/commit/7b8800121b7112d3ce44a7f4b939a5d654c35a61))
    - Update DEVELOPMENT.md with steps for local testing ([`edb7fc7`](https://github.com/wasmCloud/wasmCloud/commit/edb7fc7b2bfdb68ef2ecc5effdfcc25ae2ef35dd))
    - Allows for pushing binary wit packages with wash ([`d859c74`](https://github.com/wasmCloud/wasmCloud/commit/d859c74dcded69bfbb505663ba2ee1b1429eb465))
    - Add `wash app validate` subcommand ([`10e1d72`](https://github.com/wasmCloud/wasmCloud/commit/10e1d72fd1e899b01e38f842b9a4c7c3048f2657))
    - Add support for `wash up --wadm-manifest` ([`2aa6086`](https://github.com/wasmCloud/wasmCloud/commit/2aa6086f5ef482cd596e022f8ef1649238ccb4f4))
    - Add --replace option to `wash app deploy` ([`358a616`](https://github.com/wasmCloud/wasmCloud/commit/358a616f4b0e542228ba143aa8c238adf35ad483))
    - Replace actor references by component in crates ([`20c72ce`](https://github.com/wasmCloud/wasmCloud/commit/20c72ce0ed423561ae6dbd5a91959bec24ff7cf3))
    - Enable deleting manifests by file name ([`6018d37`](https://github.com/wasmCloud/wasmCloud/commit/6018d3730b3d78e21b064b7c71d5478ed86399b6))
    - Fix reference to http-jsonify-rust component ([`5089dd6`](https://github.com/wasmCloud/wasmCloud/commit/5089dd67da8a9b70201ed8d07cc0d6b22c6d5872))
    - Updates wash to use the new OCI spec for wasm ([`08b5e1e`](https://github.com/wasmCloud/wasmCloud/commit/08b5e1e92c411d2d913537937aec3a8ca5ccb405))
    - Bump wadm to v0.11.2 ([`df6551c`](https://github.com/wasmCloud/wasmCloud/commit/df6551c7990795b92c48b2b8a0062b8949410e7d))
    - Add option to skip certificate validation for the OCI registry connection ([`f9aa387`](https://github.com/wasmCloud/wasmCloud/commit/f9aa3879d273ae9b44f5ee09a724f76df9859d7a))
    - Add support for specifying multiple labels ([`4b38ddd`](https://github.com/wasmCloud/wasmCloud/commit/4b38dddf2295316677cbe75695eb4bffadfe1d18))
    - Remove deprecated registry_ping ([`c5b3ad0`](https://github.com/wasmCloud/wasmCloud/commit/c5b3ad049817edab6d809ffafd51d4ca6fe4db79))
</details>

## v0.28.1 (2024-05-10)

<csr-id-a4a772fb475c1f76215b7fe7aece9c2335bd0c69/>

### Chore

 - <csr-id-a4a772fb475c1f76215b7fe7aece9c2335bd0c69/> bump patch for release

### Bug Fixes

 - <csr-id-1b4faabea11ba6b77b75e34f6892f979be0adde5/> Make wash push returned digest based on the pushed manifest

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 2 commits contributed to the release.
 - 1 day passed between releases.
 - 2 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump patch for release ([`a4a772f`](https://github.com/wasmCloud/wasmCloud/commit/a4a772fb475c1f76215b7fe7aece9c2335bd0c69))
    - Make wash push returned digest based on the pushed manifest ([`1b4faab`](https://github.com/wasmCloud/wasmCloud/commit/1b4faabea11ba6b77b75e34f6892f979be0adde5))
</details>

## v0.28.0 (2024-05-08)

<csr-id-4dc6c775d7780f6811435de0f2cd5401ce21d675/>
<csr-id-d637a8619cac775b6df7f5570c9ba51c948ef36d/>
<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-c074106584ab5330a0ac346b5a51676bd966aa3c/>
<csr-id-09ddd71ba690dba7fa3a6151e98b5bd1396d15a3/>
<csr-id-35ab5d3211ef71dcaf572a49c2003c8ef58a4d6b/>
<csr-id-e49dc41d67b53e418919f538a13c687b0c74a256/>
<csr-id-d703c6fcedd092fcbbb19f7ffd8f79e251fa164d/>
<csr-id-25eeb94fe4cae339ea6a2a1eddb44c90d2cf84ae/>
<csr-id-f2d58a462f909d3b1293c43b43a8cbeca154cf99/>
<csr-id-b2e3158614f3cebf1896c3d5539a69ded97e03fe/>
<csr-id-ac3ec843f22b2946df8e2b52735a13569eaa78d6/>

### Chore

 - <csr-id-4dc6c775d7780f6811435de0f2cd5401ce21d675/> bump wasmcloud v1.0.2
 - <csr-id-d637a8619cac775b6df7f5570c9ba51c948ef36d/> bump wasmCloud and wadm bin versions
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings

### Other

 - <csr-id-ac3ec843f22b2946df8e2b52735a13569eaa78d6/> release and update CHANGELOG

### New Features

 - <csr-id-cbac8fef75bd8dda2554bd1665e75a60059ba4c3/> Adds digest and tag to output of `wash push`
   This follows a similar (but not exact) format from `docker push` and
   includes the digest and tag in JSON output.
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
 - <csr-id-026ecdc473e64c18105fd6f79dc2bad58814e0bf/> Adds support for a scratch space directory
   All plugins get their own directory keyed by ID and can create files
   in that space. Also updates the test to make sure it works
 - <csr-id-0c1dd15e84e9ca86a563168c5e86f32dbd8f2831/> Integrates plugins into the CLI
   This integrates plugins into the CLI and they now function properly. Next
   step is caching component compilation
 - <csr-id-5e81571a5f0dfd08dd8aab4710b731c6f0c685e8/> re-add wash call tests
   This commit re-adds the missing `wash call` tests to the codebase,
   enhancing `wash call` to be able to invoke incoming HTTP handlers
   along the way.

### Bug Fixes

 - <csr-id-305e9b6615d2a2473caccd3dbcbcacbdec02c3ac/> reg test failure
 - <csr-id-2b98f76f6eebdb63f570cae6d95cf3d024b98ca5/> add digest/tag to reg integration test
 - <csr-id-a12b4969876151632efbbe7ccc3f16ebf19f8264/> report filenames when operations fail
   This commit adds missing code for reporting filenames when operations
   fail -- in particular for `wash down`.
 - <csr-id-3810d3ca9a80d347a4aedc6965240e9d007acdd2/> use ID for the final saved binary on plugin install
   Name is meant to be a friendly name for humans and not for file names
 - <csr-id-42d60d20aeb80c7130b5f5f852ce0bc063cfb399/> already updated must succeed to shell
 - <csr-id-150798d33736c49b9793a5ce83e8e0d09142b2ef/> fixed failing integration test for default key_directory
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

### Refactor

 - <csr-id-09ddd71ba690dba7fa3a6151e98b5bd1396d15a3/> ensure file open errors are more informative

### Style

 - <csr-id-35ab5d3211ef71dcaf572a49c2003c8ef58a4d6b/> cargo fmt

### Test

 - <csr-id-e49dc41d67b53e418919f538a13c687b0c74a256/> update cfg attr checks for ghcr.io
 - <csr-id-d703c6fcedd092fcbbb19f7ffd8f79e251fa164d/> check component update with same image reference
 - <csr-id-25eeb94fe4cae339ea6a2a1eddb44c90d2cf84ae/> enable integration_update_actor_serial test
 - <csr-id-f2d58a462f909d3b1293c43b43a8cbeca154cf99/> update key signing test case

### Chore (BREAKING)

 - <csr-id-b2e3158614f3cebf1896c3d5539a69ded97e03fe/> remove interface generation

### New Features (BREAKING)

 - <csr-id-eb82203163249bd7d3252657e04b8d00cd397a14/> make link del interface consistent

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 28 commits contributed to the release over the course of 20 calendar days.
 - 20 days passed between releases.
 - 26 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump provider-archive v0.10.2, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0 ([`73c0ef0`](https://github.com/wasmCloud/wasmCloud/commit/73c0ef0bbe2f6b525655939d2cd30740aef4b6bc))
    - Release and update CHANGELOG ([`ac3ec84`](https://github.com/wasmCloud/wasmCloud/commit/ac3ec843f22b2946df8e2b52735a13569eaa78d6))
    - Bump provider-archive v0.10.1, wasmcloud-core v0.6.0, wash-lib v0.21.0, wasmcloud-tracing v0.4.0, wasmcloud-provider-sdk v0.5.0, wash-cli v0.28.0, safety bump 5 crates ([`75a2e52`](https://github.com/wasmCloud/wasmCloud/commit/75a2e52f52690ba143679c90237851ebd07e153f))
    - Reg test failure ([`305e9b6`](https://github.com/wasmCloud/wasmCloud/commit/305e9b6615d2a2473caccd3dbcbcacbdec02c3ac))
    - Bump wasmcloud v1.0.2 ([`4dc6c77`](https://github.com/wasmCloud/wasmCloud/commit/4dc6c775d7780f6811435de0f2cd5401ce21d675))
    - Add digest/tag to reg integration test ([`2b98f76`](https://github.com/wasmCloud/wasmCloud/commit/2b98f76f6eebdb63f570cae6d95cf3d024b98ca5))
    - Adds digest and tag to output of `wash push` ([`cbac8fe`](https://github.com/wasmCloud/wasmCloud/commit/cbac8fef75bd8dda2554bd1665e75a60059ba4c3))
    - Change plugins to support arbitrary path access ([`c074106`](https://github.com/wasmCloud/wasmCloud/commit/c074106584ab5330a0ac346b5a51676bd966aa3c))
    - Report filenames when operations fail ([`a12b496`](https://github.com/wasmCloud/wasmCloud/commit/a12b4969876151632efbbe7ccc3f16ebf19f8264))
    - Use ID for the final saved binary on plugin install ([`3810d3c`](https://github.com/wasmCloud/wasmCloud/commit/3810d3ca9a80d347a4aedc6965240e9d007acdd2))
    - Bump wasmCloud and wadm bin versions ([`d637a86`](https://github.com/wasmCloud/wasmCloud/commit/d637a8619cac775b6df7f5570c9ba51c948ef36d))
    - Update cfg attr checks for ghcr.io ([`e49dc41`](https://github.com/wasmCloud/wasmCloud/commit/e49dc41d67b53e418919f538a13c687b0c74a256))
    - Ensure file open errors are more informative ([`09ddd71`](https://github.com/wasmCloud/wasmCloud/commit/09ddd71ba690dba7fa3a6151e98b5bd1396d15a3))
    - Adds example for wash plugin ([`d9f1982`](https://github.com/wasmCloud/wasmCloud/commit/d9f1982faeb6ad7365fab39a96019f95e02156e8))
    - Adds `plugin` subcommand ([`6cb20f9`](https://github.com/wasmCloud/wasmCloud/commit/6cb20f900e1ec7dca4b1420c59b3d216014cd93f))
    - Adds support for a scratch space directory ([`026ecdc`](https://github.com/wasmCloud/wasmCloud/commit/026ecdc473e64c18105fd6f79dc2bad58814e0bf))
    - Integrates plugins into the CLI ([`0c1dd15`](https://github.com/wasmCloud/wasmCloud/commit/0c1dd15e84e9ca86a563168c5e86f32dbd8f2831))
    - Already updated must succeed to shell ([`42d60d2`](https://github.com/wasmCloud/wasmCloud/commit/42d60d20aeb80c7130b5f5f852ce0bc063cfb399))
    - Cargo fmt ([`35ab5d3`](https://github.com/wasmCloud/wasmCloud/commit/35ab5d3211ef71dcaf572a49c2003c8ef58a4d6b))
    - Check component update with same image reference ([`d703c6f`](https://github.com/wasmCloud/wasmCloud/commit/d703c6fcedd092fcbbb19f7ffd8f79e251fa164d))
    - Enable integration_update_actor_serial test ([`25eeb94`](https://github.com/wasmCloud/wasmCloud/commit/25eeb94fe4cae339ea6a2a1eddb44c90d2cf84ae))
    - Update key signing test case ([`f2d58a4`](https://github.com/wasmCloud/wasmCloud/commit/f2d58a462f909d3b1293c43b43a8cbeca154cf99))
    - Fixed failing integration test for default key_directory ([`150798d`](https://github.com/wasmCloud/wasmCloud/commit/150798d33736c49b9793a5ce83e8e0d09142b2ef))
    - Remove interface generation ([`b2e3158`](https://github.com/wasmCloud/wasmCloud/commit/b2e3158614f3cebf1896c3d5539a69ded97e03fe))
    - Update wash README ([`8b00bd3`](https://github.com/wasmCloud/wasmCloud/commit/8b00bd35d752e939e3d7725406dc7fdfc1d30d33))
    - Make link del interface consistent ([`eb82203`](https://github.com/wasmCloud/wasmCloud/commit/eb82203163249bd7d3252657e04b8d00cd397a14))
    - Re-add wash call tests ([`5e81571`](https://github.com/wasmCloud/wasmCloud/commit/5e81571a5f0dfd08dd8aab4710b731c6f0c685e8))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
</details>

## v0.27.0 (2024-04-17)

<csr-id-9341d622a3c7e14f764836fb88985a4d537ead02/>
<csr-id-beba0f8153291760c82179dc26bbf557bff32ec4/>
<csr-id-d1ac8442729d9b67e146674375349b22b43ba101/>
<csr-id-fe3fcee1d1897e6942de9cbeedfcbe082275cbdc/>

### Chore

 - <csr-id-9341d622a3c7e14f764836fb88985a4d537ead02/> bump wadm v0.11.0-alpha.4
 - <csr-id-beba0f8153291760c82179dc26bbf557bff32ec4/> bump v0.27.0-alpha.3
 - <csr-id-d1ac8442729d9b67e146674375349b22b43ba101/> bump wadm v0.11.0-alpha.3

### New Features

 - <csr-id-715feda6a0b56fb324c2238e8f7d34a66ac5c5cd/> update to wasmcloud 1.0
 - <csr-id-01f0a5f42a825a437b1706ba6ef608a6d85a760e/> use new default port and bump version
 - <csr-id-329c69bb93b7f286d7ea8642b7a187251412dff8/> change default websocket port to 4223 and enable by default

### Bug Fixes

 - <csr-id-b8ef158b60aac044323630f51b9db900e13ac5ad/> correct emoji spacing
 - <csr-id-dd891c87bdfb9c020ffb644a3c2c81f1d62f36a7/> support configuration for components
 - <csr-id-7c862aaf693182c2c354ab7935e23a5150b44cd3/> left align host ID and labels
 - <csr-id-0dd1c06a1feb46550c3b1d9d0400a845ee34ec4e/> remove ? in deployed output
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

### Reverted

 - <csr-id-abd1d600af4cb5daf8377c06968e5b51a1ebb131/> revert wash call test re-addition
   This reverts commit fe3fcee1d1897e6942de9cbeedfcbe082275cbdc.

### Test

 - <csr-id-fe3fcee1d1897e6942de9cbeedfcbe082275cbdc/> re-enable wash call test
   This commit re-enables the test for `wash call` that used to exist
   prior to wRPC MVP integration.
   
   To do this, we enable `wash call` to work for components that
   implement the `wasi:http/incoming-handler` interface, taking a
   JSON-fiied representation of a request (or a GET by defaul) and
   invoking the relevant component.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 14 commits contributed to the release over the course of 7 calendar days.
 - 8 days passed between releases.
 - 14 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Update to wasmcloud 1.0 ([`715feda`](https://github.com/wasmCloud/wasmCloud/commit/715feda6a0b56fb324c2238e8f7d34a66ac5c5cd))
    - Revert wash call test re-addition ([`abd1d60`](https://github.com/wasmCloud/wasmCloud/commit/abd1d600af4cb5daf8377c06968e5b51a1ebb131))
    - Bump wadm v0.11.0-alpha.4 ([`9341d62`](https://github.com/wasmCloud/wasmCloud/commit/9341d622a3c7e14f764836fb88985a4d537ead02))
    - Re-enable wash call test ([`fe3fcee`](https://github.com/wasmCloud/wasmCloud/commit/fe3fcee1d1897e6942de9cbeedfcbe082275cbdc))
    - Bump v0.27.0-alpha.3 ([`beba0f8`](https://github.com/wasmCloud/wasmCloud/commit/beba0f8153291760c82179dc26bbf557bff32ec4))
    - Bump wadm v0.11.0-alpha.3 ([`d1ac844`](https://github.com/wasmCloud/wasmCloud/commit/d1ac8442729d9b67e146674375349b22b43ba101))
    - Correct emoji spacing ([`b8ef158`](https://github.com/wasmCloud/wasmCloud/commit/b8ef158b60aac044323630f51b9db900e13ac5ad))
    - Support configuration for components ([`dd891c8`](https://github.com/wasmCloud/wasmCloud/commit/dd891c87bdfb9c020ffb644a3c2c81f1d62f36a7))
    - Left align host ID and labels ([`7c862aa`](https://github.com/wasmCloud/wasmCloud/commit/7c862aaf693182c2c354ab7935e23a5150b44cd3))
    - Remove ? in deployed output ([`0dd1c06`](https://github.com/wasmCloud/wasmCloud/commit/0dd1c06a1feb46550c3b1d9d0400a845ee34ec4e))
    - Rename wasmcloud.toml block to component ([`c784967`](https://github.com/wasmCloud/wasmCloud/commit/c78496759ca4703302386b7c8712c303d1f93c0a))
    - Use new default port and bump version ([`01f0a5f`](https://github.com/wasmCloud/wasmCloud/commit/01f0a5f42a825a437b1706ba6ef608a6d85a760e))
    - Change default websocket port to 4223 and enable by default ([`329c69b`](https://github.com/wasmCloud/wasmCloud/commit/329c69bb93b7f286d7ea8642b7a187251412dff8))
    - Registry image URL parsing ([`f758216`](https://github.com/wasmCloud/wasmCloud/commit/f7582160d5bd9d7f967ada2045239bc94653cb9b))
</details>

## v0.27.0-alpha.2 (2024-04-09)

<csr-id-f6e5f0e804d4a7eced93778b739bf58c30ad75e7/>
<csr-id-5995865485b91d449d068464f5f926d762645c7e/>
<csr-id-0e0acd728df340f4f4ae0ea31e47abaecb5b3907/>
<csr-id-fe50175294867bc8c9d109d8d610b0453fd65a1c/>
<csr-id-3a96d288714b14f1d8bab831ef4d0f9533204f56/>
<csr-id-65ff33fe473425fffb320309921dfbdcb7c8f868/>
<csr-id-251c443601dcfd67bfbd9a9e9f9351e3127c5584/>
<csr-id-1bad246d9e174384c1a09bdff7e2dc88d911792e/>
<csr-id-2e93989bf14b223b689f77cb4139275094debae4/>
<csr-id-4460225a145cfae39620498c159b5c106dd6ddaf/>
<csr-id-b6dd820c45f7ea0f62c8cb91adb7074c5e8c0113/>
<csr-id-fb724731281442672975612f24be39955d9535c6/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-9018c03b0bd517c4c2f7fe643c4d510a5823bfb8/>

### Chore

 - <csr-id-f6e5f0e804d4a7eced93778b739bf58c30ad75e7/> bump wash-cli and wash-lib alpha
 - <csr-id-5995865485b91d449d068464f5f926d762645c7e/> bump wasmcloud to 1.0.0-alpha.5
 - <csr-id-0e0acd728df340f4f4ae0ea31e47abaecb5b3907/> pin ctl to workspace
 - <csr-id-fe50175294867bc8c9d109d8d610b0453fd65a1c/> pin to ctl v1.0.0-alpha.2
 - <csr-id-3a96d288714b14f1d8bab831ef4d0f9533204f56/> Updates wash to use new host version
 - <csr-id-65ff33fe473425fffb320309921dfbdcb7c8f868/> address clippy warnings, simplify
 - <csr-id-251c443601dcfd67bfbd9a9e9f9351e3127c5584/> remove redundant allocations
 - <csr-id-1bad246d9e174384c1a09bdff7e2dc88d911792e/> remove unused dependencies

### New Features

 - <csr-id-52a314059176a6d6f257af1a5fc52b8fd1121387/> bump wadm and wasmcloud to 1.0 up
 - <csr-id-df92440e2506c12349bf71e4ea0080228336817e/> add feedback message to 1st run message
 - <csr-id-07b5e70a7f1321d184962d7197a8d98d1ecaaf71/> use native TLS roots along webpki

### Bug Fixes

 - <csr-id-b3ac7a5ee272ab715bbcd49f134cef0138fc58e7/> rename actor to component build path
 - <csr-id-ccbff56712dd96d0661538b489cb9fddff10f4ec/> use config option when getting project config
   This commit fixes the `wash push` command to ensure it uses the
   `--config` switch if provided when looking up project config.
 - <csr-id-66a0908863c481026522e502c83886569127e604/> use config version in error output
 - <csr-id-b2f79dae5ef421b5fe4875379ad7238bca56b8b9/> wash pull/push test failures
   This commit fixes test failures with wash pull & push within `wash-cli`

### Other

 - <csr-id-2e93989bf14b223b689f77cb4139275094debae4/> modified the default key_directory to user's /home/sidconstructs directory and modified test cases

### Refactor

 - <csr-id-4460225a145cfae39620498c159b5c106dd6ddaf/> remove capability claims

### Test

 - <csr-id-b6dd820c45f7ea0f62c8cb91adb7074c5e8c0113/> update start/stop provider events
 - <csr-id-fb724731281442672975612f24be39955d9535c6/> wait for no hosts in dev test

### Chore (BREAKING)

 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-9018c03b0bd517c4c2f7fe643c4d510a5823bfb8/> rename ctl actor to component

### New Features (BREAKING)

 - <csr-id-3f2d2f44470d44809fb83de2fa34b29ad1e6cb30/> Adds version to control API
   This should be the final breaking change of the API and it will require
   a two phased rollout. I'll need to cut new core and host versions first
   and then update wash to use the new host for tests.

### Bug Fixes (BREAKING)

 - <csr-id-93748a1ecd4edd785af257952f1de9497a7ea946/> remove usage of capability signing

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 31 commits contributed to the release over the course of 18 calendar days.
 - 22 days passed between releases.
 - 23 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump wash-cli and wash-lib alpha ([`f6e5f0e`](https://github.com/wasmCloud/wasmCloud/commit/f6e5f0e804d4a7eced93778b739bf58c30ad75e7))
    - Rename actor to component build path ([`b3ac7a5`](https://github.com/wasmCloud/wasmCloud/commit/b3ac7a5ee272ab715bbcd49f134cef0138fc58e7))
    - Bump wadm and wasmcloud to 1.0 up ([`52a3140`](https://github.com/wasmCloud/wasmCloud/commit/52a314059176a6d6f257af1a5fc52b8fd1121387))
    - Use config option when getting project config ([`ccbff56`](https://github.com/wasmCloud/wasmCloud/commit/ccbff56712dd96d0661538b489cb9fddff10f4ec))
    - Remove cluster_seed/cluster_issuers ([`bc5d296`](https://github.com/wasmCloud/wasmCloud/commit/bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f))
    - Use config version in error output ([`66a0908`](https://github.com/wasmCloud/wasmCloud/commit/66a0908863c481026522e502c83886569127e604))
    - Add feedback message to 1st run message ([`df92440`](https://github.com/wasmCloud/wasmCloud/commit/df92440e2506c12349bf71e4ea0080228336817e))
    - Revert "WIP: modified the default key_directory to user's /home/sidconstructs directory and modified test cases" ([`804cadf`](https://github.com/wasmCloud/wasmCloud/commit/804cadf517523f7e38d3946793269885b19bb875))
    - Revert "WIP: removed debug line Signed-off-by: Siddharth Khonde <sidconstructs@gmail.com>" ([`9fc8c23`](https://github.com/wasmCloud/wasmCloud/commit/9fc8c232cd42acd89563859eb9b91b0cbeaf24c4))
    - WIP: removed debug line Signed-off-by: Siddharth Khonde <sidconstructs@gmail.com> ([`04bdd40`](https://github.com/wasmCloud/wasmCloud/commit/04bdd401f3c123e1cfe0aeb790bfff18eca9817c))
    - Modified the default key_directory to user's /home/sidconstructs directory and modified test cases ([`2e93989`](https://github.com/wasmCloud/wasmCloud/commit/2e93989bf14b223b689f77cb4139275094debae4))
    - Update start/stop provider events ([`b6dd820`](https://github.com/wasmCloud/wasmCloud/commit/b6dd820c45f7ea0f62c8cb91adb7074c5e8c0113))
    - Bump wasmcloud to 1.0.0-alpha.5 ([`5995865`](https://github.com/wasmCloud/wasmCloud/commit/5995865485b91d449d068464f5f926d762645c7e))
    - Pin ctl to workspace ([`0e0acd7`](https://github.com/wasmCloud/wasmCloud/commit/0e0acd728df340f4f4ae0ea31e47abaecb5b3907))
    - Rename ctl actor to component ([`9018c03`](https://github.com/wasmCloud/wasmCloud/commit/9018c03b0bd517c4c2f7fe643c4d510a5823bfb8))
    - Wash pull/push test failures ([`b2f79da`](https://github.com/wasmCloud/wasmCloud/commit/b2f79dae5ef421b5fe4875379ad7238bca56b8b9))
    - Pin to ctl v1.0.0-alpha.2 ([`fe50175`](https://github.com/wasmCloud/wasmCloud/commit/fe50175294867bc8c9d109d8d610b0453fd65a1c))
    - Wait for no hosts in dev test ([`fb72473`](https://github.com/wasmCloud/wasmCloud/commit/fb724731281442672975612f24be39955d9535c6))
    - Cleanup and fix tests. ([`c2ceee0`](https://github.com/wasmCloud/wasmCloud/commit/c2ceee0a5ed26526b3e3b026ec3762fefe049da5))
    - Consolidate wash stop host and wash down functions. ([`4b1e420`](https://github.com/wasmCloud/wasmCloud/commit/4b1e420f866961365bf20aff3d63a7fb6cb911e3))
    - Check multilocal option first. ([`ddadf2c`](https://github.com/wasmCloud/wasmCloud/commit/ddadf2cbb97bf3aa1880a0ee9d8724cce0caf6f2))
    - Use pid to determine if host is running. ([`13198bb`](https://github.com/wasmCloud/wasmCloud/commit/13198bb9625f32363fdfb6a541ae10b649ea3e57))
    - Wash up should be idempotent. ([`b857655`](https://github.com/wasmCloud/wasmCloud/commit/b8576558988c60a928200d68b857364177e9d6a4))
    - Remove capability claims ([`4460225`](https://github.com/wasmCloud/wasmCloud/commit/4460225a145cfae39620498c159b5c106dd6ddaf))
    - Remove usage of capability signing ([`93748a1`](https://github.com/wasmCloud/wasmCloud/commit/93748a1ecd4edd785af257952f1de9497a7ea946))
    - Updates wash to use new host version ([`3a96d28`](https://github.com/wasmCloud/wasmCloud/commit/3a96d288714b14f1d8bab831ef4d0f9533204f56))
    - Adds version to control API ([`3f2d2f4`](https://github.com/wasmCloud/wasmCloud/commit/3f2d2f44470d44809fb83de2fa34b29ad1e6cb30))
    - Use native TLS roots along webpki ([`07b5e70`](https://github.com/wasmCloud/wasmCloud/commit/07b5e70a7f1321d184962d7197a8d98d1ecaaf71))
    - Address clippy warnings, simplify ([`65ff33f`](https://github.com/wasmCloud/wasmCloud/commit/65ff33fe473425fffb320309921dfbdcb7c8f868))
    - Remove redundant allocations ([`251c443`](https://github.com/wasmCloud/wasmCloud/commit/251c443601dcfd67bfbd9a9e9f9351e3127c5584))
    - Remove unused dependencies ([`1bad246`](https://github.com/wasmCloud/wasmCloud/commit/1bad246d9e174384c1a09bdff7e2dc88d911792e))
</details>

## v0.27.0-alpha.1 (2024-03-18)

<csr-id-e114b77f20463be5b028ee0d373a199fafc0893c/>
<csr-id-873e1482c1aa0fb8f532c8ec3dfbb912bf227546/>
<csr-id-888400046df8a1a636f42c9fb498d6d42331bcf2/>
<csr-id-37fbe7f3bf41ce6d290f0b28ecb7d75b7595f961/>

### Chore

 - <csr-id-e114b77f20463be5b028ee0d373a199fafc0893c/> update wasmcloud to v1.0.0-alpha.2
 - <csr-id-873e1482c1aa0fb8f532c8ec3dfbb912bf227546/> bump to 0.27-alpha.1
 - <csr-id-888400046df8a1a636f42c9fb498d6d42331bcf2/> rename actor to component

### Documentation

 - <csr-id-05ac449d3da207fd495ecbd786220b053fd6300e/> actor to components terminology
   This change only updates documentation terminology
   to use components instead of actors.
   
   Examples will use the terminology components as well so
   I'm opting to rename the example directories now ahead
   of any source code changes for actor to component
   renames.

### New Features

 - <csr-id-1a8d80b28a36c75424a071a4d785acf05516bc62/> validate user input component ids
 - <csr-id-614af7e3ed734c56b27cd1d2aacb0789a85e8b81/> implement Redis `wrpc:keyvalue/{atomic,eventual}`
 - <csr-id-4cd2b2d7de5b0899a2e274aaf3b3c7279bc204f9/> basic wasi:cli/run style wrpc invocation

### Bug Fixes

 - <csr-id-fd85e254ee56abb65bee648ba0ea93b9a227a96f/> fix deadlock and slow ack of update
 - <csr-id-4ee7a5612cac8fb0ba92177995d67d750c083ede/> disable wash call integration test
 - <csr-id-ec84fadfd819f203fe2e4906f5338f48f6ddec78/> update wrpc_client
 - <csr-id-dc2c93df97bb119bb2a024d5bd3458394f421792/> correct comment on wrpc Client

### Test

 - <csr-id-37fbe7f3bf41ce6d290f0b28ecb7d75b7595f961/> update tests to validate new apis

### New Features (BREAKING)

 - <csr-id-18de48d9664324916ee9aaa75478f1990d1bce25/> implement config subcommand
 - <csr-id-8cbfeef8dea590b15446ec29b66e7008e0e717f1/> update CLI and lib to to be 1.0 compatible
 - <csr-id-fa91e865348b99506bafb8987757d7ee516b1edf/> update wash-cli to 1.0 ctliface
 - <csr-id-25d8f5bc4d43fb3a05c871bf367a7ac14b247f79/> implement wash building provider for host machine
 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 18 commits contributed to the release over the course of 30 calendar days.
 - 32 days passed between releases.
 - 17 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Update wasmcloud to v1.0.0-alpha.2 ([`e114b77`](https://github.com/wasmCloud/wasmCloud/commit/e114b77f20463be5b028ee0d373a199fafc0893c))
    - Bump to 0.27-alpha.1 ([`873e148`](https://github.com/wasmCloud/wasmCloud/commit/873e1482c1aa0fb8f532c8ec3dfbb912bf227546))
    - Implement config subcommand ([`18de48d`](https://github.com/wasmCloud/wasmCloud/commit/18de48d9664324916ee9aaa75478f1990d1bce25))
    - Validate user input component ids ([`1a8d80b`](https://github.com/wasmCloud/wasmCloud/commit/1a8d80b28a36c75424a071a4d785acf05516bc62))
    - Update tests to validate new apis ([`37fbe7f`](https://github.com/wasmCloud/wasmCloud/commit/37fbe7f3bf41ce6d290f0b28ecb7d75b7595f961))
    - Fix deadlock and slow ack of update ([`fd85e25`](https://github.com/wasmCloud/wasmCloud/commit/fd85e254ee56abb65bee648ba0ea93b9a227a96f))
    - Update CLI and lib to to be 1.0 compatible ([`8cbfeef`](https://github.com/wasmCloud/wasmCloud/commit/8cbfeef8dea590b15446ec29b66e7008e0e717f1))
    - Rename actor to component ([`8884000`](https://github.com/wasmCloud/wasmCloud/commit/888400046df8a1a636f42c9fb498d6d42331bcf2))
    - Feat(wash-cli)\!: remove deprecated commands, update help ([`e6ee3de`](https://github.com/wasmCloud/wasmCloud/commit/e6ee3de780ec5bc6a6cc0ec3a15457278978bca6))
    - Update wash-cli to 1.0 ctliface ([`fa91e86`](https://github.com/wasmCloud/wasmCloud/commit/fa91e865348b99506bafb8987757d7ee516b1edf))
    - Actor to components terminology ([`05ac449`](https://github.com/wasmCloud/wasmCloud/commit/05ac449d3da207fd495ecbd786220b053fd6300e))
    - Implement Redis `wrpc:keyvalue/{atomic,eventual}` ([`614af7e`](https://github.com/wasmCloud/wasmCloud/commit/614af7e3ed734c56b27cd1d2aacb0789a85e8b81))
    - Disable wash call integration test ([`4ee7a56`](https://github.com/wasmCloud/wasmCloud/commit/4ee7a5612cac8fb0ba92177995d67d750c083ede))
    - Update wrpc_client ([`ec84fad`](https://github.com/wasmCloud/wasmCloud/commit/ec84fadfd819f203fe2e4906f5338f48f6ddec78))
    - Correct comment on wrpc Client ([`dc2c93d`](https://github.com/wasmCloud/wasmCloud/commit/dc2c93df97bb119bb2a024d5bd3458394f421792))
    - Basic wasi:cli/run style wrpc invocation ([`4cd2b2d`](https://github.com/wasmCloud/wasmCloud/commit/4cd2b2d7de5b0899a2e274aaf3b3c7279bc204f9))
    - Implement wash building provider for host machine ([`25d8f5b`](https://github.com/wasmCloud/wasmCloud/commit/25d8f5bc4d43fb3a05c871bf367a7ac14b247f79))
    - Updates topics to the new standard ([`42d069e`](https://github.com/wasmCloud/wasmCloud/commit/42d069eee87d1b5befff1a95b49973064f1a1d1b))
</details>

## v0.26.0 (2024-02-14)

<csr-id-28f204ab08f471b62639d36b22bd7864f85a9450/>
<csr-id-e14e498e207f7b97784b50a5dee8aebe8d3584f0/>
<csr-id-8e8f6d29518ec6d986fad9426fbe8224171660ab/>
<csr-id-7b85266232cccee176fd747ba4c7c96c3a336567/>
<csr-id-529136a3a5983238eb45b07d9c3a8e0198cbf163/>
<csr-id-1793dc9296b7e161a8efe42bd7e5717bd6687da8/>
<csr-id-7f700611a60da3848afa9007bc0d2a1b4fcab946/>
<csr-id-00570832d78d32757323bfa44527154b0ff5fe3e/>
<csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/>

### Chore

 - <csr-id-28f204ab08f471b62639d36b22bd7864f85a9450/> fix `wash-cli` clippy warning
 - <csr-id-e14e498e207f7b97784b50a5dee8aebe8d3584f0/> update nats image to 2.10 to automatically pull in patch bumps
 - <csr-id-8e8f6d29518ec6d986fad9426fbe8224171660ab/> remove ineffective ENV aliases
   This commit removes what were supposed to be ENV aliases that don't
   work, which were introduced by https://github.com/wasmCloud/wasmCloud/pull/1243
 - <csr-id-7b85266232cccee176fd747ba4c7c96c3a336567/> Remove deprecated --count argument from wash ctl stop actor calls
 - <csr-id-529136a3a5983238eb45b07d9c3a8e0198cbf163/> help text comment fixes
 - <csr-id-1793dc9296b7e161a8efe42bd7e5717bd6687da8/> replace env_logger with tracing_subscriber
 - <csr-id-7f700611a60da3848afa9007bc0d2a1b4fcab946/> bump NATS server version

### New Features

 - <csr-id-8cdd687d20a04ccbd3f812cc6748004fa2089778/> update favorites to use components
 - <csr-id-7c4a2be53a68c42af9cb36807f3acc1bd965e8f5/> Better scale message
 - <csr-id-cb29e7582a6faa40c203ccdf165b7d1fc667451f/> add support for wash app status
 - <csr-id-5dac7aff84e57eaf5d2f6cf5f0e3bc7848e284d6/> support other build languages
 - <csr-id-f624025842146fee3d1e024d1db660ee968f305f/> detect arch when building PAR files
   This commit adds the ability to detect the arch + OS combination when
   building PAR files, and use that as a default valiue. It's unlikely
   that people will create PARs from *not* the native toolchain, and in
   those cases they can specify `--arch` as normal.
 - <csr-id-1ad43c4dfddf411107c0d63358a9c8779339bb99/> add label command to set and remove host labels
 - <csr-id-c7233dba737f4c86ae94d040e1a1c3bd18af5ce6/> remove experimental flag from

### Other

 - <csr-id-00570832d78d32757323bfa44527154b0ff5fe3e/> v0.26.0

### New Features (BREAKING)

 - <csr-id-63f01857a9d9f324c4fa619147224163b340f9e2/> update wasmcloud 0.82, wadm 0.10
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
 - <csr-id-dc0785bf1a45558a0deecebd51bf3e39bff4ee3b/> enable websocket port by default

### Refactor (BREAKING)

 - <csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/> rename lattice prefix to just lattice

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 22 commits contributed to the release over the course of 42 calendar days.
 - 47 days passed between releases.
 - 22 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Update wasmcloud 0.82, wadm 0.10 ([`63f0185`](https://github.com/wasmCloud/wasmCloud/commit/63f01857a9d9f324c4fa619147224163b340f9e2))
    - Update favorites to use components ([`8cdd687`](https://github.com/wasmCloud/wasmCloud/commit/8cdd687d20a04ccbd3f812cc6748004fa2089778))
    - Fix `wash-cli` clippy warning ([`28f204a`](https://github.com/wasmCloud/wasmCloud/commit/28f204ab08f471b62639d36b22bd7864f85a9450))
    - V0.26.0 ([`0057083`](https://github.com/wasmCloud/wasmCloud/commit/00570832d78d32757323bfa44527154b0ff5fe3e))
    - Better scale message ([`7c4a2be`](https://github.com/wasmCloud/wasmCloud/commit/7c4a2be53a68c42af9cb36807f3acc1bd965e8f5))
    - Update nats image to 2.10 to automatically pull in patch bumps ([`e14e498`](https://github.com/wasmCloud/wasmCloud/commit/e14e498e207f7b97784b50a5dee8aebe8d3584f0))
    - Remove ineffective ENV aliases ([`8e8f6d2`](https://github.com/wasmCloud/wasmCloud/commit/8e8f6d29518ec6d986fad9426fbe8224171660ab))
    - Add support for wash app status ([`cb29e75`](https://github.com/wasmCloud/wasmCloud/commit/cb29e7582a6faa40c203ccdf165b7d1fc667451f))
    - Remove deprecated --count argument from wash ctl stop actor calls ([`7b85266`](https://github.com/wasmCloud/wasmCloud/commit/7b85266232cccee176fd747ba4c7c96c3a336567))
    - Allow relative paths in file-based WADM manifests ([`8863f14`](https://github.com/wasmCloud/wasmCloud/commit/8863f14f00dcde3c6a299551e7dfbca7867843dc))
    - Rename lattice prefix to just lattice ([`6e8faab`](https://github.com/wasmCloud/wasmCloud/commit/6e8faab6a6e9f9bb7327ffb71ded2a83718920f7))
    - Support other build languages ([`5dac7af`](https://github.com/wasmCloud/wasmCloud/commit/5dac7aff84e57eaf5d2f6cf5f0e3bc7848e284d6))
    - Detect arch when building PAR files ([`f624025`](https://github.com/wasmCloud/wasmCloud/commit/f624025842146fee3d1e024d1db660ee968f305f))
    - Remove singular actor events, add actor_scaled ([`df01bbd`](https://github.com/wasmCloud/wasmCloud/commit/df01bbd89fd2b690c2d1bcfe68455fb827646a10))
    - Upgrade max_instances to u32 ([`5cca9ee`](https://github.com/wasmCloud/wasmCloud/commit/5cca9ee0a88d63cb53e8d352c16a5d9d59966bc8))
    - Rename max-concurrent to max-instances, simplify scale ([`d8eb9f3`](https://github.com/wasmCloud/wasmCloud/commit/d8eb9f3ee9df65e96d076a6ba11d2600d0513207))
    - Help text comment fixes ([`529136a`](https://github.com/wasmCloud/wasmCloud/commit/529136a3a5983238eb45b07d9c3a8e0198cbf163))
    - Enable websocket port by default ([`dc0785b`](https://github.com/wasmCloud/wasmCloud/commit/dc0785bf1a45558a0deecebd51bf3e39bff4ee3b))
    - Add label command to set and remove host labels ([`1ad43c4`](https://github.com/wasmCloud/wasmCloud/commit/1ad43c4dfddf411107c0d63358a9c8779339bb99))
    - Replace env_logger with tracing_subscriber ([`1793dc9`](https://github.com/wasmCloud/wasmCloud/commit/1793dc9296b7e161a8efe42bd7e5717bd6687da8))
    - Remove experimental flag from ([`c7233db`](https://github.com/wasmCloud/wasmCloud/commit/c7233dba737f4c86ae94d040e1a1c3bd18af5ce6))
    - Bump NATS server version ([`7f70061`](https://github.com/wasmCloud/wasmCloud/commit/7f700611a60da3848afa9007bc0d2a1b4fcab946))
</details>

## v0.25.0 (2023-12-28)

<csr-id-c12eff1597e444fcd926dbfb0abab547b2efc2b0/>
<csr-id-8b751e4e9bce78281f6bf6979bfb70c3f6b33634/>
<csr-id-b0fdf60a33d6866a92924b02e5e2ae8544e421a5/>
<csr-id-b7e54e7bbccd1fbcb4f1a9f77cb1a0289f8a239b/>
<csr-id-046fd4c735c8c0ebb2f5a64ae4b5a762b0034591/>
<csr-id-25af017f69652a98b8969609e2854636e2bc7553/>
<csr-id-7bc207bf24873e5d916edf7e8a4b56c7ed04b9a7/>
<csr-id-7de31820034c4b70ab6edc772713e64aafe294a9/>
<csr-id-65d2e28d54929b8f4d0b39077ee82ddad2387c8e/>
<csr-id-57d014fb7fe11542d2e64068ba86e42a19f64f98/>
<csr-id-4e9bae34fe95ecaffbc81fd452bf29746b4e5856/>

### Chore

 - <csr-id-c12eff1597e444fcd926dbfb0abab547b2efc2b0/> update wasmcloud version to 0.81
 - <csr-id-8b751e4e9bce78281f6bf6979bfb70c3f6b33634/> remove references to PROV_RPC settings
 - <csr-id-b0fdf60a33d6866a92924b02e5e2ae8544e421a5/> pin wasmcloud version to 0.81-rc1
 - <csr-id-b7e54e7bbccd1fbcb4f1a9f77cb1a0289f8a239b/> bump wash-cli to 0.25
 - <csr-id-046fd4c735c8c0ebb2f5a64ae4b5a762b0034591/> convert httpserver to provider-wit-bindgen
   The httpserver capability provider enables actors to respond to HTTP
   requests in a given lattice. Up until now, the httpserver provider was
   defined using Smithy contracts and the older `weld` based ecosystem.
   
   Moving forward to enable WIT-ification of the wasmcloud ecosystem,
   in-tree providers are being converted to binaries powered by WIT
   primarily, rather than Smithy contracts.
   
   This commit converts the in-tree `warp`-based httpserver capability provider to use
   `provider-wit-bindgen`, including changes to `provider-wit-bindgen` to
   support the increased complexity that is presented by the `httpserver`
   capability provider.
 - <csr-id-25af017f69652a98b8969609e2854636e2bc7553/> replace broken URLs
 - <csr-id-7bc207bf24873e5d916edf7e8a4b56c7ed04b9a7/> refactor command parsing for readability

### New Features

 - <csr-id-715e94e7f1a35da002769a0a25d531606f003d49/> consistently prefix cli flags
 - <csr-id-d91e92b7bd32a23804cafc4381e7648a151ace38/> prefix absolute path references with file://
 - <csr-id-bae6a00390e2ac10eaede2966d060477b7091697/> enable only signing actors

### Bug Fixes

 - <csr-id-37618a316baf573cc31311ad3ae78cd054e0e2b5/> update format for serialized claims

### Refactor

 - <csr-id-7de31820034c4b70ab6edc772713e64aafe294a9/> remove deprecated code related to start actor cmd
 - <csr-id-65d2e28d54929b8f4d0b39077ee82ddad2387c8e/> update parsing from RegistryCredential to RegistryAuth
 - <csr-id-57d014fb7fe11542d2e64068ba86e42a19f64f98/> revised implementation of registry url and credentials resolution
 - <csr-id-4e9bae34fe95ecaffbc81fd452bf29746b4e5856/> some cleanup before revised implementation

### New Features (BREAKING)

 - <csr-id-b0e6c1f167c9c2e06750d72f10dc729d17f0b81a/> force minimum wasmCloud version to 0.81
 - <csr-id-a86415712621504b820b8c4d0b71017b7140470b/> add support for inspecting wit
 - <csr-id-023307fcb351a67fe2271862ace8657ac0e101b6/> add support for custom build command

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 18 commits contributed to the release over the course of 30 calendar days.
 - 36 days passed between releases.
 - 18 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Update wasmcloud version to 0.81 ([`c12eff1`](https://github.com/wasmCloud/wasmCloud/commit/c12eff1597e444fcd926dbfb0abab547b2efc2b0))
    - Consistently prefix cli flags ([`715e94e`](https://github.com/wasmCloud/wasmCloud/commit/715e94e7f1a35da002769a0a25d531606f003d49))
    - Prefix absolute path references with file:// ([`d91e92b`](https://github.com/wasmCloud/wasmCloud/commit/d91e92b7bd32a23804cafc4381e7648a151ace38))
    - Remove references to PROV_RPC settings ([`8b751e4`](https://github.com/wasmCloud/wasmCloud/commit/8b751e4e9bce78281f6bf6979bfb70c3f6b33634))
    - Force minimum wasmCloud version to 0.81 ([`b0e6c1f`](https://github.com/wasmCloud/wasmCloud/commit/b0e6c1f167c9c2e06750d72f10dc729d17f0b81a))
    - Pin wasmcloud version to 0.81-rc1 ([`b0fdf60`](https://github.com/wasmCloud/wasmCloud/commit/b0fdf60a33d6866a92924b02e5e2ae8544e421a5))
    - Bump wash-cli to 0.25 ([`b7e54e7`](https://github.com/wasmCloud/wasmCloud/commit/b7e54e7bbccd1fbcb4f1a9f77cb1a0289f8a239b))
    - Convert httpserver to provider-wit-bindgen ([`046fd4c`](https://github.com/wasmCloud/wasmCloud/commit/046fd4c735c8c0ebb2f5a64ae4b5a762b0034591))
    - Add support for inspecting wit ([`a864157`](https://github.com/wasmCloud/wasmCloud/commit/a86415712621504b820b8c4d0b71017b7140470b))
    - Remove deprecated code related to start actor cmd ([`7de3182`](https://github.com/wasmCloud/wasmCloud/commit/7de31820034c4b70ab6edc772713e64aafe294a9))
    - Update parsing from RegistryCredential to RegistryAuth ([`65d2e28`](https://github.com/wasmCloud/wasmCloud/commit/65d2e28d54929b8f4d0b39077ee82ddad2387c8e))
    - Revised implementation of registry url and credentials resolution ([`57d014f`](https://github.com/wasmCloud/wasmCloud/commit/57d014fb7fe11542d2e64068ba86e42a19f64f98))
    - Some cleanup before revised implementation ([`4e9bae3`](https://github.com/wasmCloud/wasmCloud/commit/4e9bae34fe95ecaffbc81fd452bf29746b4e5856))
    - Replace broken URLs ([`25af017`](https://github.com/wasmCloud/wasmCloud/commit/25af017f69652a98b8969609e2854636e2bc7553))
    - Update format for serialized claims ([`37618a3`](https://github.com/wasmCloud/wasmCloud/commit/37618a316baf573cc31311ad3ae78cd054e0e2b5))
    - Refactor command parsing for readability ([`7bc207b`](https://github.com/wasmCloud/wasmCloud/commit/7bc207bf24873e5d916edf7e8a4b56c7ed04b9a7))
    - Add support for custom build command ([`023307f`](https://github.com/wasmCloud/wasmCloud/commit/023307fcb351a67fe2271862ace8657ac0e101b6))
    - Enable only signing actors ([`bae6a00`](https://github.com/wasmCloud/wasmCloud/commit/bae6a00390e2ac10eaede2966d060477b7091697))
</details>

## v0.24.1 (2023-11-22)

<csr-id-19f34054fddb6991a51ee8ab953cf36ef4c79399/>

### Other

 - <csr-id-19f34054fddb6991a51ee8ab953cf36ef4c79399/> bump to 0.24.1

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump to 0.24.1 ([`19f3405`](https://github.com/wasmCloud/wasmCloud/commit/19f34054fddb6991a51ee8ab953cf36ef4c79399))
</details>

## v0.24.0 (2023-11-21)

<csr-id-a972375413491a180dec6c7a3948eee597850340/>
<csr-id-bfb51a2dc47d09af1aec0ec4cb23654f93903f25/>
<csr-id-9f0fefeeaba9edc016b151e94c4dc0b57a44882e/>
<csr-id-85193dd0a6f1892cd04c231b40b206720089fa3e/>
<csr-id-dc003f8dd193648988927d312958c6c79c980aaf/>
<csr-id-267d24dcdc871bbc85c0adc0d102a632310bb9f0/>

### Chore

 - <csr-id-a972375413491a180dec6c7a3948eee597850340/> update brew install command
 - <csr-id-bfb51a2dc47d09af1aec0ec4cb23654f93903f25/> update docker dep versions

### Documentation

 - <csr-id-20ffecb027c225fb62d60b584d6b518aff4ceb51/> update wash URLs
 - <csr-id-3d37a8615f2c40c4fbb089b9e8d9263e9e163c16/> update installation instructions for wash

### Other

 - <csr-id-9f0fefeeaba9edc016b151e94c4dc0b57a44882e/> bump wash to 0.24.0

### Refactor

 - <csr-id-85193dd0a6f1892cd04c231b40b206720089fa3e/> move more wash invocations into TestWashInstance
   `TestWashInstance` is a test utility struct that encapsulates (and tracks) child
   processes spawned by `wash` so that they can be cleaned up upon `drop()`,
   and information about spawned hosts can be retrieved.
   
   Some invocations of `wash` itself (normally from tests that ensure
   functionality works have been moved into `TestWashInstance` to make
   them easier to call -- with the *current* built version of
   `wash` (i.e. the cargo-provided ENV variable `CARGO_BIN_EXE_wash`).
   
   This commit adds more invocations (`wash start provider`, `wash stop
   actor`, `wash stop host`) into the `TestWashInstance` struct used from
   tests, shortening the code required for individual tests.

### Test

 - <csr-id-dc003f8dd193648988927d312958c6c79c980aaf/> add a test for wash up labels
   This commit adds a test to ensure specifying labels via wash up works
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

 - 9 commits contributed to the release over the course of 6 calendar days.
 - 7 days passed between releases.
 - 9 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Update brew install command ([`a972375`](https://github.com/wasmCloud/wasmCloud/commit/a972375413491a180dec6c7a3948eee597850340))
    - Move more wash invocations into TestWashInstance ([`85193dd`](https://github.com/wasmCloud/wasmCloud/commit/85193dd0a6f1892cd04c231b40b206720089fa3e))
    - Bump wash to 0.24.0 ([`9f0fefe`](https://github.com/wasmCloud/wasmCloud/commit/9f0fefeeaba9edc016b151e94c4dc0b57a44882e))
    - Removes need for actor/provider/host IDs in almost all cases ([`ce7904e`](https://github.com/wasmCloud/wasmCloud/commit/ce7904e6f4cc49ca92ec8dee8e263d23da26afd0))
    - Update docker dep versions ([`bfb51a2`](https://github.com/wasmCloud/wasmCloud/commit/bfb51a2dc47d09af1aec0ec4cb23654f93903f25))
    - Add a test for wash up labels ([`dc003f8`](https://github.com/wasmCloud/wasmCloud/commit/dc003f8dd193648988927d312958c6c79c980aaf))
    - Add integration test for wash-call ([`267d24d`](https://github.com/wasmCloud/wasmCloud/commit/267d24dcdc871bbc85c0adc0d102a632310bb9f0))
    - Update wash URLs ([`20ffecb`](https://github.com/wasmCloud/wasmCloud/commit/20ffecb027c225fb62d60b584d6b518aff4ceb51))
    - Update installation instructions for wash ([`3d37a86`](https://github.com/wasmCloud/wasmCloud/commit/3d37a8615f2c40c4fbb089b9e8d9263e9e163c16))
</details>

## v0.23.0 (2023-11-14)

<csr-id-5301084bde0db0c65811aa30c48de2a63e091fcf/>
<csr-id-39a9e218418a0662de4edabbc9078268ba095842/>
<csr-id-bb4fbeaa780552fa90e310773f53b16e83569438/>
<csr-id-d734e98529a5fe1c7f014b5b0c5aaf4c84af912a/>
<csr-id-db99594fb6537d8f84a421edf153d9ca6bdbbeed/>
<csr-id-694bf86d100c98d9b1c771972e96a15d70fef116/>
<csr-id-cbc9ed7008f8969312534e326cf119dbbdf89aaa/>
<csr-id-248e9d3ac60fdd2b380723e9bbaf1cc8023beb44/>
<csr-id-cb4d311c6d666e59c22199f950757abc65167f53/>
<csr-id-7d6155e62512e6909379bbed5e73abe219838e4b/>
<csr-id-9bf9accbcefa3e852c3b62290c14ee5e71731530/>

### Chore

 - <csr-id-5301084bde0db0c65811aa30c48de2a63e091fcf/> remove support for bindle references
 - <csr-id-39a9e218418a0662de4edabbc9078268ba095842/> use with_context for lazy eval
 - <csr-id-bb4fbeaa780552fa90e310773f53b16e83569438/> remove `wasmcloud-test-util` dependency
 - <csr-id-d734e98529a5fe1c7f014b5b0c5aaf4c84af912a/> add context to encoding errors
 - <csr-id-db99594fb6537d8f84a421edf153d9ca6bdbbeed/> remove `wasmbus_rpc` dependency

### Documentation

 - <csr-id-572c4cd62bb4645da90ffd69f92e9422a632e628/> add doc comment for label option
 - <csr-id-4ef9921e2283e7fc43ea427b90f36fb874b0d32a/> format rustup
 - <csr-id-3d373ed3da71736ac82015a222c54c275733f6aa/> add instructions for setting up language toolchains
 - <csr-id-f6814b9c82fe0a7d71aaccf5f379e5362622f9bf/> update help text for keys gen

### New Features

 - <csr-id-6098e2488729a0fd50a71623699d9ee257da43d9/> add --wadm-js-domain option
 - <csr-id-196569848412e5680a2d286d449f20776f7de26e/> add --label option to wash up
 - <csr-id-b82aadccb7b2a21fd704667c1f9d1767479ddbc0/> respect wash context for wash up

### Bug Fixes

 - <csr-id-c7b2a1dd9f96542982fd8e4f188eca374d51db7d/> allow specifying --nats-remote-url without --nats-credsfile
 - <csr-id-3b4da1d734e3217dc63f09971a4046d4818cabb3/> use --nats-js-domain for NATS server
 - <csr-id-61da61726c5a9a791a96d9a42014822d4872fd57/> use valid host and public keys for wash call
 - <csr-id-d9e08049aaefa0c6c1f3d112c5423ac205b448b0/> continue passing PROV_RPC variables until the host removes support
 - <csr-id-70ac131767572f757fca6c37cdc428f40212bc6f/> proper derivation of lattice_prefix (ie, lattice_prefix arg > context arg > $current_default context.lattice_prefix)

### Other

 - <csr-id-694bf86d100c98d9b1c771972e96a15d70fef116/> v0.23.0

### Refactor

 - <csr-id-cbc9ed7008f8969312534e326cf119dbbdf89aaa/> always have a context
 - <csr-id-248e9d3ac60fdd2b380723e9bbaf1cc8023beb44/> rename new_with_dir to from_dir
 - <csr-id-cb4d311c6d666e59c22199f950757abc65167f53/> use create_nats_client_from_opts from wash-lib
 - <csr-id-7d6155e62512e6909379bbed5e73abe219838e4b/> more refactoring...
 - <csr-id-9bf9accbcefa3e852c3b62290c14ee5e71731530/> moving things around, better scopring for lattice_prefix parsing on app cmds

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 24 commits contributed to the release over the course of 5 calendar days.
 - 10 days passed between releases.
 - 23 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - V0.23.0 ([`694bf86`](https://github.com/wasmCloud/wasmCloud/commit/694bf86d100c98d9b1c771972e96a15d70fef116))
    - Allow specifying --nats-remote-url without --nats-credsfile ([`c7b2a1d`](https://github.com/wasmCloud/wasmCloud/commit/c7b2a1dd9f96542982fd8e4f188eca374d51db7d))
    - Use --nats-js-domain for NATS server ([`3b4da1d`](https://github.com/wasmCloud/wasmCloud/commit/3b4da1d734e3217dc63f09971a4046d4818cabb3))
    - Add --wadm-js-domain option ([`6098e24`](https://github.com/wasmCloud/wasmCloud/commit/6098e2488729a0fd50a71623699d9ee257da43d9))
    - Remove support for bindle references ([`5301084`](https://github.com/wasmCloud/wasmCloud/commit/5301084bde0db0c65811aa30c48de2a63e091fcf))
    - Add doc comment for label option ([`572c4cd`](https://github.com/wasmCloud/wasmCloud/commit/572c4cd62bb4645da90ffd69f92e9422a632e628))
    - Add --label option to wash up ([`1965698`](https://github.com/wasmCloud/wasmCloud/commit/196569848412e5680a2d286d449f20776f7de26e))
    - Use valid host and public keys for wash call ([`61da617`](https://github.com/wasmCloud/wasmCloud/commit/61da61726c5a9a791a96d9a42014822d4872fd57))
    - Always have a context ([`cbc9ed7`](https://github.com/wasmCloud/wasmCloud/commit/cbc9ed7008f8969312534e326cf119dbbdf89aaa))
    - Rename new_with_dir to from_dir ([`248e9d3`](https://github.com/wasmCloud/wasmCloud/commit/248e9d3ac60fdd2b380723e9bbaf1cc8023beb44))
    - Use with_context for lazy eval ([`39a9e21`](https://github.com/wasmCloud/wasmCloud/commit/39a9e218418a0662de4edabbc9078268ba095842))
    - Use create_nats_client_from_opts from wash-lib ([`cb4d311`](https://github.com/wasmCloud/wasmCloud/commit/cb4d311c6d666e59c22199f950757abc65167f53))
    - Continue passing PROV_RPC variables until the host removes support ([`d9e0804`](https://github.com/wasmCloud/wasmCloud/commit/d9e08049aaefa0c6c1f3d112c5423ac205b448b0))
    - Respect wash context for wash up ([`b82aadc`](https://github.com/wasmCloud/wasmCloud/commit/b82aadccb7b2a21fd704667c1f9d1767479ddbc0))
    - Refactor!(wash-cli): initialize contexts consistently ([`703283b`](https://github.com/wasmCloud/wasmCloud/commit/703283b144a97a7e41ef67cae242ae73d85067a9))
    - Remove `wasmcloud-test-util` dependency ([`bb4fbea`](https://github.com/wasmCloud/wasmCloud/commit/bb4fbeaa780552fa90e310773f53b16e83569438))
    - Add context to encoding errors ([`d734e98`](https://github.com/wasmCloud/wasmCloud/commit/d734e98529a5fe1c7f014b5b0c5aaf4c84af912a))
    - Remove `wasmbus_rpc` dependency ([`db99594`](https://github.com/wasmCloud/wasmCloud/commit/db99594fb6537d8f84a421edf153d9ca6bdbbeed))
    - Format rustup ([`4ef9921`](https://github.com/wasmCloud/wasmCloud/commit/4ef9921e2283e7fc43ea427b90f36fb874b0d32a))
    - Add instructions for setting up language toolchains ([`3d373ed`](https://github.com/wasmCloud/wasmCloud/commit/3d373ed3da71736ac82015a222c54c275733f6aa))
    - Update help text for keys gen ([`f6814b9`](https://github.com/wasmCloud/wasmCloud/commit/f6814b9c82fe0a7d71aaccf5f379e5362622f9bf))
    - More refactoring... ([`7d6155e`](https://github.com/wasmCloud/wasmCloud/commit/7d6155e62512e6909379bbed5e73abe219838e4b))
    - Moving things around, better scopring for lattice_prefix parsing on app cmds ([`9bf9acc`](https://github.com/wasmCloud/wasmCloud/commit/9bf9accbcefa3e852c3b62290c14ee5e71731530))
    - Proper derivation of lattice_prefix (ie, lattice_prefix arg > context arg > $current_default context.lattice_prefix) ([`70ac131`](https://github.com/wasmCloud/wasmCloud/commit/70ac131767572f757fca6c37cdc428f40212bc6f))
</details>

## v0.22.0 (2023-11-04)

<csr-id-3ebfdd25b43c09a8117158d1d1aaaf0e5431746e/>
<csr-id-b936abf2812b191ece6a01a65a090081c69d2013/>
<csr-id-a1c3b9d86db14f31ef7fbebeb30e8784f974df6f/>
<csr-id-007660e96ad7472918bc25baf9d52d60e5230823/>
<csr-id-dfad0be609868cbd0f0ce97d7d9238b41996b5fc/>
<csr-id-a8e085e4eb46a635c9efd02a864584079b0eca4e/>
<csr-id-e28c1ac58a8cd6a1ec745f0de18d0776ec4e064e/>
<csr-id-3f05d242dde36ce33e3ee87ba5b3c62c37c30d63/>
<csr-id-18ed1810f8b8e0517b02ec7139a6c55742296d87/>
<csr-id-82e8bc2e8c2cd6ddcd88232c503241c024dc1ec1/>
<csr-id-c5845c0aed2d12174986f6cfa875f89704cb04d7/>
<csr-id-6343ebfdf155cbfb3b70b1f2cbdcf38651946010/>
<csr-id-413e395b60d3ee0c187ec398a2cb6429fd27d009/>
<csr-id-3d47e91e7a836ff04fd7bc809a036fadc42c01a7/>
<csr-id-abc075095e5df96e0b3c155bf1afb8dbeea4a6e5/>
<csr-id-62f30c7bd3e591bb08d1583498aaba8b0483859d/>
<csr-id-d1ee13ed7c1668b55f4644b1c1673f521ba9d9f8/>
<csr-id-dadfacb6541eec6e6a440bad99ffa66ea17a94a5/>
<csr-id-a1e8d3f09e039723d28d738d98b47bce54e4450d/>

### Chore

 - <csr-id-3ebfdd25b43c09a8117158d1d1aaaf0e5431746e/> fix import order
 - <csr-id-b936abf2812b191ece6a01a65a090081c69d2013/> move washboard to its own directory
 - <csr-id-a1c3b9d86db14f31ef7fbebeb30e8784f974df6f/> support domain, links, keys alias
 - <csr-id-007660e96ad7472918bc25baf9d52d60e5230823/> update control interface 0.31
 - <csr-id-dfad0be609868cbd0f0ce97d7d9238b41996b5fc/> integrate `wash` into the workspace

### New Features

 - <csr-id-041525dcca71bb437963adb4f6944066c1a26f76/> allow specifying washboard version
 - <csr-id-11eaf81137d476769312bf70589d2734f923887d/> download washboard assets from releases instead of embedding from source
 - <csr-id-4004c41fb42a0bfe62b1521bcfa3ceaadd2a9caf/> stricter args parsing for wash keys gen cmd
 - <csr-id-9ffcc1b7494ced74e4a12094bd9b4ef782b6a83f/> add status indicator

### Bug Fixes

 - <csr-id-4fb8118f8fd74a4baf8019f3ab6c6cea2fd1c889/> require revision and version args on sign cmd
 - <csr-id-544fa7e4c117512e613de15626e05853f1244f6b/> resubscribing when lattice connection change
   related to https://github.com/wasmCloud/wash/issues/741
   related to https://github.com/wasmCloud/wash/pull/742

### Other

 - <csr-id-a8e085e4eb46a635c9efd02a864584079b0eca4e/> wash-cli-v0.22.0
 - <csr-id-e28c1ac58a8cd6a1ec745f0de18d0776ec4e064e/> Bump lucide-react in /crates/wash-cli/washboard
   Bumps [lucide-react](https://github.com/lucide-icons/lucide/tree/HEAD/packages/lucide-react) from 0.289.0 to 0.290.0.
   - [Release notes](https://github.com/lucide-icons/lucide/releases)
   - [Commits](https://github.com/lucide-icons/lucide/commits/0.290.0/packages/lucide-react)
   
   ---
   updated-dependencies:
   - dependency-name: lucide-react
     dependency-type: direct:production
     update-type: version-update:semver-minor
   ...
 - <csr-id-3f05d242dde36ce33e3ee87ba5b3c62c37c30d63/> Bump @vitejs/plugin-react-swc
   Bumps [@vitejs/plugin-react-swc](https://github.com/vitejs/vite-plugin-react-swc) from 3.4.0 to 3.4.1.
   - [Release notes](https://github.com/vitejs/vite-plugin-react-swc/releases)
   - [Changelog](https://github.com/vitejs/vite-plugin-react-swc/blob/main/CHANGELOG.md)
   - [Commits](https://github.com/vitejs/vite-plugin-react-swc/compare/v3.4.0...v3.4.1)
   
   ---
   updated-dependencies:
   - dependency-name: "@vitejs/plugin-react-swc"
     dependency-type: direct:development
     update-type: version-update:semver-patch
   ...
 - <csr-id-18ed1810f8b8e0517b02ec7139a6c55742296d87/> Bump tailwind-merge in /crates/wash-cli/washboard
   Bumps [tailwind-merge](https://github.com/dcastil/tailwind-merge) from 1.14.0 to 2.0.0.
   - [Release notes](https://github.com/dcastil/tailwind-merge/releases)
   - [Commits](https://github.com/dcastil/tailwind-merge/compare/v1.14.0...v2.0.0)
   
   ---
   updated-dependencies:
   - dependency-name: tailwind-merge
     dependency-type: direct:production
     update-type: version-update:semver-major
   ...
 - <csr-id-82e8bc2e8c2cd6ddcd88232c503241c024dc1ec1/> Bump eslint-plugin-unicorn
   Bumps [eslint-plugin-unicorn](https://github.com/sindresorhus/eslint-plugin-unicorn) from 48.0.1 to 49.0.0.
   - [Release notes](https://github.com/sindresorhus/eslint-plugin-unicorn/releases)
   - [Commits](https://github.com/sindresorhus/eslint-plugin-unicorn/compare/v48.0.1...v49.0.0)
   
   ---
   updated-dependencies:
   - dependency-name: eslint-plugin-unicorn
     dependency-type: direct:development
     update-type: version-update:semver-major
   ...
 - <csr-id-c5845c0aed2d12174986f6cfa875f89704cb04d7/> Bump eslint-plugin-react-refresh
   Bumps [eslint-plugin-react-refresh](https://github.com/ArnaudBarre/eslint-plugin-react-refresh) from 0.4.3 to 0.4.4.
   - [Release notes](https://github.com/ArnaudBarre/eslint-plugin-react-refresh/releases)
   - [Changelog](https://github.com/ArnaudBarre/eslint-plugin-react-refresh/blob/main/CHANGELOG.md)
   - [Commits](https://github.com/ArnaudBarre/eslint-plugin-react-refresh/compare/v0.4.3...v0.4.4)
   
   ---
   updated-dependencies:
   - dependency-name: eslint-plugin-react-refresh
     dependency-type: direct:development
     update-type: version-update:semver-patch
   ...
 - <csr-id-6343ebfdf155cbfb3b70b1f2cbdcf38651946010/> move nextest config to root
 - <csr-id-413e395b60d3ee0c187ec398a2cb6429fd27d009/> revert to upstream `wash` dev doc
 - <csr-id-3d47e91e7a836ff04fd7bc809a036fadc42c01a7/> move completion doc to `wash-cli` crate
 - <csr-id-abc075095e5df96e0b3c155bf1afb8dbeea4a6e5/> build for Windows msvc
   Unfortunately, `wash` cannot be built for mingw due to
   https://github.com/rust-lang/rust/issues/92212

### Refactor

 - <csr-id-62f30c7bd3e591bb08d1583498aaba8b0483859d/> cleaner pattern matching on keytype arg for wash keys gen cmd.
 - <csr-id-d1ee13ed7c1668b55f4644b1c1673f521ba9d9f8/> reorder target-specific dep

### Test

 - <csr-id-dadfacb6541eec6e6a440bad99ffa66ea17a94a5/> remove vestigial integration tests assertions for wash claims

### Chore (BREAKING)

 - <csr-id-a1e8d3f09e039723d28d738d98b47bce54e4450d/> update ctl to 0.31.0

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 20 commits contributed to the release over the course of 4 calendar days.
 - 4 days passed between releases.
 - 19 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Fix import order ([`3ebfdd2`](https://github.com/wasmCloud/wasmCloud/commit/3ebfdd25b43c09a8117158d1d1aaaf0e5431746e))
    - Allow specifying washboard version ([`041525d`](https://github.com/wasmCloud/wasmCloud/commit/041525dcca71bb437963adb4f6944066c1a26f76))
    - Download washboard assets from releases instead of embedding from source ([`11eaf81`](https://github.com/wasmCloud/wasmCloud/commit/11eaf81137d476769312bf70589d2734f923887d))
    - Remove vestigial integration tests assertions for wash claims ([`dadfacb`](https://github.com/wasmCloud/wasmCloud/commit/dadfacb6541eec6e6a440bad99ffa66ea17a94a5))
    - Require revision and version args on sign cmd ([`4fb8118`](https://github.com/wasmCloud/wasmCloud/commit/4fb8118f8fd74a4baf8019f3ab6c6cea2fd1c889))
    - Wash-cli-v0.22.0 ([`a8e085e`](https://github.com/wasmCloud/wasmCloud/commit/a8e085e4eb46a635c9efd02a864584079b0eca4e))
    - Move washboard to its own directory ([`b936abf`](https://github.com/wasmCloud/wasmCloud/commit/b936abf2812b191ece6a01a65a090081c69d2013))
    - Cleaner pattern matching on keytype arg for wash keys gen cmd. ([`62f30c7`](https://github.com/wasmCloud/wasmCloud/commit/62f30c7bd3e591bb08d1583498aaba8b0483859d))
    - Stricter args parsing for wash keys gen cmd ([`4004c41`](https://github.com/wasmCloud/wasmCloud/commit/4004c41fb42a0bfe62b1521bcfa3ceaadd2a9caf))
    - Support domain, links, keys alias ([`a1c3b9d`](https://github.com/wasmCloud/wasmCloud/commit/a1c3b9d86db14f31ef7fbebeb30e8784f974df6f))
    - Update control interface 0.31 ([`007660e`](https://github.com/wasmCloud/wasmCloud/commit/007660e96ad7472918bc25baf9d52d60e5230823))
    - Update ctl to 0.31.0 ([`a1e8d3f`](https://github.com/wasmCloud/wasmCloud/commit/a1e8d3f09e039723d28d738d98b47bce54e4450d))
    - Add status indicator ([`9ffcc1b`](https://github.com/wasmCloud/wasmCloud/commit/9ffcc1b7494ced74e4a12094bd9b4ef782b6a83f))
    - Resubscribing when lattice connection change ([`544fa7e`](https://github.com/wasmCloud/wasmCloud/commit/544fa7e4c117512e613de15626e05853f1244f6b))
    - Bump lucide-react in /crates/wash-cli/washboard ([`e28c1ac`](https://github.com/wasmCloud/wasmCloud/commit/e28c1ac58a8cd6a1ec745f0de18d0776ec4e064e))
    - Bump @vitejs/plugin-react-swc ([`3f05d24`](https://github.com/wasmCloud/wasmCloud/commit/3f05d242dde36ce33e3ee87ba5b3c62c37c30d63))
    - Bump tailwind-merge in /crates/wash-cli/washboard ([`18ed181`](https://github.com/wasmCloud/wasmCloud/commit/18ed1810f8b8e0517b02ec7139a6c55742296d87))
    - Bump eslint-plugin-unicorn ([`82e8bc2`](https://github.com/wasmCloud/wasmCloud/commit/82e8bc2e8c2cd6ddcd88232c503241c024dc1ec1))
    - Bump eslint-plugin-react-refresh ([`c5845c0`](https://github.com/wasmCloud/wasmCloud/commit/c5845c0aed2d12174986f6cfa875f89704cb04d7))
    - Merge pull request #807 from rvolosatovs/merge/wash ([`f2bc010`](https://github.com/wasmCloud/wasmCloud/commit/f2bc010110d96fc21bc3502798543b7d5b68b1b5))
</details>

## v0.0.0-rc1 (2023-10-30)

<csr-id-dfad0be609868cbd0f0ce97d7d9238b41996b5fc/>
<csr-id-6343ebfdf155cbfb3b70b1f2cbdcf38651946010/>
<csr-id-413e395b60d3ee0c187ec398a2cb6429fd27d009/>
<csr-id-3d47e91e7a836ff04fd7bc809a036fadc42c01a7/>
<csr-id-abc075095e5df96e0b3c155bf1afb8dbeea4a6e5/>
<csr-id-d1ee13ed7c1668b55f4644b1c1673f521ba9d9f8/>

### Chore

 - <csr-id-dfad0be609868cbd0f0ce97d7d9238b41996b5fc/> integrate `wash` into the workspace

### Other

 - <csr-id-6343ebfdf155cbfb3b70b1f2cbdcf38651946010/> move nextest config to root
 - <csr-id-413e395b60d3ee0c187ec398a2cb6429fd27d009/> revert to upstream `wash` dev doc
 - <csr-id-3d47e91e7a836ff04fd7bc809a036fadc42c01a7/> move completion doc to `wash-cli` crate
 - <csr-id-abc075095e5df96e0b3c155bf1afb8dbeea4a6e5/> build for Windows msvc
   Unfortunately, `wash` cannot be built for mingw due to
   https://github.com/rust-lang/rust/issues/92212

### Refactor

 - <csr-id-d1ee13ed7c1668b55f4644b1c1673f521ba9d9f8/> reorder target-specific dep

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 6 commits contributed to the release.
 - 6 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Move nextest config to root ([`6343ebf`](https://github.com/wasmCloud/wasmCloud/commit/6343ebfdf155cbfb3b70b1f2cbdcf38651946010))
    - Revert to upstream `wash` dev doc ([`413e395`](https://github.com/wasmCloud/wasmCloud/commit/413e395b60d3ee0c187ec398a2cb6429fd27d009))
    - Move completion doc to `wash-cli` crate ([`3d47e91`](https://github.com/wasmCloud/wasmCloud/commit/3d47e91e7a836ff04fd7bc809a036fadc42c01a7))
    - Reorder target-specific dep ([`d1ee13e`](https://github.com/wasmCloud/wasmCloud/commit/d1ee13ed7c1668b55f4644b1c1673f521ba9d9f8))
    - Build for Windows msvc ([`abc0750`](https://github.com/wasmCloud/wasmCloud/commit/abc075095e5df96e0b3c155bf1afb8dbeea4a6e5))
    - Integrate `wash` into the workspace ([`dfad0be`](https://github.com/wasmCloud/wasmCloud/commit/dfad0be609868cbd0f0ce97d7d9238b41996b5fc))
</details>


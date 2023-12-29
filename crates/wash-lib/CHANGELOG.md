# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

## v0.16.0 (2023-12-28)

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
    - Update wasmcloud version to 0.81 (c12eff1)
    - Fix typo in test file; fix assert statements (9476b91)
    - Fix unit test failling due to wrong expected value (e1c00a3)
    - Project config overrides for claims commands (3e744b5)
    - Claims signing shouldn't require a wasmcloud.toml file. (c7270fd)
    - Simplify nkey directory path derivation logic (189fdf8)
    - Make wash claims aware of wasmcloud.toml (4450972)
    - Prefix absolute path references with file:// (d91e92b)
    - Only embed metadata in tinygo modules (edc1fa5)
    - Force minimum wasmCloud version to 0.81 (b0e6c1f)
    - Pin wasmcloud version to 0.81-rc1 (b0fdf60)
    - Bump wash-lib to 0.16 (fc10788)
    - Fix generating from git branch (5f3850f)
    - Update adapters (087b5c3)
    - Enable docs feature when building for docs.rs (a63d565)
    - Update golang example to wasmtime 16 (cfc002b)
    - Add support for inspecting wit (a864157)
    - Remove object file from expected test (7fac3db)
    - Revert `wash` adapter update (ff2e832)
    - Update to wasmtime 16 (75c0739)
    - Remove unused import (98b7a55)
    - Remove vestigial actor refresh function call in dev setup (e58d357)
    - Remove deprecated code related to start actor cmd (7de3182)
    - Add support for model.status wadm command in wash-lib (57eec5c)
    - Revised implementation of registry url and credentials resolution (57d014f)
    - Some cleanup before revised implementation (4e9bae3)
    - Replace broken URLs (25af017)
    - Refactor command parsing for readability (7bc207b)
    - Add support for custom build command (023307f)
    - Enable only signing actors (bae6a00)
    - Do not enable new component encoding (547ed47)
</details>

## v0.15.0 (2023-11-21)

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
    - V0.15.0 (000299c)
    - Support RISCV64 (91dfdfe)
    - Removes need for actor/provider/host IDs in almost all cases (ce7904e)
    - Add integration test for wash-call (267d24d)
    - Update wash URLs (20ffecb)
    - Update `async-nats` to 0.33 (4adbf06)
</details>

## v0.14.0 (2023-11-14)

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
    - V0.14.0 (d43d300)
    - Allow specifying --nats-remote-url without --nats-credsfile (c7b2a1d)
    - Always have a context (cbc9ed7)
    - Use write for convenience (21db64c)
    - Better syntax (7166f54)
    - Rename new_with_dir to from_dir (248e9d3)
    - Use with_context for lazy eval (39a9e21)
    - Use create_nats_client_from_opts from wash-lib (cb4d311)
    - Refactor!(wash-cli): initialize contexts consistently (703283b)
    - Exclude test run for windows; will be dealt with in another PR. (9da236f)
    - Fix test for lattice_prefix getter (e2927c6)
    - More refactoring... (7d6155e)
    - Moving things around, better scopring for lattice_prefix parsing on app cmds (9bf9acc)
    - Proper derivation of lattice_prefix (ie, lattice_prefix arg > context arg > $current_default context.lattice_prefix) (70ac131)
    - Ensure expected behavior when creating/switching context (7da3e83)
    - Remove direct `wasmbus_rpc` dependency (8e071dd)
    - Address clippy issues (9c8abf3)
    - Rebased with upstream/main to fix failing unit test (42ccace)
    - Make revision required (w/ default) on wasmcloud.toml commong config (30b835d)
    - Require revision and version args on sign cmd (4fb8118)
    - Correct typo and link in README (8240af2)
</details>

## v0.13.0 (2023-11-01)

### Chore

 - <csr-id-ee51a176a00b3f8fe03e0d3212a9da6dbfd6044f/> release wash-lib-v0.13.0
 - <csr-id-a1c3b9d86db14f31ef7fbebeb30e8784f974df6f/> support domain, links, keys alias
 - <csr-id-007660e96ad7472918bc25baf9d52d60e5230823/> update control interface 0.31
 - <csr-id-dfad0be609868cbd0f0ce97d7d9238b41996b5fc/> integrate `wash` into the workspace
 - <csr-id-5ef2c4c924dbc2d93a75f99b5975b321e1bad75f/> remove unused var
 - <csr-id-9caf89a7d15a7d8ec80a490fe0f4106089c77728/> update test message

### New Features

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

### Bug Fixes

 - <csr-id-ef3e4e584fef4d597cab0215fdf3cfe864f701e9/> Configure signing keys directory for build cmd
   The keys directory can be specified via wasmcloud.toml, CLI arguments (`--keys-directory`), or environment variable (`WASH_KEYS`).

### Other

 - <csr-id-621e449a1e70f9216016b11a6ff50c7a1def10e1/> update dependencies

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

### Chore (BREAKING)

 - <csr-id-a1e8d3f09e039723d28d738d98b47bce54e4450d/> update ctl to 0.31.0
 - <csr-id-d53bf1b5e3be1cd8d076939cc80460305e30d8c5/> remove prov_rpc options

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 18 commits contributed to the release over the course of 14 calendar days.
 - 16 days passed between releases.
 - 15 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Release wash-lib-v0.13.0 (ee51a17)
    - Support domain, links, keys alias (a1c3b9d)
    - Update control interface 0.31 (007660e)
    - Update ctl to 0.31.0 (a1e8d3f)
    - Apply tags in actor config during signing (810e220)
    - Merge pull request #807 from rvolosatovs/merge/wash (f2bc010)
    - Integrate `wash` into the workspace (dfad0be)
    - Generate golang code during wash build (17bb1aa)
    - Update dependencies (621e449)
    - Configure signing keys directory for build cmd (ef3e4e5)
    - `Err(anyhow!(...))` -> `bail!`, err msg capitals (5af1c68)
    - Mark components built with wash as experimental (462767b)
    - Remove unused var (5ef2c4c)
    - Remove prov_rpc options (d53bf1b)
    - Merge pull request #922 from vados-cosmonic/refactor/light-testing-code-refactor (0b9e1ca)
    - Various fixes to testing code (372e81e)
    - Merge pull request #914 from connorsmith256/chore/update-test (516aa5e)
    - Update test message (9caf89a)
</details>

## v0.12.1 (2023-10-16)

### Chore

 - <csr-id-5ae8fd8bad3fadb5b97be28d5e163b621938a272/> bump wash-lib and wash-cli for wit-parser fix

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release.
 - 2 days passed between releases.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump wash-lib and wash-cli for wit-parser fix (5ae8fd8)
</details>

## v0.12.0 (2023-10-13)

### Chore

 - <csr-id-70b20a12553e84697ffe9f8dbf32219162bdf946/> update async_nats,ctl,wasmbus_rpc to latest
 - <csr-id-c44f657e3bdc1e4a6679b3cc687b7039fb729f34/> bump to 0.21.0, wash-lib 0.12.0

### New Features

 - <csr-id-5c0ccc5f872ad42b6152c66c34ab73f855f82832/> query all host inventories
 - <csr-id-109e934ceaa026f81aeadaca84e7da83668dc5fd/> add scale and update integration tests
 - <csr-id-32ea9f9eb8ba63118dfd23084d413aae23226124/> polishing app manifest loader
 - <csr-id-6907c8012fd59bbcaa6234c533b62ba997b86139/> http & stdin manifest input sources support for put & deploy cmds

### Bug Fixes

 - <csr-id-1fa7604d3347df6c0cfb71b8ea4be6bba9bceb34/> for app manifest loading, file input source check should preceed http input source.
 - <csr-id-0eb5a7cade13a87e59c27c7f6faa89234d07863d/> some cleanup relevant to app manifest input sources

### Refactor

 - <csr-id-571a25ddb7d8f18b2bb1d3f6b22401503d31f719/> add manifest source type to use with app manifest loader.
 - <csr-id-ee29478631ba0df2d67a00e3f1336b4c40099489/> adjustments to app manifest loader

### New Features (BREAKING)

 - <csr-id-7851a53ab31273b04df8372662198ac6dc70f78e/> add scale and update cmds
 - <csr-id-bb69ea644d95517bfdc38779c2060096f1cec30f/> update to start/stop/scale for concurrent instances

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 14 commits contributed to the release over the course of 4 calendar days.
 - 8 days passed between releases.
 - 12 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #873 from connorsmith256/feat/get-all-inventories (3b58fc7)
    - Query all host inventories (5c0ccc5)
    - Merge pull request #875 from ahmedtadde/feat/expand-manifest-input-sources-clean (c25352b)
    - For app manifest loading, file input source check should preceed http input source. (1fa7604)
    - Add manifest source type to use with app manifest loader. (571a25d)
    - Add scale and update integration tests (109e934)
    - Add scale and update cmds (7851a53)
    - Update to start/stop/scale for concurrent instances (bb69ea6)
    - Update async_nats,ctl,wasmbus_rpc to latest (70b20a1)
    - Bump to 0.21.0, wash-lib 0.12.0 (c44f657)
    - Adjustments to app manifest loader (ee29478)
    - Some cleanup relevant to app manifest input sources (0eb5a7c)
    - Polishing app manifest loader (32ea9f9)
    - Http & stdin manifest input sources support for put & deploy cmds (6907c80)
</details>

## v0.11.4 (2023-10-05)

### Bug Fixes

 - <csr-id-2b55ae469c07af8bd94e21f606584ef67e2e0f9a/> typo
 - <csr-id-6d71c1f36111efe1942e522c8ac6b315c78d81ab/> unify rust and tinygo component target logic

### Other

 - <csr-id-b3965d7bb04e70da967bc393b9455c4c1da6b20b/> wash-lib v0.11.4

### Refactor

 - <csr-id-ddd3b072e8ec4236936c2cb53af1521ab1abeded/> embed component metadata

### Test

 - <csr-id-1495c8f3e6fdda67a90fc821a731072b72fc4062/> add wit_world to test case

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 8 commits contributed to the release.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #864 from connorsmith256/release/wash-lib-v0.11.4 (79a2cef)
    - Wash-lib v0.11.4 (b3965d7)
    - Merge pull request #758 from wasmCloud/tg_wasi_respect (a7df4cb)
    - Add wit_world to test case (1495c8f)
    - Typo (2b55ae4)
    - Embed component metadata (ddd3b07)
    - Unify rust and tinygo component target logic (6d71c1f)
    - Add to wasi target tinygo builder (3d5517c)
</details>

## v0.11.3 (2023-10-05)

### Other

 - <csr-id-4a4c148f2e1ddb3eba535b40575265f51968ffaa/> wash-lib v0.11.3

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 2 commits contributed to the release.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #863 from connorsmith256/release/wash-lib-v0.11.3 (590159c)
    - Wash-lib v0.11.3 (4a4c148)
</details>

## v0.11.2 (2023-10-05)

### Chore

 - <csr-id-016c37812b8cf95615a6ad34ee49de669c66886b/> fix lint

### Other

 - <csr-id-b9c23d959c5fb0a1854b8f90db6a0a0e4b1cdda9/> wash-lib v0.11.2

### New Features (BREAKING)

 - <csr-id-90f79447bc0b1dc7efbef2b13af9cf715e1ea1f0/> add par command support to wash-lib
   * Added par support to wash-lib

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 5 commits contributed to the release over the course of 1 calendar day.
 - 5 days passed between releases.
 - 3 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #861 from connorsmith256/release/wash-lib-v0.11.2 (f35dcad)
    - Wash-lib v0.11.2 (b9c23d9)
    - Merge pull request #849 from vados-cosmonic/chore/fix-lint (894329f)
    - Fix lint (016c378)
    - Add par command support to wash-lib (90f7944)
</details>

## v0.11.1 (2023-09-29)

### Other

 - <csr-id-f582dc07ea768f9b52b13c7d5c618c36e4ff0a0c/> wash-lib v0.11.1

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release.
 - 2 days passed between releases.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #840 from wasmCloud/release/wash-lib-v0.11.1 (64bdebf)
    - Wash-lib v0.11.1 (f582dc0)
    - Merge pull request #839 from aish-where-ya/fix/update-actor (6d98a6d)
    - Minor fix to update actor in wash-lib (3dbbc03)
</details>

## v0.11.0 (2023-09-26)

### New Features

 - <csr-id-99262d8b1c0bdb09657407663e2d5d4a3fb7651c/> move update-actor for wash ctl update to wash-lib.
 - <csr-id-6405f6ce45d43850ca427c4d80ca50369ee10405/> add support for Android releases

### Bug Fixes

 - <csr-id-3351e0a83bc92dab8b73bc88b8d03a95dfad3e0a/> move generate key message to info log

### Other

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

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 7 commits contributed to the release over the course of 12 calendar days.
 - 20 days passed between releases.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #832 from connorsmith256/release/wash-lib-v0.11.0 (f635d63)
    - V0.11.0 (0f5add0)
    - Move update-actor for wash ctl update to wash-lib. (99262d8)
    - Merge pull request #822 from rvolosatovs/feat/android (4bde6b7)
    - Add support for Android releases (6405f6c)
    - Move generate key message to info log (3351e0a)
    - Bump cargo_metadata from 0.17.0 to 0.18.0 (3797857)
</details>

## v0.10.1 (2023-09-06)

### Chore

 - <csr-id-bb76aec405e437c249d385e3492cb67932960125/> bump to 0.10.1 to release wadm
 - <csr-id-bbf0b1a6074108a96d9534500c97c8ad5ed13dd6/> remove references to DASHBOARD_PORT

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
    - Bump to 0.10.1 to release wadm (bb76aec)
    - Remove references to DASHBOARD_PORT (bbf0b1a)
</details>

## v0.10.0 (2023-09-02)

### New Features

 - <csr-id-78b99fde8606febf59e30f1d12ac558b29d425bf/> set default to Rust host
   - update paths to release binary
   - allow-file-upload default bug
   - mention dashboard ui cmd

### Bug Fixes

 - <csr-id-f9279294ea7602ad6bbc55a5f3dc8940f2d46d71/> update test to reflect changes from OTP to Rust host
 - <csr-id-7111b5d9a5ece7543ded436b7816974ad27910e2/> config loading for preview2 adapter path

### Other

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

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 8 commits contributed to the release over the course of 11 calendar days.
 - 36 days passed between releases.
 - 6 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #762 from wasmCloud/release/v0.10.0 (308a3cb)
    - Wash-lib v0.10.0 (e67ded6)
    - Merge pull request #759 from wasmCloud/rust-host-default (6be0162)
    - Update test to reflect changes from OTP to Rust host (f927929)
    - Use rc2 (f4a9cd6)
    - Set default to Rust host (78b99fd)
    - Bump cargo_metadata from 0.15.4 to 0.17.0 (a4f67e5)
    - Config loading for preview2 adapter path (7111b5d)
</details>

## v0.9.3 (2023-07-27)

### Bug Fixes

 - <csr-id-b0e746be713d070b4400294ec401b87444bd5741/> preserve interactive terminal when checking git

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release.
 - 5 days passed between releases.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Preserve interactive terminal when checking git (b0e746b)
</details>

## v0.9.2 (2023-07-21)

### Chore

 - <csr-id-10ede9e84e537fecbad3cbbb09960506b6359ef4/> use released wasmcloud-component-adapters

### New Features

 - <csr-id-4144f711ad2056e9334e085cbe08663065605b0c/> build wasi preview components from wash
 - <csr-id-bb454cb3ae1ff05d8381ba2ea1f48b461d059474/> add p2 target to wasmcloud.toml

### Other

 - <csr-id-ae65e85bf4b8bcbc215d48664fcf6941d25de165/> v0.9.2

### New Features (BREAKING)

 - <csr-id-acdcd957bfedb5a86a0420c052da1e65d32e6c23/> allow get inventory to query the only host

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 14 commits contributed to the release over the course of 16 calendar days.
 - 19 days passed between releases.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: #677

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#677**
    - Adding the ability to inspect and inject configuration schemas (db3fe8d)
 * **Uncategorized**
    - Merge pull request #682 from vados-cosmonic/release/wash-lib/v0.9.2 (0f9df26)
    - Merge pull request #684 from vados-cosmonic/chore/use-upstream-fix-for-windows-component-adapter (9b42815)
    - Merge pull request #683 from wasmCloud/feat/single-host-inventory-query (3fe92ae)
    - Use released wasmcloud-component-adapters (10ede9e)
    - Allow get inventory to query the only host (acdcd95)
    - V0.9.2 (ae65e85)
    - Merge pull request #663 from vados-cosmonic/feat/support-adapting-p2-components (28c4aa6)
    - Build wasi preview components from wash (4144f71)
    - Merge pull request #643 from lachieh/detachable-washboard (6402d13)
    - Add standalone washboard (experimental) (12fdad0)
    - Add p2 target to wasmcloud.toml (bb454cb)
    - Merge pull request #629 from thomastaylor312/fix/multiple_nats (389a702)
    - Corrected creds escaping on Windows (d47f2b4)
</details>

## v0.9.1 (2023-07-02)

### New Features

 - <csr-id-02b1f03e05c4ffc7b62d2438752344cd2c805d3f/> first check that git command is installed
 - <csr-id-f9658287e6bdb77a6991e827454951a0711bce42/> return an explicit error when the build tools don't exist
 - <csr-id-e9fe020a0906cb377f6ea8bd3a9879e5bad877b7/> add wash dev command

### Bug Fixes

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

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 9 commits contributed to the release over the course of 17 calendar days.
 - 19 days passed between releases.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bumped cargo versions for wash-lib 0.9.1 wash 0.18.1 (30ca8e0)
    - First check that git command is installed (02b1f03)
    - Return an explicit error when the build tools don't exist (f965828)
    - Allows multiple hosts to run without sharing data (4900f82)
    - Merge pull request #619 from vados-cosmonic/fix/flaky-tests (eb9de36)
    - Flaky tests (c7643e8)
    - Merge pull request #610 from vados-cosmonic/feat/add-wash-dev (00e0aea)
    - Add wash dev command (e9fe020)
    - Added kvcounter template to wash favorites (e6b874c)
</details>

## v0.9.0 (2023-06-13)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 3 commits contributed to the release.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Moved registry cli things to registry cli (1172806)
    - Fixed wash-lib release failure (0f6b5c2)
    - Bumped to stable versions, 0.18.0 (811eb48)
</details>

## v0.9.0-alpha.3 (2023-06-13)

### New Features

 - <csr-id-8c96789f1c793c5565715080b84fecfbe0653b43/> Adds a new experimental `wash capture` command
   This one is very experimental, so I didn't even add it to the top
   level help text, but it is all manually tested and good to go
 - <csr-id-e58c6a60928a7157ffbbc95f9eabcc9cae3db2a7/> Adds `wash spy` command with experimental flag support
 - <csr-id-6923ce7efb721f8678c33f42647b87ea33a7653a/> flatten multiple commands into wash get
 - <csr-id-4daf51be422d395bc0142d62b8d59060b89feafa/> flatten wash reg push/pull into wash push/pull
 - <csr-id-128f7603c67443f23e76c3cb4bd1468ffd8f5462/> flatten `wash ctl stop` into `wash stop`
 - <csr-id-2a6c401834b4cb55ef420538e15503b98281eaf1/> flatten `wash ctl start` into `wash start`
 - <csr-id-24bba484009be9e87bfcbd926a731534e936c339/> flatten `wash ctl link` into `wash link`

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 15 commits contributed to the release over the course of 20 calendar days.
 - 21 days passed between releases.
 - 7 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: #556

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#556**
    - Feat(*) wash burrito support (812f0e0)
 * **Uncategorized**
    - Merge pull request #612 from thomastaylor312/feat/wash_capture (3a14bbc)
    - Adds a new experimental `wash capture` command (8c96789)
    - Merge pull request #603 from thomastaylor312/feat/wash_spy (213ac6b)
    - Adds `wash spy` command with experimental flag support (e58c6a6)
    - Bumps wadm to 0.4.0 stable (41d3d3c)
    - Flatten multiple commands into wash get (6923ce7)
    - Merge pull request #580 from vados-cosmonic/feat/ux/wash-reg-push-and-pull (a553348)
    - Flatten wash reg push/pull into wash push/pull (4daf51b)
    - Merge pull request #576 from vados-cosmonic/feat/ux/flatten-wash-stop (7b66d65)
    - Flatten `wash ctl stop` into `wash stop` (128f760)
    - Merge pull request #573 from vados-cosmonic/feat/ux/flatten-wash-start (612951b)
    - Flatten `wash ctl start` into `wash start` (2a6c401)
    - Merge pull request #569 from vados-cosmonic/feat/ux/flatten-wash-link (def34b6)
    - Flatten `wash ctl link` into `wash link` (24bba48)
</details>

## v0.9.0-alpha.2 (2023-05-22)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 3 commits contributed to the release over the course of 4 calendar days.
 - 7 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: #560

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#560**
    - Bug build actor cargo workspace #wasm cloud/wash/446 (410d87c)
 * **Uncategorized**
    - Removed error in generate (ec4e20b)
    - Bumped wadm to 0.4.0-alpha.3 (a01b605)
</details>

## v0.9.0-alpha.1 (2023-05-15)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release over the course of 16 calendar days.
 - 23 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: #520

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#520**
    - Feat(*) wadm 0.4 support in `wash app` (b3e2615)
 * **Uncategorized**
    - Fixed ci, ensured wadm doesn't connect to default nats (b348399)
    - Set up 0.18.0 alpha release for testing (3320ee7)
    - #466 Update toml crate, which required updating weld-codegen. (1915f2d)
</details>

## v0.8.1 (2023-04-21)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release.
 - 2 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Patched start wasmcloud to accept dashboard port (b68bbfc)
</details>

## v0.8.0 (2023-04-18)

### Bug Fixes

 - <csr-id-89e638a8e63073800fc952c0a874e54e9996d422/> Bumps wash-lib version
   This was missed and so cargo installing from main causes issues. Also
   bumps 0.17 so that it can pick up the new version from crates. Once this
   is published we should yank 0.17.0

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 8 commits contributed to the release over the course of 32 calendar days.
 - 32 days passed between releases.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #522 from thomastaylor312/chore/bump_wash_lib (5b8441b)
    - Bumps wash-lib version (89e638a)
    - Merge pull request #513 from connorsmith256/feat/allow-file-upload (bf4e46c)
    - Satisfy clippy (4f5afad)
    - Merge pull request #508 from aish-where-ya/main (6fd026c)
    - Refactoring based on review comments (448211e)
    - Wash up waits for washboard to be up (efaacd7)
    - Merge pull request #379 from ceejimus/bug/latest-tags-w-no-allow-latest (ec5240b)
</details>

## v0.7.1 (2023-03-16)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 5 commits contributed to the release over the course of 1 calendar day.
 - 3 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: #459

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#459**
    - Removed workspace deps for wash-lib modules (6170336)
 * **Uncategorized**
    - Merge pull request #477 from connorsmith256/bump/wasmcloud-host-version (7dbd961)
    - Bump to v0.61.0 of wasmcloud host (3d80c4e)
    - [fix] make regex required (fb5f5d2)
    - [fix] add better error handling for empty tags when --allow-latest is false (98faa4a)
</details>

## v0.7.0 (2023-03-13)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release over the course of 5 calendar days.
 - 5 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: #452

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#452**
    - Feat/wash inspect (0b2f0d3)
 * **Uncategorized**
    - Merge pull request #467 from connorsmith256/bump/versions (423c0ad)
    - Bump versions to same commit (6df3165)
    - Merge branch 'main' into fix/nextest-usage-in-makefile (03c02f2)
</details>

## v0.6.1 (2023-03-08)

### Bug Fixes

 - <csr-id-656ea644696ea97bdafdbf8d5fd4a5e736593fc8/> use lib.name from cargo.toml for rust wasm binary name
   * fix(rust): read wasm binary name from cargo.toml explicitly

### Other

 - <csr-id-0ed956f457a94ad390b847a46df9911e5ebb35a9/> wash v0.16.1, wash-lib v0.6.1
 - <csr-id-80b104011536c03ef3c1c58a1440992defae1351/> adopt workspace dependencies
   This simplifies maintenance of the repository and allows for easier
   audit of the dependencies

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 10 commits contributed to the release over the course of 15 calendar days.
 - 26 days passed between releases.
 - 3 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 5 unique issues were worked on: #390, #393, #399, #400, #407

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#390**
    - Use lib.name from cargo.toml for rust wasm binary name (656ea64)
 * **#393**
    - Fix clippy lints (030b844)
 * **#399**
    - Use exact imports instead of globs (95851b6)
 * **#400**
    - Remove git command output from `wash new actor` output and add message about cloning the template (f9a656f)
 * **#407**
    - Adopt workspace dependencies (80b1040)
 * **Uncategorized**
    - Merge pull request #450 from vados-cosmonic/release/wash-lib/v0.6.1 (8a3e9c7)
    - Wash v0.16.1, wash-lib v0.6.1 (0ed956f)
    - Merge pull request #420 from thomastaylor312/fml/less_flakes_by_making_it_nap (bbba36f)
    - Makes sure we wait for the NATS server to be up before continuing with the host (51e63e4)
    - Adds more error messaging around some flakes (e3e3c0a)
</details>

## v0.6.0 (2023-02-09)

### Bug Fixes

 - <csr-id-2e69e12d4b78f5ea7710ba12226345440e7541ef/> Makes sure that wash downloads different versions of wasmcloud
   This now downloads different versions to different directories. Also did
   a little bit of cleanup with some clippy warnings in the tests and bumping
   NATS to a later version

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release.
 - 6 days passed between releases.
 - 1 commit was understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Merge pull request #381 from wasmCloud/bump/0.15.0-wasmcloud-0.60.0 (b06b71b)
    - Bumps wash lib version, as the semver gods intended (e3c423b)
    - Addresses PR comment (1609b0d)
    - Makes sure that wash downloads different versions of wasmcloud (2e69e12)
</details>

## v0.5.1 (2023-02-03)

### New Features

 - <csr-id-12cae48ff806b26b6c4f583ae00337b21bc65d3c/> consume new wascap and hashing
   This updates to a newer version of wasmparser
   which should fix attempting to sign newer wasi modules.
   
   The integration test caught an issue introduced a long
   time ago with wascap v0.5.0 and a very old module
   signed with that version from way back when.
   v0.9.2 of wascap fixes this issue in our integration
   tests by correctly removing the old metadata.
   
   Bump wascap - looks small but NOTE:
   
   The hashes computed with v0.9.0 and later of wascap
   are not compatible with the hashes signed by prior versions.
   As a result, modules signed with older versions of wascap
   will not have their module hashes validated
   (they'll be ignored).
   
   Once the module has been signed with 0.9.0 or greater,
   it will go back to having its module hash verified.

### Bug Fixes

 - <csr-id-5cc6ebe2b8596b5fb1a56abb4d17e4e3f104b110/> grant execute permission to `mac_listener` for hot-reloading
   * fix(wash-up): grant execute permission to `mac_listener` for hot-reloading

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 9 commits contributed to the release over the course of 24 calendar days.
 - 25 days passed between releases.
 - 2 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 3 unique issues were worked on: #359, #375, #376

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#359**
    - Grant execute permission to `mac_listener` for hot-reloading (5cc6ebe)
 * **#375**
    - Allow prerelease tags with warning (a3aebd2)
 * **#376**
    - Create default context if host_config not found (51d4748)
 * **Uncategorized**
    - Merge pull request #368 from connorsmith256/add-echo-messaging-template (2808632)
    - Bump wash-lib to v0.5.0 (7baa633)
    - Add echo-messaging to default templates (fc38533)
    - Merge pull request #361 from ricochet/bump-wascap (eba79d4)
    - Merge branch 'main' into bump-wascap (cd35ff9)
    - Consume new wascap and hashing (12cae48)
</details>

## v0.4.0 (2023-01-09)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release.
 - 23 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: #363

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#363**
    - Pinned to stable versions for 0.14.0 release (223096b)
</details>

## v0.4.0-alpha.4 (2022-12-16)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release.
 - 1 day passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: #355

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#355**
    - Moved generate module to wash-lib (9fa5331)
</details>

## v0.4.0-alpha.3 (2022-12-14)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 2 commits contributed to the release.
 - 8 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 2 unique issues were worked on: #353, #354

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#353**
    - Moved project build functionality to wash-lib (c31a5d4)
 * **#354**
    - Fixed 352, added js_domain to context (c7f4c1d)
</details>

## v0.4.0-alpha.2 (2022-12-06)

### New Features

 - <csr-id-84b95392993cbbc65da36bc8b872241cce32a63e/> Moves claims and registry code into wash lib
   Sorry for the big PR here, but due to a bunch of codependent code,
   I had to move a bunch of stuff at once. There are two main threads
   to this PR. First, I noticed that the claims code is all CLI specific,
   but it is likely that anyone building a CLI will not want to rewrite that
   again. If you are doing this purely in code, you can just use the
   wascap library. To make this work, I started added the CLI specific stuff
   to the `cli` module of wash lib. There will probably be other things we
   add to it as we finish this refactor
   
   Second, this moves the reusable registry bits into its own module, which
   is super handy even for those not doing a CLI as it avoids direct
   interaction with the lower level OCI crates
 - <csr-id-a62b07b8ff321c400c6debefdb6199e273445490/> Adds new keys module to wash-lib
   Please note that this introduces one small breaking change to output
   that removes the `.nk` suffix from the list of keys. However, there is
   backward compatibility for providing <key_name>.nk to `wash keys get`
   so it will still function as it did previously. This change was
   specifically made because the key name is more important than the suffix.
   If desired, I can back out that change, but it seemed to make more sense
   to make it less like a wash-specific `ls` of a directory
 - <csr-id-d0659d346a6acadf81ce8dd952262f372c738e8d/> Adds new context tests
 - <csr-id-b1bf6b1ac7851dc09e6757d7c2bde4558ec48098/> Adds drain command to wash lib
   This also starts the process of creating a `config` module that I'll
   continue to update as I push forward the other PRs. Please note that
   this is the first of many PRs. I plan on doing each command as a separate
   PR instead of a mega PR

### Other

 - <csr-id-52ef5b6b1b6b01bc5e7a2c8fe3cbb2a08d4ad864/> Creates new context library
   This creates a new context library with some extendable traits for
   loading as well as a fully featured module for handling context on
   disk.
   
   Additional tests will be in the next commit

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 15 commits contributed to the release over the course of 19 calendar days.
 - 25 days passed between releases.
 - 5 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 2 unique issues were worked on: #333, #346

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#333**
    - Parse version and name from `Cargo.toml` when not provided in `wasmcloud.toml`. (dfa9994)
 * **#346**
    - Bump dependencies (0178c36)
 * **Uncategorized**
    - Merge pull request #345 from thomastaylor312/lib/claims (b0e385d)
    - Bring over to_lowercase (6cab2aa)
    - Moves claims and registry code into wash lib (84b9539)
    - Merge pull request #344 from thomastaylor312/lib/keys (08bbb0f)
    - Adds new keys module to wash-lib (a62b07b)
    - Merge pull request #339 from thomastaylor312/lib/context (10f9c1b)
    - Fixes issue with creating initial context (92f448e)
    - Adds deleting of default context (d658dc4)
    - Adds new context tests (d0659d3)
    - Creates new context library (52ef5b6)
    - Merge pull request #337 from thomastaylor312/feat/wash-lib (06cea91)
    - Addresses PR comments (2fa41d5)
    - Adds drain command to wash lib (b1bf6b1)
</details>

## v0.4.0-alpha.1 (2022-11-10)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 4 commits contributed to the release.
 - 16 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 2 unique issues were worked on: #327, #329

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#327**
    - Feat/wash down (33cdd7d)
 * **#329**
    - Fix credentials path format for Windows (e81addb)
 * **Uncategorized**
    - Merge pull request #330 from connorsmith256/fix/running-host-check (c023d59)
    - Pass env vars when checking for running host (f2c2276)
</details>

## v0.3.1 (2022-10-25)

### Chore

 - <csr-id-a1d77b0e12ebb7b4b946004b61a208482e599ce4/> bump wash version
 - <csr-id-2aa4b041af6195ff4dbd6bf7e04f6cba281585b9/> fix clippy warnings

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 5 commits contributed to the release over the course of 24 calendar days.
 - 33 days passed between releases.
 - 2 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: #318

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#318**
    - Set stdin to null when starting a wasmcloud host with wash-lib (38e05b2)
 * **Uncategorized**
    - Merge pull request #321 from thomastaylor312/chore/0.13_update (38fbf3a)
    - Bump wash version (a1d77b0)
    - Merge pull request #317 from ricochet/chore/clap-v4 (c6ab554)
    - Fix clippy warnings (2aa4b04)
</details>

## v0.3.0 (2022-09-21)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release over the course of 1 calendar day.
 - 11 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: #297

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#297**
    - Create `wash build` command and add configuration parsing (f72ca88)
</details>

## v0.2.0 (2022-09-09)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release.
 - 36 days passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 1 unique issue was worked on: #303

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#303**
    - Update wash-lib with minimum version requirement and mix releases (13d44c7)
</details>

## v0.1.0 (2022-08-04)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 2 commits contributed to the release over the course of 11 calendar days.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 2 unique issues were worked on: #292, #294

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#292**
    - [FEATURE] Adding `wash-lib`, implementing `start` functionality (b77b90d)
 * **#294**
    - `wash up` implementation (3104999)
</details>


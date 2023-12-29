# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 1 commit contributed to the release.
 - 1 day passed between releases.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump wasmcloud-control-interface v0.33.0, safety bump 2 crates ([`c585084`](https://github.com/connorsmith256/wasmcloud/commit/c585084d47e4b07c3bee295485a3302b0f071bf2))
</details>

## v0.25.0 (2023-12-28)

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

 - 18 commits contributed to the release over the course of 29 calendar days.
 - 35 days passed between releases.
 - 18 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Update wasmcloud version to 0.81 ([`c12eff1`](https://github.com/connorsmith256/wasmcloud/commit/c12eff1597e444fcd926dbfb0abab547b2efc2b0))
    - Consistently prefix cli flags ([`715e94e`](https://github.com/connorsmith256/wasmcloud/commit/715e94e7f1a35da002769a0a25d531606f003d49))
    - Prefix absolute path references with file:// ([`d91e92b`](https://github.com/connorsmith256/wasmcloud/commit/d91e92b7bd32a23804cafc4381e7648a151ace38))
    - Remove references to PROV_RPC settings ([`8b751e4`](https://github.com/connorsmith256/wasmcloud/commit/8b751e4e9bce78281f6bf6979bfb70c3f6b33634))
    - Force minimum wasmCloud version to 0.81 ([`b0e6c1f`](https://github.com/connorsmith256/wasmcloud/commit/b0e6c1f167c9c2e06750d72f10dc729d17f0b81a))
    - Pin wasmcloud version to 0.81-rc1 ([`b0fdf60`](https://github.com/connorsmith256/wasmcloud/commit/b0fdf60a33d6866a92924b02e5e2ae8544e421a5))
    - Bump wash-cli to 0.25 ([`b7e54e7`](https://github.com/connorsmith256/wasmcloud/commit/b7e54e7bbccd1fbcb4f1a9f77cb1a0289f8a239b))
    - Convert httpserver to provider-wit-bindgen ([`046fd4c`](https://github.com/connorsmith256/wasmcloud/commit/046fd4c735c8c0ebb2f5a64ae4b5a762b0034591))
    - Add support for inspecting wit ([`a864157`](https://github.com/connorsmith256/wasmcloud/commit/a86415712621504b820b8c4d0b71017b7140470b))
    - Remove deprecated code related to start actor cmd ([`7de3182`](https://github.com/connorsmith256/wasmcloud/commit/7de31820034c4b70ab6edc772713e64aafe294a9))
    - Update parsing from RegistryCredential to RegistryAuth ([`65d2e28`](https://github.com/connorsmith256/wasmcloud/commit/65d2e28d54929b8f4d0b39077ee82ddad2387c8e))
    - Revised implementation of registry url and credentials resolution ([`57d014f`](https://github.com/connorsmith256/wasmcloud/commit/57d014fb7fe11542d2e64068ba86e42a19f64f98))
    - Some cleanup before revised implementation ([`4e9bae3`](https://github.com/connorsmith256/wasmcloud/commit/4e9bae34fe95ecaffbc81fd452bf29746b4e5856))
    - Replace broken URLs ([`25af017`](https://github.com/connorsmith256/wasmcloud/commit/25af017f69652a98b8969609e2854636e2bc7553))
    - Update format for serialized claims ([`37618a3`](https://github.com/connorsmith256/wasmcloud/commit/37618a316baf573cc31311ad3ae78cd054e0e2b5))
    - Refactor command parsing for readability ([`7bc207b`](https://github.com/connorsmith256/wasmcloud/commit/7bc207bf24873e5d916edf7e8a4b56c7ed04b9a7))
    - Add support for custom build command ([`023307f`](https://github.com/connorsmith256/wasmcloud/commit/023307fcb351a67fe2271862ace8657ac0e101b6))
    - Enable only signing actors ([`bae6a00`](https://github.com/connorsmith256/wasmcloud/commit/bae6a00390e2ac10eaede2966d060477b7091697))
</details>

## v0.24.1 (2023-11-22)

### Chore

 - <csr-id-a972375413491a180dec6c7a3948eee597850340/> update brew install command

### Other

 - <csr-id-19f34054fddb6991a51ee8ab953cf36ef4c79399/> bump to 0.24.1

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
    - Bump to 0.24.1 ([`19f3405`](https://github.com/connorsmith256/wasmcloud/commit/19f34054fddb6991a51ee8ab953cf36ef4c79399))
    - Update brew install command ([`a972375`](https://github.com/connorsmith256/wasmcloud/commit/a972375413491a180dec6c7a3948eee597850340))
    - Move more wash invocations into TestWashInstance ([`85193dd`](https://github.com/connorsmith256/wasmcloud/commit/85193dd0a6f1892cd04c231b40b206720089fa3e))
</details>

## v0.24.0 (2023-11-21)

### Chore

 - <csr-id-bfb51a2dc47d09af1aec0ec4cb23654f93903f25/> update docker dep versions

### Documentation

 - <csr-id-20ffecb027c225fb62d60b584d6b518aff4ceb51/> update wash URLs
 - <csr-id-3d37a8615f2c40c4fbb089b9e8d9263e9e163c16/> update installation instructions for wash

### Other

 - <csr-id-9f0fefeeaba9edc016b151e94c4dc0b57a44882e/> bump wash to 0.24.0

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

 - 7 commits contributed to the release over the course of 6 calendar days.
 - 7 days passed between releases.
 - 7 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Bump wash to 0.24.0 ([`9f0fefe`](https://github.com/connorsmith256/wasmcloud/commit/9f0fefeeaba9edc016b151e94c4dc0b57a44882e))
    - Removes need for actor/provider/host IDs in almost all cases ([`ce7904e`](https://github.com/connorsmith256/wasmcloud/commit/ce7904e6f4cc49ca92ec8dee8e263d23da26afd0))
    - Update docker dep versions ([`bfb51a2`](https://github.com/connorsmith256/wasmcloud/commit/bfb51a2dc47d09af1aec0ec4cb23654f93903f25))
    - Add a test for wash up labels ([`dc003f8`](https://github.com/connorsmith256/wasmcloud/commit/dc003f8dd193648988927d312958c6c79c980aaf))
    - Add integration test for wash-call ([`267d24d`](https://github.com/connorsmith256/wasmcloud/commit/267d24dcdc871bbc85c0adc0d102a632310bb9f0))
    - Update wash URLs ([`20ffecb`](https://github.com/connorsmith256/wasmcloud/commit/20ffecb027c225fb62d60b584d6b518aff4ceb51))
    - Update installation instructions for wash ([`3d37a86`](https://github.com/connorsmith256/wasmcloud/commit/3d37a8615f2c40c4fbb089b9e8d9263e9e163c16))
</details>

## v0.23.0 (2023-11-14)

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
    - V0.23.0 ([`694bf86`](https://github.com/connorsmith256/wasmcloud/commit/694bf86d100c98d9b1c771972e96a15d70fef116))
    - Allow specifying --nats-remote-url without --nats-credsfile ([`c7b2a1d`](https://github.com/connorsmith256/wasmcloud/commit/c7b2a1dd9f96542982fd8e4f188eca374d51db7d))
    - Use --nats-js-domain for NATS server ([`3b4da1d`](https://github.com/connorsmith256/wasmcloud/commit/3b4da1d734e3217dc63f09971a4046d4818cabb3))
    - Add --wadm-js-domain option ([`6098e24`](https://github.com/connorsmith256/wasmcloud/commit/6098e2488729a0fd50a71623699d9ee257da43d9))
    - Remove support for bindle references ([`5301084`](https://github.com/connorsmith256/wasmcloud/commit/5301084bde0db0c65811aa30c48de2a63e091fcf))
    - Add doc comment for label option ([`572c4cd`](https://github.com/connorsmith256/wasmcloud/commit/572c4cd62bb4645da90ffd69f92e9422a632e628))
    - Add --label option to wash up ([`1965698`](https://github.com/connorsmith256/wasmcloud/commit/196569848412e5680a2d286d449f20776f7de26e))
    - Use valid host and public keys for wash call ([`61da617`](https://github.com/connorsmith256/wasmcloud/commit/61da61726c5a9a791a96d9a42014822d4872fd57))
    - Always have a context ([`cbc9ed7`](https://github.com/connorsmith256/wasmcloud/commit/cbc9ed7008f8969312534e326cf119dbbdf89aaa))
    - Rename new_with_dir to from_dir ([`248e9d3`](https://github.com/connorsmith256/wasmcloud/commit/248e9d3ac60fdd2b380723e9bbaf1cc8023beb44))
    - Use with_context for lazy eval ([`39a9e21`](https://github.com/connorsmith256/wasmcloud/commit/39a9e218418a0662de4edabbc9078268ba095842))
    - Use create_nats_client_from_opts from wash-lib ([`cb4d311`](https://github.com/connorsmith256/wasmcloud/commit/cb4d311c6d666e59c22199f950757abc65167f53))
    - Continue passing PROV_RPC variables until the host removes support ([`d9e0804`](https://github.com/connorsmith256/wasmcloud/commit/d9e08049aaefa0c6c1f3d112c5423ac205b448b0))
    - Respect wash context for wash up ([`b82aadc`](https://github.com/connorsmith256/wasmcloud/commit/b82aadccb7b2a21fd704667c1f9d1767479ddbc0))
    - Refactor!(wash-cli): initialize contexts consistently ([`703283b`](https://github.com/connorsmith256/wasmcloud/commit/703283b144a97a7e41ef67cae242ae73d85067a9))
    - Remove `wasmcloud-test-util` dependency ([`bb4fbea`](https://github.com/connorsmith256/wasmcloud/commit/bb4fbeaa780552fa90e310773f53b16e83569438))
    - Add context to encoding errors ([`d734e98`](https://github.com/connorsmith256/wasmcloud/commit/d734e98529a5fe1c7f014b5b0c5aaf4c84af912a))
    - Remove `wasmbus_rpc` dependency ([`db99594`](https://github.com/connorsmith256/wasmcloud/commit/db99594fb6537d8f84a421edf153d9ca6bdbbeed))
    - Format rustup ([`4ef9921`](https://github.com/connorsmith256/wasmcloud/commit/4ef9921e2283e7fc43ea427b90f36fb874b0d32a))
    - Add instructions for setting up language toolchains ([`3d373ed`](https://github.com/connorsmith256/wasmcloud/commit/3d373ed3da71736ac82015a222c54c275733f6aa))
    - Update help text for keys gen ([`f6814b9`](https://github.com/connorsmith256/wasmcloud/commit/f6814b9c82fe0a7d71aaccf5f379e5362622f9bf))
    - More refactoring... ([`7d6155e`](https://github.com/connorsmith256/wasmcloud/commit/7d6155e62512e6909379bbed5e73abe219838e4b))
    - Moving things around, better scopring for lattice_prefix parsing on app cmds ([`9bf9acc`](https://github.com/connorsmith256/wasmcloud/commit/9bf9accbcefa3e852c3b62290c14ee5e71731530))
    - Proper derivation of lattice_prefix (ie, lattice_prefix arg > context arg > $current_default context.lattice_prefix) ([`70ac131`](https://github.com/connorsmith256/wasmcloud/commit/70ac131767572f757fca6c37cdc428f40212bc6f))
</details>

## v0.22.0 (2023-11-04)

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

 - 26 commits contributed to the release over the course of 4 calendar days.
 - 25 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Fix import order ([`3ebfdd2`](https://github.com/connorsmith256/wasmcloud/commit/3ebfdd25b43c09a8117158d1d1aaaf0e5431746e))
    - Allow specifying washboard version ([`041525d`](https://github.com/connorsmith256/wasmcloud/commit/041525dcca71bb437963adb4f6944066c1a26f76))
    - Download washboard assets from releases instead of embedding from source ([`11eaf81`](https://github.com/connorsmith256/wasmcloud/commit/11eaf81137d476769312bf70589d2734f923887d))
    - Remove vestigial integration tests assertions for wash claims ([`dadfacb`](https://github.com/connorsmith256/wasmcloud/commit/dadfacb6541eec6e6a440bad99ffa66ea17a94a5))
    - Require revision and version args on sign cmd ([`4fb8118`](https://github.com/connorsmith256/wasmcloud/commit/4fb8118f8fd74a4baf8019f3ab6c6cea2fd1c889))
    - Wash-cli-v0.22.0 ([`a8e085e`](https://github.com/connorsmith256/wasmcloud/commit/a8e085e4eb46a635c9efd02a864584079b0eca4e))
    - Move washboard to its own directory ([`b936abf`](https://github.com/connorsmith256/wasmcloud/commit/b936abf2812b191ece6a01a65a090081c69d2013))
    - Cleaner pattern matching on keytype arg for wash keys gen cmd. ([`62f30c7`](https://github.com/connorsmith256/wasmcloud/commit/62f30c7bd3e591bb08d1583498aaba8b0483859d))
    - Stricter args parsing for wash keys gen cmd ([`4004c41`](https://github.com/connorsmith256/wasmcloud/commit/4004c41fb42a0bfe62b1521bcfa3ceaadd2a9caf))
    - Support domain, links, keys alias ([`a1c3b9d`](https://github.com/connorsmith256/wasmcloud/commit/a1c3b9d86db14f31ef7fbebeb30e8784f974df6f))
    - Update control interface 0.31 ([`007660e`](https://github.com/connorsmith256/wasmcloud/commit/007660e96ad7472918bc25baf9d52d60e5230823))
    - Update ctl to 0.31.0 ([`a1e8d3f`](https://github.com/connorsmith256/wasmcloud/commit/a1e8d3f09e039723d28d738d98b47bce54e4450d))
    - Add status indicator ([`9ffcc1b`](https://github.com/connorsmith256/wasmcloud/commit/9ffcc1b7494ced74e4a12094bd9b4ef782b6a83f))
    - Resubscribing when lattice connection change ([`544fa7e`](https://github.com/connorsmith256/wasmcloud/commit/544fa7e4c117512e613de15626e05853f1244f6b))
    - Bump lucide-react in /crates/wash-cli/washboard ([`e28c1ac`](https://github.com/connorsmith256/wasmcloud/commit/e28c1ac58a8cd6a1ec745f0de18d0776ec4e064e))
    - Bump @vitejs/plugin-react-swc ([`3f05d24`](https://github.com/connorsmith256/wasmcloud/commit/3f05d242dde36ce33e3ee87ba5b3c62c37c30d63))
    - Bump tailwind-merge in /crates/wash-cli/washboard ([`18ed181`](https://github.com/connorsmith256/wasmcloud/commit/18ed1810f8b8e0517b02ec7139a6c55742296d87))
    - Bump eslint-plugin-unicorn ([`82e8bc2`](https://github.com/connorsmith256/wasmcloud/commit/82e8bc2e8c2cd6ddcd88232c503241c024dc1ec1))
    - Bump eslint-plugin-react-refresh ([`c5845c0`](https://github.com/connorsmith256/wasmcloud/commit/c5845c0aed2d12174986f6cfa875f89704cb04d7))
    - Merge pull request #807 from rvolosatovs/merge/wash ([`f2bc010`](https://github.com/connorsmith256/wasmcloud/commit/f2bc010110d96fc21bc3502798543b7d5b68b1b5))
    - Move nextest config to root ([`6343ebf`](https://github.com/connorsmith256/wasmcloud/commit/6343ebfdf155cbfb3b70b1f2cbdcf38651946010))
    - Revert to upstream `wash` dev doc ([`413e395`](https://github.com/connorsmith256/wasmcloud/commit/413e395b60d3ee0c187ec398a2cb6429fd27d009))
    - Move completion doc to `wash-cli` crate ([`3d47e91`](https://github.com/connorsmith256/wasmcloud/commit/3d47e91e7a836ff04fd7bc809a036fadc42c01a7))
    - Reorder target-specific dep ([`d1ee13e`](https://github.com/connorsmith256/wasmcloud/commit/d1ee13ed7c1668b55f4644b1c1673f521ba9d9f8))
    - Build for Windows msvc ([`abc0750`](https://github.com/connorsmith256/wasmcloud/commit/abc075095e5df96e0b3c155bf1afb8dbeea4a6e5))
    - Integrate `wash` into the workspace ([`dfad0be`](https://github.com/connorsmith256/wasmcloud/commit/dfad0be609868cbd0f0ce97d7d9238b41996b5fc))
</details>


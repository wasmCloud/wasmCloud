# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.33.0 (2023-12-29)

<csr-id-39d4de57e25af8cb4686d53410037c1cc93027ba/>
<csr-id-5301084bde0db0c65811aa30c48de2a63e091fcf/>
<csr-id-f43d88283ddc17ed81b1f95bf64b5985bda70fd3/>
<csr-id-723ae50ea0eff41875f65622ba72cf2c4f53489f/>
<csr-id-18791e7666b4de2526628e2a973c47b7f51d9481/>
<csr-id-84fc7a928697c8fc9c6a03e94ed2053783577a4f/>
<csr-id-a61723a12a298f10e28eb7464a2bb623b5cfe244/>
<csr-id-17db669d79e242144eeffbd8d2ac2b1ae9edeb35/>
<csr-id-7de31820034c4b70ab6edc772713e64aafe294a9/>
<csr-id-65d2e28d54929b8f4d0b39077ee82ddad2387c8e/>
<csr-id-57d014fb7fe11542d2e64068ba86e42a19f64f98/>
<csr-id-4e9bae34fe95ecaffbc81fd452bf29746b4e5856/>
<csr-id-bdb72eed8778a5d8c59d0b8939f147c374cb671f/>
<csr-id-d3e6269dc1441b21d4c06d7620e9e7c6d839e211/>
<csr-id-413410bad26d148aeda28b6403add7842570efac/>
<csr-id-f8846e022a49d4c9158250af1ab9ae6661bceaf0/>
<csr-id-79a8f1b03a63a4b5a5295cdf86ef69780bade052/>
<csr-id-b604a8c7a5f1c9d3b417a178d68d90104d817b3a/>
<csr-id-98a59529e451214d61acdffe4703552a5f4a231a/>
<csr-id-f8c2d51f1b049e2035ea0d5df096a129482da7e4/>
<csr-id-ae3c37c61b20c38abbf8e09b37c546dd1db4db42/>
<csr-id-859b0baeff818a1af7e1824cbb80510669bdc976/>

### Chore

 - <csr-id-39d4de57e25af8cb4686d53410037c1cc93027ba/> bump to 0.32.1
 - <csr-id-5301084bde0db0c65811aa30c48de2a63e091fcf/> remove support for bindle references
 - <csr-id-f43d88283ddc17ed81b1f95bf64b5985bda70fd3/> fix lint
   This commit fixes a couple small lints that were left in the
   wasmcloud-control-interface crate
 - <csr-id-723ae50ea0eff41875f65622ba72cf2c4f53489f/> address clippy warnings
 - <csr-id-18791e7666b4de2526628e2a973c47b7f51d9481/> integrate `control-interface` into the workspace
 - <csr-id-84fc7a928697c8fc9c6a03e94ed2053783577a4f/> add 'crates/control-interface/' from commit 'cea335729f3bf368178cc6b8745478bdd01c54b5'

### Chore

 - <csr-id-90d7c48a46e112ab884d9836bfc25c1de5570fee/> add changelogs for wash

### Chore

 - <csr-id-859b0baeff818a1af7e1824cbb80510669bdc976/> add changelogs for host

### New Features

 - <csr-id-6994a2202f856da93d0fe50e40c8e72dd3b7d9e6/> add event name as suffix on event topic
 - <csr-id-85cb573d29c75eae4fdaca14be808131383ca3cd/> enable updating host labels via the control interface
 - <csr-id-1a048a71320dbbf58f331e7e958f4b1cd5ed4537/> Adds support for actor config
   This is a fairly large PR because it is adding several new control interface
   topics as well as actually adding the actor config feature.
   
   This feature was motivated by 2 major reasons:
   
   1. We have been needing something like this for a while, at the very least for
   being able to configure link names in an actor at runtime
2. There aren't currently any active (yes there were some in the past) efforts
      to add a generic `wasi:cloud/guest-config` interface that can allow any host
      to provide config values to a component. I want to use this as a springboard
      for the conversation in wasi-cloud as we will start to use it and can give
      active feedback as to how the interface should be shaped
 - <csr-id-1499ab7114e0425e5260f26b68a569532fbf02b3/> a new feature

### Other

 - <csr-id-a61723a12a298f10e28eb7464a2bb623b5cfe244/> v0.32.0
 - <csr-id-17db669d79e242144eeffbd8d2ac2b1ae9edeb35/> update `wasmcloud-control-interface`

### Refactor

 - <csr-id-7de31820034c4b70ab6edc772713e64aafe294a9/> remove deprecated code related to start actor cmd
 - <csr-id-65d2e28d54929b8f4d0b39077ee82ddad2387c8e/> update parsing from RegistryCredential to RegistryAuth
 - <csr-id-57d014fb7fe11542d2e64068ba86e42a19f64f98/> revised implementation of registry url and credentials resolution
 - <csr-id-4e9bae34fe95ecaffbc81fd452bf29746b4e5856/> some cleanup before revised implementation
 - <csr-id-bdb72eed8778a5d8c59d0b8939f147c374cb671f/> rename label to key
 - <csr-id-d3e6269dc1441b21d4c06d7620e9e7c6d839e211/> reverting back to simple util method call for identifier verification
 - <csr-id-413410bad26d148aeda28b6403add7842570efac/> trying out "nominal typing" for validating identifiers. Only HostId implemented.
 - <csr-id-f8846e022a49d4c9158250af1ab9ae6661bceaf0/> validate identifier inputs
 - <csr-id-79a8f1b03a63a4b5a5295cdf86ef69780bade052/> simplify `collect_sub_timeout`
 - <csr-id-b604a8c7a5f1c9d3b417a178d68d90104d817b3a/> remove `sub_stream` module
 - <csr-id-98a59529e451214d61acdffe4703552a5f4a231a/> remove unused `HeaderInjector`
 - <csr-id-f8c2d51f1b049e2035ea0d5df096a129482da7e4/> clean-up imports

### Test

 - <csr-id-ae3c37c61b20c38abbf8e09b37c546dd1db4db42/> remove initial test function.

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 28 commits contributed to the release over the course of 53 calendar days.
 - 974 days passed between releases.
 - 27 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - A new feature ([`1499ab7`](https://github.com/connorsmith256/wasmcloud/commit/1499ab7114e0425e5260f26b68a569532fbf02b3))
    - Add changelogs for wash ([`90d7c48`](https://github.com/connorsmith256/wasmcloud/commit/90d7c48a46e112ab884d9836bfc25c1de5570fee))
    - Add changelogs for host ([`859b0ba`](https://github.com/connorsmith256/wasmcloud/commit/859b0baeff818a1af7e1824cbb80510669bdc976))
    - Bump to 0.32.1 ([`39d4de5`](https://github.com/connorsmith256/wasmcloud/commit/39d4de57e25af8cb4686d53410037c1cc93027ba))
    - Remove deprecated code related to start actor cmd ([`7de3182`](https://github.com/connorsmith256/wasmcloud/commit/7de31820034c4b70ab6edc772713e64aafe294a9))
    - Update parsing from RegistryCredential to RegistryAuth ([`65d2e28`](https://github.com/connorsmith256/wasmcloud/commit/65d2e28d54929b8f4d0b39077ee82ddad2387c8e))
    - Revised implementation of registry url and credentials resolution ([`57d014f`](https://github.com/connorsmith256/wasmcloud/commit/57d014fb7fe11542d2e64068ba86e42a19f64f98))
    - Some cleanup before revised implementation ([`4e9bae3`](https://github.com/connorsmith256/wasmcloud/commit/4e9bae34fe95ecaffbc81fd452bf29746b4e5856))
    - Add event name as suffix on event topic ([`6994a22`](https://github.com/connorsmith256/wasmcloud/commit/6994a2202f856da93d0fe50e40c8e72dd3b7d9e6))
    - Rename label to key ([`bdb72ee`](https://github.com/connorsmith256/wasmcloud/commit/bdb72eed8778a5d8c59d0b8939f147c374cb671f))
    - Enable updating host labels via the control interface ([`85cb573`](https://github.com/connorsmith256/wasmcloud/commit/85cb573d29c75eae4fdaca14be808131383ca3cd))
    - Adds support for actor config ([`1a048a7`](https://github.com/connorsmith256/wasmcloud/commit/1a048a71320dbbf58f331e7e958f4b1cd5ed4537))
    - V0.32.0 ([`a61723a`](https://github.com/connorsmith256/wasmcloud/commit/a61723a12a298f10e28eb7464a2bb623b5cfe244))
    - Update `wasmcloud-control-interface` ([`17db669`](https://github.com/connorsmith256/wasmcloud/commit/17db669d79e242144eeffbd8d2ac2b1ae9edeb35))
    - Reverting back to simple util method call for identifier verification ([`d3e6269`](https://github.com/connorsmith256/wasmcloud/commit/d3e6269dc1441b21d4c06d7620e9e7c6d839e211))
    - Remove initial test function. ([`ae3c37c`](https://github.com/connorsmith256/wasmcloud/commit/ae3c37c61b20c38abbf8e09b37c546dd1db4db42))
    - Trying out "nominal typing" for validating identifiers. Only HostId implemented. ([`413410b`](https://github.com/connorsmith256/wasmcloud/commit/413410bad26d148aeda28b6403add7842570efac))
    - Validate identifier inputs ([`f8846e0`](https://github.com/connorsmith256/wasmcloud/commit/f8846e022a49d4c9158250af1ab9ae6661bceaf0))
    - Remove support for bindle references ([`5301084`](https://github.com/connorsmith256/wasmcloud/commit/5301084bde0db0c65811aa30c48de2a63e091fcf))
    - Fix lint ([`f43d882`](https://github.com/connorsmith256/wasmcloud/commit/f43d88283ddc17ed81b1f95bf64b5985bda70fd3))
    - Simplify `collect_sub_timeout` ([`79a8f1b`](https://github.com/connorsmith256/wasmcloud/commit/79a8f1b03a63a4b5a5295cdf86ef69780bade052))
    - Remove `sub_stream` module ([`b604a8c`](https://github.com/connorsmith256/wasmcloud/commit/b604a8c7a5f1c9d3b417a178d68d90104d817b3a))
    - Address clippy warnings ([`723ae50`](https://github.com/connorsmith256/wasmcloud/commit/723ae50ea0eff41875f65622ba72cf2c4f53489f))
    - Remove unused `HeaderInjector` ([`98a5952`](https://github.com/connorsmith256/wasmcloud/commit/98a59529e451214d61acdffe4703552a5f4a231a))
    - Clean-up imports ([`f8c2d51`](https://github.com/connorsmith256/wasmcloud/commit/f8c2d51f1b049e2035ea0d5df096a129482da7e4))
    - Merge pull request #927 from rvolosatovs/merge/control-interface ([`5d40fcb`](https://github.com/connorsmith256/wasmcloud/commit/5d40fcb06f4a029cca05f0d5b5f8c12722553822))
    - Integrate `control-interface` into the workspace ([`18791e7`](https://github.com/connorsmith256/wasmcloud/commit/18791e7666b4de2526628e2a973c47b7f51d9481))
    - Add 'crates/control-interface/' from commit 'cea335729f3bf368178cc6b8745478bdd01c54b5' ([`84fc7a9`](https://github.com/connorsmith256/wasmcloud/commit/84fc7a928697c8fc9c6a03e94ed2053783577a4f))
</details>

## v0.3.1 (2021-04-29)

## v0.3.0 (2021-04-16)

## v0.2.1 (2021-03-26)

## v0.2.0 (2021-03-22)

## v0.1.0 (2021-02-16)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 14 commits contributed to the release over the course of 75 calendar days.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 9 unique issues were worked on: [#16](https://github.com/connorsmith256/wasmcloud/issues/16), [#25](https://github.com/connorsmith256/wasmcloud/issues/25), [#32](https://github.com/connorsmith256/wasmcloud/issues/32), [#37](https://github.com/connorsmith256/wasmcloud/issues/37), [#40](https://github.com/connorsmith256/wasmcloud/issues/40), [#49](https://github.com/connorsmith256/wasmcloud/issues/49), [#67](https://github.com/connorsmith256/wasmcloud/issues/67), [#81](https://github.com/connorsmith256/wasmcloud/issues/81), [#84](https://github.com/connorsmith256/wasmcloud/issues/84)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#16](https://github.com/connorsmith256/wasmcloud/issues/16)**
    - Safety/WIP checkin. Partially done implementing control interface ([`308bbe4`](https://github.com/connorsmith256/wasmcloud/commit/308bbe4605f2f21359a7eed8518a8fe844a4f149))
 * **[#25](https://github.com/connorsmith256/wasmcloud/issues/25)**
    - Implementation of the scheduling auction client and host functionality ([`617555f`](https://github.com/connorsmith256/wasmcloud/commit/617555f8412b5485c266585c031a9a4332eb70af))
 * **[#32](https://github.com/connorsmith256/wasmcloud/issues/32)**
    - Initial implementation of actor update functionality ([`2390d79`](https://github.com/connorsmith256/wasmcloud/commit/2390d79063f3df58a8f358010839462a3f8e77a1))
 * **[#37](https://github.com/connorsmith256/wasmcloud/issues/37)**
    - Adding support for RPC invocations over control interface ([`f411d06`](https://github.com/connorsmith256/wasmcloud/commit/f411d06fb5cadda900928e620407538213aa5c2c))
 * **[#40](https://github.com/connorsmith256/wasmcloud/issues/40)**
    - Fixing topic prefixes ([`e968243`](https://github.com/connorsmith256/wasmcloud/commit/e968243a954806086f4cf2ae6f586a2a8207422e))
 * **[#49](https://github.com/connorsmith256/wasmcloud/issues/49)**
    - Convert lattice cache (networked and offline) into use of capability provider ([`6b47d05`](https://github.com/connorsmith256/wasmcloud/commit/6b47d050fd85883758518ff069a74b6eca5627a9))
 * **[#67](https://github.com/connorsmith256/wasmcloud/issues/67)**
    - Control interface start actor and provider now acknowledge prior to downloading OCI bytes ([`e6c228e`](https://github.com/connorsmith256/wasmcloud/commit/e6c228e70f433a1d2e6049d702c1841652c449d2))
 * **[#81](https://github.com/connorsmith256/wasmcloud/issues/81)**
    - Remove git dependencies ([`3d3bd2d`](https://github.com/connorsmith256/wasmcloud/commit/3d3bd2d7542efe037e56b1ea98e401cae1252fd5))
 * **[#84](https://github.com/connorsmith256/wasmcloud/issues/84)**
    - Updated crate READMEs, additional build/release actions, increased echo delay ([`9643645`](https://github.com/connorsmith256/wasmcloud/commit/9643645b18bf4d1478e4cf7666e7c576d9ed5ce0))
 * **Uncategorized**
    - Merge pull request #39 from brooksmtownsend/pub-mod-but-not-too-pub ([`9ed226d`](https://github.com/connorsmith256/wasmcloud/commit/9ed226d3eece88def03af7c173e5777d6b9d4823))
    - Merge remote-tracking branch 'upstream/main' into release_gh ([`c02921b`](https://github.com/connorsmith256/wasmcloud/commit/c02921bf17cf14767894449df08b886aab2e9eed))
    - Make only Invocation and InvocationResponse public ([`0154b89`](https://github.com/connorsmith256/wasmcloud/commit/0154b8943795c7925b94bfa3a1aa4f4da1b75854))
    - Merge pull request #38 from brooksmtownsend/make-mod-pub ([`27232ff`](https://github.com/connorsmith256/wasmcloud/commit/27232ffacf3b5d277328c6821b3cd25479f8bda9))
    - Make inv mod public for control_interface imports ([`92c80c1`](https://github.com/connorsmith256/wasmcloud/commit/92c80c11d35421d6ed3899d0e32651f177697772))
</details>


# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

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

 - 26 commits contributed to the release over the course of 53 calendar days.
 - 974 days passed between releases.
 - 25 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Add changelogs for host (859b0ba)
    - Bump to 0.32.1 (39d4de5)
    - Remove deprecated code related to start actor cmd (7de3182)
    - Update parsing from RegistryCredential to RegistryAuth (65d2e28)
    - Revised implementation of registry url and credentials resolution (57d014f)
    - Some cleanup before revised implementation (4e9bae3)
    - Add event name as suffix on event topic (6994a22)
    - Rename label to key (bdb72ee)
    - Enable updating host labels via the control interface (85cb573)
    - Adds support for actor config (1a048a7)
    - V0.32.0 (a61723a)
    - Update `wasmcloud-control-interface` (17db669)
    - Reverting back to simple util method call for identifier verification (d3e6269)
    - Remove initial test function. (ae3c37c)
    - Trying out "nominal typing" for validating identifiers. Only HostId implemented. (413410b)
    - Validate identifier inputs (f8846e0)
    - Remove support for bindle references (5301084)
    - Fix lint (f43d882)
    - Simplify `collect_sub_timeout` (79a8f1b)
    - Remove `sub_stream` module (b604a8c)
    - Address clippy warnings (723ae50)
    - Remove unused `HeaderInjector` (98a5952)
    - Clean-up imports (f8c2d51)
    - Merge pull request #927 from rvolosatovs/merge/control-interface (5d40fcb)
    - Integrate `control-interface` into the workspace (18791e7)
    - Add 'crates/control-interface/' from commit 'cea335729f3bf368178cc6b8745478bdd01c54b5' (84fc7a9)
</details>

<csr-unknown>
With that said, note that this is only going to be added for actors built againstthe component model. Since this is net new functionality, I didn’t think it wasworth it to try to backport.As for testing, I have tested that an actor can import the functions and get the valuesvia the various e2e tests and also manually validated that all of the new topicswork.<csr-unknown/>

## v0.3.1 (2021-04-29)

## v0.3.0 (2021-04-16)

## v0.2.1 (2021-03-26)

## v0.2.0 (2021-03-22)

## v0.1.0 (2021-02-16)

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 14 commits contributed to the release over the course of 75 calendar days.
 - 0 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 9 unique issues were worked on: #16, #25, #32, #37, #40, #49, #67, #81, #84

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **#16**
    - Safety/WIP checkin. Partially done implementing control interface (308bbe4)
 * **#25**
    - Implementation of the scheduling auction client and host functionality (617555f)
 * **#32**
    - Initial implementation of actor update functionality (2390d79)
 * **#37**
    - Adding support for RPC invocations over control interface (f411d06)
 * **#40**
    - Fixing topic prefixes (e968243)
 * **#49**
    - Convert lattice cache (networked and offline) into use of capability provider (6b47d05)
 * **#67**
    - Control interface start actor and provider now acknowledge prior to downloading OCI bytes (e6c228e)
 * **#81**
    - Remove git dependencies (3d3bd2d)
 * **#84**
    - Updated crate READMEs, additional build/release actions, increased echo delay (9643645)
 * **Uncategorized**
    - Merge pull request #39 from brooksmtownsend/pub-mod-but-not-too-pub (9ed226d)
    - Merge remote-tracking branch 'upstream/main' into release_gh (c02921b)
    - Make only Invocation and InvocationResponse public (0154b89)
    - Merge pull request #38 from brooksmtownsend/make-mod-pub (27232ff)
    - Make inv mod public for control_interface imports (92c80c1)
</details>


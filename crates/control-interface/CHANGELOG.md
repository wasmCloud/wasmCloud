# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-4a51868f45b6bff8472b1e9337ca83243ee102e8/>
<csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/>
<csr-id-0c193ff7cdf626b1ad8da11f933456d21be21246/>
<csr-id-74eb7525a8b0a32d5dfaeb16d347ef3a0ec48b7c/>
<csr-id-647a6358ffd0355bf00fb53aef18d937ee0e5324/>
<csr-id-39d4de57e25af8cb4686d53410037c1cc93027ba/>
<csr-id-5301084bde0db0c65811aa30c48de2a63e091fcf/>
<csr-id-f43d88283ddc17ed81b1f95bf64b5985bda70fd3/>
<csr-id-723ae50ea0eff41875f65622ba72cf2c4f53489f/>
<csr-id-18791e7666b4de2526628e2a973c47b7f51d9481/>
<csr-id-84fc7a928697c8fc9c6a03e94ed2053783577a4f/>
<csr-id-6417be87afb6df3e14892022148f38815056104c/>
<csr-id-a61723a12a298f10e28eb7464a2bb623b5cfe244/>
<csr-id-17db669d79e242144eeffbd8d2ac2b1ae9edeb35/>
<csr-id-c654448653db224c6a676ecf43150d880a9daf8c/>
<csr-id-c49a6ef0b6460b3eb463315fe31878eb71ae5364/>
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
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-d4bf78a704affaa84808fb167d3ab1636ffc35ac/>
<csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/>

### Chore

 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings
 - <csr-id-4a51868f45b6bff8472b1e9337ca83243ee102e8/> bump to v1.0.0
 - <csr-id-fd69df40f24ca565ace0f8c97a0c47a89db575a4/> Excises vestigal remains of wasmbus-rpc
   There were some parts of the core crate that we no longer use,
   especially now that we don't require claims signing anymore. This
   removes them and bumps the core crate in preparation for 1.0
 - <csr-id-0c193ff7cdf626b1ad8da11f933456d21be21246/> bump v1.0.0-alpha.3
 - <csr-id-74eb7525a8b0a32d5dfaeb16d347ef3a0ec48b7c/> bump to v1.0.0-alpha.2
 - <csr-id-647a6358ffd0355bf00fb53aef18d937ee0e5324/> bump to 1.0.0-alpha.1
 - <csr-id-39d4de57e25af8cb4686d53410037c1cc93027ba/> bump to 0.32.1
 - <csr-id-5301084bde0db0c65811aa30c48de2a63e091fcf/> remove support for bindle references
 - <csr-id-f43d88283ddc17ed81b1f95bf64b5985bda70fd3/> fix lint
   This commit fixes a couple small lints that were left in the
   wasmcloud-control-interface crate
 - <csr-id-723ae50ea0eff41875f65622ba72cf2c4f53489f/> address clippy warnings
 - <csr-id-18791e7666b4de2526628e2a973c47b7f51d9481/> integrate `control-interface` into the workspace
 - <csr-id-84fc7a928697c8fc9c6a03e94ed2053783577a4f/> add 'crates/control-interface/' from commit 'cea335729f3bf368178cc6b8745478bdd01c54b5'

### Documentation

 - <csr-id-7d0a9774b24c182b0e38ecaa0c1c4383c517af45/> indicate get_response usage
 - <csr-id-05ac449d3da207fd495ecbd786220b053fd6300e/> actor to components terminology
   This change only updates documentation terminology
   to use components instead of actors.
   
   Examples will use the terminology components as well so
   I'm opting to rename the example directories now ahead
   of any source code changes for actor to component
   renames.

### New Features

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
 - <csr-id-82c249b15dba4dbe4c14a6afd2b52c7d3dc99985/> Glues in named config to actors
   This introduces a new config bundle that can watch for config changes. There
   is probably a way to reduce the number of allocations here, but it is good
   enough for now.
   
   Also, sorry for the new file. I renamed `config.rs` to `host_config.rs` so
   I could reuse the `config.rs` file, but I forgot to git mv. So that file
   hasn't changed
 - <csr-id-4803b7f2381b5439f862746407ac13a31ebdfee3/> add wasmcloud-test-util crate
   This commit adds a `wasmcloud-test-util` crate, which contains utilities
   for testing wasmCloud hosts, providers, and actors locally
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
 - <csr-id-85a550d889d18ce4e437f88cbd8b3d127a9e5fbe/> added InterfaceLinkDefinition
 - <csr-id-3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3/> enable OTEL logs
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
 - <csr-id-cda9f724d2d2e4ea55006a43b166d18875148c48/> generate crate changelogs

### Bug Fixes

 - <csr-id-cab6fd2cae47f0a866f17dfdb593a48a9210bab8/> flatten claims response payload
 - <csr-id-215b492a1297fd35577e428dee25c1407ef8e6e2/> fix compilation

### Other

 - <csr-id-6417be87afb6df3e14892022148f38815056104c/> v0.33.0
 - <csr-id-a61723a12a298f10e28eb7464a2bb623b5cfe244/> v0.32.0
 - <csr-id-17db669d79e242144eeffbd8d2ac2b1ae9edeb35/> update `wasmcloud-control-interface`

### Refactor

 - <csr-id-c654448653db224c6a676ecf43150d880a9daf8c/> move wasmcloud wrpc transport client to core
   This commit moves the wasmcloud-specific wrpc transport client to the
   `wasmcloud-core` crate. From here, it can be used by both
   the host (`wasmbus`) and other places like tests.
 - <csr-id-c49a6ef0b6460b3eb463315fe31878eb71ae5364/> InterfaceLinkDefinition -> core
   This commit refators the types defined in both `wasmcloud-core` and
   `wasmcloud-control-interface` to make it easier to distinguish what
   types belong where and what they're related to.
   
   Ultimately the goal here was was to move `InterfaceLinkDefinition`
   into `wasmcloud-core` so it can be used in other places, but it was a
   good chance to reorganize.
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

### Chore (BREAKING)

 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-d4bf78a704affaa84808fb167d3ab1636ffc35ac/> rename actor to component

### New Features (BREAKING)

 - <csr-id-9e23be23131bbcdad746f7e85d33d5812e5f2ff9/> rename actor_scale* events
 - <csr-id-3f2d2f44470d44809fb83de2fa34b29ad1e6cb30/> Adds version to control API
   This should be the final breaking change of the API and it will require
   a two phased rollout. I'll need to cut new core and host versions first
   and then update wash to use the new host for tests.
 - <csr-id-36c70a6e572eeefc4fd211baef3934c691af2679/> support static named config for providers
 - <csr-id-4c54a488f5ea4a7d5f6793db62c9e2b0fd6ddf3a/> wrap all operations in CtlResponse
 - <csr-id-e16da6614ad9ae28e8c3e6ac3ebb36faf12cb4d1/> remove collection type aliases
 - <csr-id-49aba5d593d1d2a5ef10c46bb412be434bcf7e49/> flatten instances on actor/providers
 - <csr-id-48fc893ba2de576511aeea98a3da4cc97024c53e/> fully support interface links, remove aliases
 - <csr-id-1d46c284e32d2623d0b105014ef0c2f6ebc7e079/> Changes config topic to be for named config
   This is the first in a set of changes to move over to named config. It is
   not technically complete as you essentially have to name your config the
   same as the actor ID. I did this purposefully so as to not have a PR of
   doom with all the changes. The next PR will be adding named config to the
   scale command, then support for named config and providers in another PR
   after that
 - <csr-id-42d069eee87d1b5befff1a95b49973064f1a1d1b/> Updates topics to the new standard
   This is a wide ranging PR that changes all the topics as described
   in #1108. This also involved removing the start and stop actor
   operations. While I was in different parts of the code I did some small
   "campfire rule" cleanups mostly of clippy lints and removal of
   clippy pedant mode.
 - <csr-id-1753549d210c41405c9f2758ec857adc1505b61a/> allow receiving specific events
 - <csr-id-2e8893af27700b86dbeb63e5e7fc4252ec6771e1/> add heartbeat fields to inventory
 - <csr-id-df01bbd89fd2b690c2d1bcfe68455fb827646a10/> remove singular actor events, add actor_scaled
 - <csr-id-5cca9ee0a88d63cb53e8d352c16a5d9d59966bc8/> upgrade max_instances to u32
 - <csr-id-d8eb9f3ee9df65e96d076a6ba11d2600d0513207/> rename max-concurrent to max-instances, simplify scale

### Bug Fixes (BREAKING)

 - <csr-id-301ba5aacadfe939db5717eb9cff47a31fffd116/> consistent link operations

### Refactor (BREAKING)

 - <csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/> rename lattice prefix to just lattice

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 64 commits contributed to the release over the course of 175 calendar days.
 - 1096 days passed between releases.
 - 62 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Generate crate changelogs ([`cda9f72`](https://github.com/wasmCloud/wasmCloud/commit/cda9f724d2d2e4ea55006a43b166d18875148c48))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
    - Bump to v1.0.0 ([`4a51868`](https://github.com/wasmCloud/wasmCloud/commit/4a51868f45b6bff8472b1e9337ca83243ee102e8))
    - Indicate get_response usage ([`7d0a977`](https://github.com/wasmCloud/wasmCloud/commit/7d0a9774b24c182b0e38ecaa0c1c4383c517af45))
    - Remove cluster_seed/cluster_issuers ([`bc5d296`](https://github.com/wasmCloud/wasmCloud/commit/bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f))
    - Excises vestigal remains of wasmbus-rpc ([`fd69df4`](https://github.com/wasmCloud/wasmCloud/commit/fd69df40f24ca565ace0f8c97a0c47a89db575a4))
    - Bump v1.0.0-alpha.3 ([`0c193ff`](https://github.com/wasmCloud/wasmCloud/commit/0c193ff7cdf626b1ad8da11f933456d21be21246))
    - Rename actor to component ([`d4bf78a`](https://github.com/wasmCloud/wasmCloud/commit/d4bf78a704affaa84808fb167d3ab1636ffc35ac))
    - Bump to v1.0.0-alpha.2 ([`74eb752`](https://github.com/wasmCloud/wasmCloud/commit/74eb7525a8b0a32d5dfaeb16d347ef3a0ec48b7c))
    - Rename actor_scale* events ([`9e23be2`](https://github.com/wasmCloud/wasmCloud/commit/9e23be23131bbcdad746f7e85d33d5812e5f2ff9))
    - Adds version to control API ([`3f2d2f4`](https://github.com/wasmCloud/wasmCloud/commit/3f2d2f44470d44809fb83de2fa34b29ad1e6cb30))
    - Bump to 1.0.0-alpha.1 ([`647a635`](https://github.com/wasmCloud/wasmCloud/commit/647a6358ffd0355bf00fb53aef18d937ee0e5324))
    - Flatten claims response payload ([`cab6fd2`](https://github.com/wasmCloud/wasmCloud/commit/cab6fd2cae47f0a866f17dfdb593a48a9210bab8))
    - Actor to components terminology ([`05ac449`](https://github.com/wasmCloud/wasmCloud/commit/05ac449d3da207fd495ecbd786220b053fd6300e))
    - Support static named config for providers ([`36c70a6`](https://github.com/wasmCloud/wasmCloud/commit/36c70a6e572eeefc4fd211baef3934c691af2679))
    - Move wasmcloud wrpc transport client to core ([`c654448`](https://github.com/wasmCloud/wasmCloud/commit/c654448653db224c6a676ecf43150d880a9daf8c))
    - Support pubsub on wRPC subjects ([`76c1ed7`](https://github.com/wasmCloud/wasmCloud/commit/76c1ed7b5c49152aabd83d27f0b8955d7f874864))
    - InterfaceLinkDefinition -> core ([`c49a6ef`](https://github.com/wasmCloud/wasmCloud/commit/c49a6ef0b6460b3eb463315fe31878eb71ae5364))
    - Glues in named config to actors ([`82c249b`](https://github.com/wasmCloud/wasmCloud/commit/82c249b15dba4dbe4c14a6afd2b52c7d3dc99985))
    - Wrap all operations in CtlResponse ([`4c54a48`](https://github.com/wasmCloud/wasmCloud/commit/4c54a488f5ea4a7d5f6793db62c9e2b0fd6ddf3a))
    - Consistent link operations ([`301ba5a`](https://github.com/wasmCloud/wasmCloud/commit/301ba5aacadfe939db5717eb9cff47a31fffd116))
    - Remove collection type aliases ([`e16da66`](https://github.com/wasmCloud/wasmCloud/commit/e16da6614ad9ae28e8c3e6ac3ebb36faf12cb4d1))
    - Flatten instances on actor/providers ([`49aba5d`](https://github.com/wasmCloud/wasmCloud/commit/49aba5d593d1d2a5ef10c46bb412be434bcf7e49))
    - Feat(control-interface)!: add component ID, remove unneeded parameters from payloads for wrpc ([`a9518e0`](https://github.com/wasmCloud/wasmCloud/commit/a9518e0b567685fa2c6bf0d9d8aca80498c79da9))
    - Fully support interface links, remove aliases ([`48fc893`](https://github.com/wasmCloud/wasmCloud/commit/48fc893ba2de576511aeea98a3da4cc97024c53e))
    - Add wasmcloud-test-util crate ([`4803b7f`](https://github.com/wasmCloud/wasmCloud/commit/4803b7f2381b5439f862746407ac13a31ebdfee3))
    - Change set-target to set-link-name ([`5d19ba1`](https://github.com/wasmCloud/wasmCloud/commit/5d19ba16a98dca9439628e8449309ccaa763ab10))
    - Added InterfaceLinkDefinition ([`85a550d`](https://github.com/wasmCloud/wasmCloud/commit/85a550d889d18ce4e437f88cbd8b3d127a9e5fbe))
    - Changes config topic to be for named config ([`1d46c28`](https://github.com/wasmCloud/wasmCloud/commit/1d46c284e32d2623d0b105014ef0c2f6ebc7e079))
    - Updates topics to the new standard ([`42d069e`](https://github.com/wasmCloud/wasmCloud/commit/42d069eee87d1b5befff1a95b49973064f1a1d1b))
    - Enable OTEL logs ([`3602bdf`](https://github.com/wasmCloud/wasmCloud/commit/3602bdf5345ec9a75e88c7ce1ab4599585bcc2d3))
    - V0.33.0 ([`6417be8`](https://github.com/wasmCloud/wasmCloud/commit/6417be87afb6df3e14892022148f38815056104c))
    - Fix compilation ([`215b492`](https://github.com/wasmCloud/wasmCloud/commit/215b492a1297fd35577e428dee25c1407ef8e6e2))
    - Allow receiving specific events ([`1753549`](https://github.com/wasmCloud/wasmCloud/commit/1753549d210c41405c9f2758ec857adc1505b61a))
    - Rename lattice prefix to just lattice ([`6e8faab`](https://github.com/wasmCloud/wasmCloud/commit/6e8faab6a6e9f9bb7327ffb71ded2a83718920f7))
    - Add heartbeat fields to inventory ([`2e8893a`](https://github.com/wasmCloud/wasmCloud/commit/2e8893af27700b86dbeb63e5e7fc4252ec6771e1))
    - Remove singular actor events, add actor_scaled ([`df01bbd`](https://github.com/wasmCloud/wasmCloud/commit/df01bbd89fd2b690c2d1bcfe68455fb827646a10))
    - Upgrade max_instances to u32 ([`5cca9ee`](https://github.com/wasmCloud/wasmCloud/commit/5cca9ee0a88d63cb53e8d352c16a5d9d59966bc8))
    - Rename max-concurrent to max-instances, simplify scale ([`d8eb9f3`](https://github.com/wasmCloud/wasmCloud/commit/d8eb9f3ee9df65e96d076a6ba11d2600d0513207))
    - Bump to 0.32.1 ([`39d4de5`](https://github.com/wasmCloud/wasmCloud/commit/39d4de57e25af8cb4686d53410037c1cc93027ba))
    - Remove deprecated code related to start actor cmd ([`7de3182`](https://github.com/wasmCloud/wasmCloud/commit/7de31820034c4b70ab6edc772713e64aafe294a9))
    - Update parsing from RegistryCredential to RegistryAuth ([`65d2e28`](https://github.com/wasmCloud/wasmCloud/commit/65d2e28d54929b8f4d0b39077ee82ddad2387c8e))
    - Revised implementation of registry url and credentials resolution ([`57d014f`](https://github.com/wasmCloud/wasmCloud/commit/57d014fb7fe11542d2e64068ba86e42a19f64f98))
    - Some cleanup before revised implementation ([`4e9bae3`](https://github.com/wasmCloud/wasmCloud/commit/4e9bae34fe95ecaffbc81fd452bf29746b4e5856))
    - Add event name as suffix on event topic ([`6994a22`](https://github.com/wasmCloud/wasmCloud/commit/6994a2202f856da93d0fe50e40c8e72dd3b7d9e6))
    - Rename label to key ([`bdb72ee`](https://github.com/wasmCloud/wasmCloud/commit/bdb72eed8778a5d8c59d0b8939f147c374cb671f))
    - Enable updating host labels via the control interface ([`85cb573`](https://github.com/wasmCloud/wasmCloud/commit/85cb573d29c75eae4fdaca14be808131383ca3cd))
    - Adds support for actor config ([`1a048a7`](https://github.com/wasmCloud/wasmCloud/commit/1a048a71320dbbf58f331e7e958f4b1cd5ed4537))
    - V0.32.0 ([`a61723a`](https://github.com/wasmCloud/wasmCloud/commit/a61723a12a298f10e28eb7464a2bb623b5cfe244))
    - Update `wasmcloud-control-interface` ([`17db669`](https://github.com/wasmCloud/wasmCloud/commit/17db669d79e242144eeffbd8d2ac2b1ae9edeb35))
    - Reverting back to simple util method call for identifier verification ([`d3e6269`](https://github.com/wasmCloud/wasmCloud/commit/d3e6269dc1441b21d4c06d7620e9e7c6d839e211))
    - Remove initial test function. ([`ae3c37c`](https://github.com/wasmCloud/wasmCloud/commit/ae3c37c61b20c38abbf8e09b37c546dd1db4db42))
    - Trying out "nominal typing" for validating identifiers. Only HostId implemented. ([`413410b`](https://github.com/wasmCloud/wasmCloud/commit/413410bad26d148aeda28b6403add7842570efac))
    - Validate identifier inputs ([`f8846e0`](https://github.com/wasmCloud/wasmCloud/commit/f8846e022a49d4c9158250af1ab9ae6661bceaf0))
    - Remove support for bindle references ([`5301084`](https://github.com/wasmCloud/wasmCloud/commit/5301084bde0db0c65811aa30c48de2a63e091fcf))
    - Fix lint ([`f43d882`](https://github.com/wasmCloud/wasmCloud/commit/f43d88283ddc17ed81b1f95bf64b5985bda70fd3))
    - Simplify `collect_sub_timeout` ([`79a8f1b`](https://github.com/wasmCloud/wasmCloud/commit/79a8f1b03a63a4b5a5295cdf86ef69780bade052))
    - Remove `sub_stream` module ([`b604a8c`](https://github.com/wasmCloud/wasmCloud/commit/b604a8c7a5f1c9d3b417a178d68d90104d817b3a))
    - Address clippy warnings ([`723ae50`](https://github.com/wasmCloud/wasmCloud/commit/723ae50ea0eff41875f65622ba72cf2c4f53489f))
    - Remove unused `HeaderInjector` ([`98a5952`](https://github.com/wasmCloud/wasmCloud/commit/98a59529e451214d61acdffe4703552a5f4a231a))
    - Clean-up imports ([`f8c2d51`](https://github.com/wasmCloud/wasmCloud/commit/f8c2d51f1b049e2035ea0d5df096a129482da7e4))
    - Merge pull request #927 from rvolosatovs/merge/control-interface ([`5d40fcb`](https://github.com/wasmCloud/wasmCloud/commit/5d40fcb06f4a029cca05f0d5b5f8c12722553822))
    - Integrate `control-interface` into the workspace ([`18791e7`](https://github.com/wasmCloud/wasmCloud/commit/18791e7666b4de2526628e2a973c47b7f51d9481))
    - Add 'crates/control-interface/' from commit 'cea335729f3bf368178cc6b8745478bdd01c54b5' ([`84fc7a9`](https://github.com/wasmCloud/wasmCloud/commit/84fc7a928697c8fc9c6a03e94ed2053783577a4f))
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
 - 9 unique issues were worked on: [#16](https://github.com/wasmCloud/wasmCloud/issues/16), [#25](https://github.com/wasmCloud/wasmCloud/issues/25), [#32](https://github.com/wasmCloud/wasmCloud/issues/32), [#37](https://github.com/wasmCloud/wasmCloud/issues/37), [#40](https://github.com/wasmCloud/wasmCloud/issues/40), [#49](https://github.com/wasmCloud/wasmCloud/issues/49), [#67](https://github.com/wasmCloud/wasmCloud/issues/67), [#81](https://github.com/wasmCloud/wasmCloud/issues/81), [#84](https://github.com/wasmCloud/wasmCloud/issues/84)

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **[#16](https://github.com/wasmCloud/wasmCloud/issues/16)**
    - Safety/WIP checkin. Partially done implementing control interface ([`308bbe4`](https://github.com/wasmCloud/wasmCloud/commit/308bbe4605f2f21359a7eed8518a8fe844a4f149))
 * **[#25](https://github.com/wasmCloud/wasmCloud/issues/25)**
    - Implementation of the scheduling auction client and host functionality ([`617555f`](https://github.com/wasmCloud/wasmCloud/commit/617555f8412b5485c266585c031a9a4332eb70af))
 * **[#32](https://github.com/wasmCloud/wasmCloud/issues/32)**
    - Initial implementation of actor update functionality ([`2390d79`](https://github.com/wasmCloud/wasmCloud/commit/2390d79063f3df58a8f358010839462a3f8e77a1))
 * **[#37](https://github.com/wasmCloud/wasmCloud/issues/37)**
    - Adding support for RPC invocations over control interface ([`f411d06`](https://github.com/wasmCloud/wasmCloud/commit/f411d06fb5cadda900928e620407538213aa5c2c))
 * **[#40](https://github.com/wasmCloud/wasmCloud/issues/40)**
    - Fixing topic prefixes ([`e968243`](https://github.com/wasmCloud/wasmCloud/commit/e968243a954806086f4cf2ae6f586a2a8207422e))
 * **[#49](https://github.com/wasmCloud/wasmCloud/issues/49)**
    - Convert lattice cache (networked and offline) into use of capability provider ([`6b47d05`](https://github.com/wasmCloud/wasmCloud/commit/6b47d050fd85883758518ff069a74b6eca5627a9))
 * **[#67](https://github.com/wasmCloud/wasmCloud/issues/67)**
    - Control interface start actor and provider now acknowledge prior to downloading OCI bytes ([`e6c228e`](https://github.com/wasmCloud/wasmCloud/commit/e6c228e70f433a1d2e6049d702c1841652c449d2))
 * **[#81](https://github.com/wasmCloud/wasmCloud/issues/81)**
    - Remove git dependencies ([`3d3bd2d`](https://github.com/wasmCloud/wasmCloud/commit/3d3bd2d7542efe037e56b1ea98e401cae1252fd5))
 * **[#84](https://github.com/wasmCloud/wasmCloud/issues/84)**
    - Updated crate READMEs, additional build/release actions, increased echo delay ([`9643645`](https://github.com/wasmCloud/wasmCloud/commit/9643645b18bf4d1478e4cf7666e7c576d9ed5ce0))
 * **Uncategorized**
    - Merge pull request #39 from brooksmtownsend/pub-mod-but-not-too-pub ([`9ed226d`](https://github.com/wasmCloud/wasmCloud/commit/9ed226d3eece88def03af7c173e5777d6b9d4823))
    - Merge remote-tracking branch 'upstream/main' into release_gh ([`c02921b`](https://github.com/wasmCloud/wasmCloud/commit/c02921bf17cf14767894449df08b886aab2e9eed))
    - Make only Invocation and InvocationResponse public ([`0154b89`](https://github.com/wasmCloud/wasmCloud/commit/0154b8943795c7925b94bfa3a1aa4f4da1b75854))
    - Merge pull request #38 from brooksmtownsend/make-mod-pub ([`27232ff`](https://github.com/wasmCloud/wasmCloud/commit/27232ffacf3b5d277328c6821b3cd25479f8bda9))
    - Make inv mod public for control_interface imports ([`92c80c1`](https://github.com/wasmCloud/wasmCloud/commit/92c80c11d35421d6ed3899d0e32651f177697772))
</details>


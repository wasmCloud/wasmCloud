# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.26.0 (2025-05-28)

### Chore

 - <csr-id-467e7ca03142ee926b5f4aa4cf6d8a0e3a69eb46/> Reference wasmcloud_core::rpc helper methods consistently for provider subjects
 - <csr-id-ee7d4bbca9b0d07847b0e08af6251c6b4ee5ea2f/> remove TODOs scattered in code
 - <csr-id-1b357dd07183ce673f9ac4af97aef40cb9c3cee1/> embed README, fix broken doc comment links
 - <csr-id-27efae8904182ab43e7a93afd52f56280d360bf7/> warn on large ctl respose payload
 - <csr-id-06ecdcae83ba1e5886a7b102a0534fcaf694e653/> warn on large event payload
 - <csr-id-68c048f8ac90efe33805fe019cdd90d43bd9b538/> Bump patch versions of tracing and host
 - <csr-id-0b034328ebbbccfb596f7a41d0d60412e491aa22/> Add tests for parse_selectors_from_host_labels
 - <csr-id-edb544ef42f24995f7a823cd209d299af409d46c/> Address feedback
 - <csr-id-670c43d1cdd9df97b3b765196734fec4f4b1d239/> Mark host-label parsing code as unix-only
 - <csr-id-ae33ae1d3ebdf44bef23a19362d89274f9d57212/> Rename workload-identity feature to workload-identity-auth to better represent its intended purpose
 - <csr-id-3078c88f0ebed96027e20997bccc1c125583fad4/> bump provider-archive v0.16.0, wasmcloud-core v0.17.0, wasmcloud-tracing v0.13.0, wasmcloud-provider-sdk v0.14.0, wasmcloud-provider-http-server v0.27.0, wasmcloud-provider-messaging-nats v0.26.0, wasmcloud-runtime v0.9.0, wasmcloud-secrets-types v0.6.0, wasmcloud-secrets-client v0.7.0, wasmcloud-host v0.25.0, wasmcloud-test-util v0.17.0, secrets-nats-kv v0.2.0, wash v0.41.0
 - <csr-id-5a97c3abc50e5289fb76af764f0d82983b4962df/> Match non-workload identity first
 - <csr-id-383fb22ede84002851081ba21f760b35cf9a2263/> Address feedback
 - <csr-id-015bb52602f68a76d4cea2e666d19a75df7d9aa8/> Update workload identity to be unix-only feature
 - <csr-id-9d9d1c52b260f8fa66140ca9951893b482363a8a/> Add workload identity integration test
 - <csr-id-d54eb0f035a4269ea163dbb5c3282a613f8e78e4/> Match method and variable naming from suggestions
 - <csr-id-ec331af4d90eb8d369a5f5de51afcf8a45f476a3/> Remove the use of channels for passing SVIDs around
 - <csr-id-3d0c7ea40fba0b7f93357a0ad060eb3594872843/> bump v0.24.1
 - <csr-id-f85c6be0e355cf0e8284865122d0be35a9de728c/> interpolate tracing logs for provider restarts
 - <csr-id-6659528a4531f8d8024785296a36874b7e409f31/> fix spelling
 - <csr-id-b46467dbd662e0c5e277fdb4349369a22b4f7f67/> Improve error message
 - <csr-id-dfd66aa879ef42b3a0cfc6c2242c60101122b76c/> improve flow
 - <csr-id-fd2572d205158fca69fa8afbe4b2f70f5e3651d5/> cargo fmt + clippy
 - <csr-id-4f30198215220b3f9ce0c2aa6da8aa7d31a6a72d/> bump wasmcloud-core v0.16.0, wash-lib v0.32.0, wash-cli v0.38.0, safety bump 6 crates
   SAFETY BUMP: wash-lib v0.32.0, wash-cli v0.38.0, wasmcloud-host v0.24.0, wasmcloud-provider-sdk v0.13.0, wasmcloud-test-util v0.16.0, wasmcloud-runtime v0.8.0
 - <csr-id-97c436019740568f22ad8e4ff633fcd3f70260dc/> upgrade opentelemetry libraries to 0.27
 - <csr-id-7cf33ece6eb6ff9fd46043402096e51f9ca1ac60/> ignore `content-type` values with a `warn` log
 - <csr-id-b7bdfed2f043c6fbcf17ef6588c6b099057014a2/> add `wasmcloud:messaging@0.3.0` dependency
 - <csr-id-eb52eca817fe24b33e7f1a65c1ba5c46c50bef4e/> removed unused dependencies
   A batch scanning all crates and remove unused dependencies by running 'cargo machete'.
 - <csr-id-6d250ffa473385baae59b6f83b35ff38f119c054/> update `wrpc-transport-nats`
 - <csr-id-68ea303e2cb3a3bbfd6878bcc1884f4951ed693d/> convert wasi-logging levels into more conventional format
 - <csr-id-c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33/> bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, wasmcloud-host v0.22.0, wasmcloud-runtime v0.6.0, wasmcloud-test-util v0.14.0
 - <csr-id-44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a/> bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates
   SAFETY BUMP: wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, wasmcloud-host v0.21.0, wasmcloud-runtime v0.5.0, wasmcloud-test-util v0.13.0
 - <csr-id-fbd1dd10a7c92a40a69c21b2cbba21c07ae8e893/> Switch to using oci feature
 - <csr-id-fa01304b62e349be3ac3cf00aa43c2f5ead93dd5/> fix clippy lints
 - <csr-id-d21d2a9e7dffd16315eeb565e2cd0e1f1aeeac6c/> set missing link log to warn
 - <csr-id-40e5edfc0ee48fadccd0f0fb8f8d0eb53db026f0/> Make wasmcloud host heartbeat interval configurable
 - <csr-id-51c8ceb895b0069af9671e895b9f1ecb841ea6c3/> update component/runtime/host crate READMEs
 - <csr-id-da461edd4e5ede0220cb9923b1d9a62808f560dc/> clarify missing secret config error
 - <csr-id-f36471d7620fd66ff642518ae96188fef6fde5e0/> fix clippy lint
 - <csr-id-da879d3e50d32fe1c09edcf2b58cb2db9c9e2661/> update secrets integration to use the update config structure
   Update the secrets integration in a wasmCloud host to include
   information about the policy that determines which backend to
   communicate with. This is a change that comes in from wadm where the
   policy block now contains the information about which backend to use.
   
   This also passes any propertes defined on the policy to the correct
   backend, which are stored as a versioned string-encoded JSON object.
 - <csr-id-81ab5914e7d08740eb9371c9b718f13f0419c23f/> enable `ring` feature for `async-nats`
 - <csr-id-bd50166619b8810ccdc2bcd80c33ff80d94bc909/> address clippy warnings
 - <csr-id-0f7093660a1ef09ff745daf5e1a96fd72c88984d/> update to stream-based serving
 - <csr-id-e7c30405302fcccc612209335179f0bc47d8e996/> improve error messages for missing links
   When known interfaces are accessed, we show a message that notes that
   the target is unknown, but we can improve on that by alerting the user
   to a possibly missing link.
 - <csr-id-20a72597d17db8fcf0c70a7e9172edadcaad5b22/> improve error messages for missing links
   When known interfaces are accessed, we show a message that notes that
   the target is unknown, but we can improve on that by alerting the user
   to a possibly missing link.
 - <csr-id-d9a8c62d6fce6e71edadcf7de78cac749cf58126/> downgrade link/claims log/trace
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-88c07bf3be18da4f4afac3e7e356ddc507a6d85e/> Bump oci-distribution to 0.11.0
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-e6dd0b2809510e785f4ee4c531f5666e6ab21998/> replace references to 'actor' with 'component'
 - <csr-id-bdb519f91125c3f32f60ad9e9d1ce7bc1f147dc4/> remove unnecessary todo comments
 - <csr-id-9f1b2787255cb106d98481019d26e3208c11fc9f/> show provider ID on healthcheck failure messages
 - <csr-id-863296d7db28ca4815820f8b9a96a63dfe626904/> improve error message for forceful provider shutdown
 - <csr-id-e1ab91d678d8191f28e2496a68e52c7b93ad90c3/> update URLs to `wrpc` org
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings
 - <csr-id-346753ab823f911b12de763225dfd154272f1d3a/> Bumps host version to rc.2
   While I was here, I fixed the issue where we were using the host crate
   version instead of the top level binary host version in our events and
   ctl API responses
 - <csr-id-e8aac21cbc094f87fb486a903eaab9a132a7ee07/> imrpove wording for spec/provider ref mismatch
   This commit slightly improves the wording when a provider ID and
   component specification URL mismatch occurs, along with specifying a
   possible solution.
   
   This error is thrown by `wash` and it's a bit difficult to figure out
   what to resolve it otherwise.
 - <csr-id-955a6893792e86292883e76de57434616c28d380/> update `messaging` to `0.2.0`
 - <csr-id-f2aed15288300989aca03f899b095d3a71f8e5cd/> remove compat crate
 - <csr-id-adb08b70ecc37ec14bb9b7eea41c8110696d9b98/> address clippy warnings
 - <csr-id-6b369d49cd37a87dca1f92f31c4d4d3e33dec501/> use `&str` directly
 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-95a9d7d3b8c6367df93b65a2e218315cc3ec42eb/> refactor component invocation tracking
 - <csr-id-67847106d968a515ff89427454b7b14dfb486a3d/> remove functionality related to wasmbus invocations
 - <csr-id-49d86501487f6811bb8b65641c40ab353f6e110d/> update wRPC
 - <csr-id-e12ec1d6655a9aa319236a8d62a53fd6521bd683/> revert incorrectly handled config conficts
 - <csr-id-9957ca7f8b21444b2d4e32f20a50b09f92a5b6ee/> remove plural actor events
 - <csr-id-4f55396a0340d65dbebdf6d4f0ca070d6f990fc4/> integrate set-link-name and wrpc
 - <csr-id-5990b00ea49b1bfeac3ee913dc0a9188729abeff/> remove unused imports/functions
 - <csr-id-1bda5cd0da34dcf2d2613fca13430fac2484b5d9/> remove unused function
 - <csr-id-a90b0eaccdeb095ef147bed58e262440fb5f8486/> reintroduce wasmbus over wrpc
 - <csr-id-50c82440b34932ed5c03cb24a45fbacfe0c3e4d3/> fix `wasmcloud-host` clippy warning
 - <csr-id-aa03d411b571e446a842fa0e6b506436e5a04e4c/> update version to 0.82.0
 - <csr-id-08b8a3c72902e6d8ff4f9dcaa95b9649f3716e75/> implement preview 2 interfaces
 - <csr-id-c038aa74a257664780719103c7362a747fc5a539/> bump wasmcloud to 0.81
 - <csr-id-9a086ec818dcb0292d332f606f49e04c503866b4/> use consistent message prefix
 - <csr-id-9f9ca40e7a4b1d2d553fabee8a8bfc3f49e85a3f/> address clippy issue
   This is caused by Rust update
 - <csr-id-c8240e200c5fab84cfc558efc6445ecc91a9fa24/> remove `local` host
 - <csr-id-5301084bde0db0c65811aa30c48de2a63e091fcf/> remove support for bindle references
 - <csr-id-2389f27f0b570164a895a37abd462be2d68f20be/> polish tracing and logging levels
 - <csr-id-2c778413dd347ade2ade472365545fc954da20d0/> disambiguate traced function names
 - <csr-id-d377cb4553519413e420f9a547fef7ecf2421591/> improve reference parsing
 - <csr-id-75c200da45e383d02b2557df0bc9db5edb5f9979/> add logs related to registry config
 - <csr-id-02ae07006c9b2bb7b58b79b9e581ba255027fc7d/> add some control interface logs
 - <csr-id-93c0981a4d69bc8f8fe06e6139e78e7f700a3115/> resolve 1.73.0 warnings
 - <csr-id-a4b284c182278542b25056f32c86480c490a67b4/> give NATS 2 secs to start in test
 - <csr-id-cd8f69e8d155f3e2aa5169344ff827e1f7d965cf/> rename SUCCESS to ACCEPTED, None concurrent max
 - <csr-id-8ffa1317b1f106d6dcd2ec01c41fa14e6e41966e/> drop logging level to trace
 - <csr-id-0023f7e86d5a40a534f623b7220743f27871549e/> reduce verbosity of instrumented functions
 - <csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/> satisfy clippy linting
 - <csr-id-5923e34245c498bd9e7206bbe4ac6690192c7c60/> emit more clear start message
 - <csr-id-90918988a075ea7c0a110cf5301ce917f5822c3b/> reduce noise from REFMAP entries
 - <csr-id-11c932b6838aa987eb0122bc50067cee3417025b/> reduce noise on instruments
 - <csr-id-4fb8206e1d5fb21892a01b9e4f009e48c8bea2df/> remove noisy fields from instruments
 - <csr-id-b77767e6d3c32ceba0b4e5b421b532ac0788dc15/> rename friendly noun
 - <csr-id-5cd8afe68e4c481dcf09c9bebb125a9e4667ed1e/> refactor connection opts
 - <csr-id-478f775eb79bc955af691a7b5c7911cc36e8c98f/> made fetch arg ordering consistent

### Documentation

 - <csr-id-7bf02ede2e92aed19bbf7ef5162e2a87dc8f5cb8/> add README for the host crate

### New Features

 - <csr-id-bf2d6e6033ca2fe631c0eab57d50c480787b693b/> Add names to nats connections
 - <csr-id-f9c1131f7aa06542ab23059a2bbeda9fe6a7cc12/> track max component instances
 - <csr-id-7e59529c7942a5d4616323bbf35970f0c3d8bea1/> track active component instances
 - <csr-id-a6d01eca1eebc2e18291d483e29452a73bd20f13/> support max core instances per component
 - <csr-id-f66049dc5072b92cc427111c3b1c0acb6b12aa0f/> add builtin http host routing
 - <csr-id-6061eabcd60c56de88ed275aa1b988afec9a426a/> add `wasmcloud:bus/error.error`
 - <csr-id-129fd42074ce46f260798840539a12c585fea893/> Adds support for resource attributes
   We weren't properly discovering resource attributes from the environment.
   This should fix the issue in both the host and provider. This also
   changes our start provider process to proxy through all OTEL env vars to
   the process
 - <csr-id-e9e32ced1cdc7d00e13bd268ddb78388fb03d1a0/> Add wasmcloud:identity interface for fetching component workload identity
 - <csr-id-441bdfb0ab40cb82a2a2922a094f9b1fd19923a5/> Add experimental support for workload identity
 - <csr-id-b1e656f8380ded7d7461fa01e403c3f145f79b54/> More host metrics & attributes
 - <csr-id-ca530c77e94d755f3e4dff9e7d486dd6b1f7c2e7/> add wasi:keyvalue/watch handler and redis provider
 - <csr-id-388c3f48e1141c2e927b4034943f72b63c9378ea/> refresh config on provider restart
 - <csr-id-a89b3c54ef01ea9756dc83b8a6bc178a035a37c3/> support pausing provider restarts during shutdown
 - <csr-id-bf82eb498ab5fee4e8dd0cec937facadf92558dc/> add http-server path integration test
 - <csr-id-a0e41b67f4aee71ccf14b31e222f0ec58196e2be/> implement builtin path based routing
 - <csr-id-b6eb0a8640c611948a051133370647c519748f64/> allow disabling auction participation support
 - <csr-id-febf47d0ed7d70a245006710960a1a3bd695e6bf/> gate messaging@v3 behind feature flag
 - <csr-id-85ede3c9de6259908ad36225099c78e15364fa6f/> implement health check API
 - <csr-id-2772b1b3025b579fa016b86c472330bc756706af/> add JetStream consumer support
 - <csr-id-8afa8ff95db21c93d3b280edc39d477dc8a2aaac/> implement `wasmcloud:messaging@0.3.0`
 - <csr-id-0edd6d739e97141cd5a7efda2809d9be5341111d/> gate builtins with feature flags
 - <csr-id-2aa024056642683fbb6c3d93a87bb3e680b3237e/> trace builtin providers
 - <csr-id-d1feac0b8abb8cb8e2eec665b4d9b38ec8cd6d7f/> propagate outgoing context via store not handler
 - <csr-id-d70329d15042da16d9cfa8ae2bdcea76ded2784e/> account for `max_instances` in builtin providers
 - <csr-id-5200f4d86cd6f58f7c571765555add944fe8ecd0/> handle incoming NATS messages concurrently
 - <csr-id-13a4de8e5fe7d6b3641269e10a9aae2e8269164f/> add metrics to builtin providers
 - <csr-id-45fa6b06216da820045e4d39838a6ac3bdf97075/> add builtin `wasmcloud:messaging`, HTTP server
 - <csr-id-f051d9d3b8afb0ea1baafd87941babb29830fe4d/> Adds the ability to configure sampling and buffer size
   This adds the ability for provider to configure some of the tunables for
   tracing via config. The underlying SDK defaults (and hence, the host) use
   the standard environment variables
   
   Please note that some of our traces are using a span that lives for the
   lifetime of the application, so the sampling percentage may not work
   until we fix that as well
 - <csr-id-f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2/> Updates tests and examples to support the new wkg deps
   This updates all dependencies to have a wkg.lock but I didn't add to the
   gitignore for convenience. The deps are still committed in tree for backwards
   compatibility and they all use the new versioned logging. This looks
   really chunky bust is mostly dep updates/deletes
 - <csr-id-a7015f83af4d8d46284d3f49398ffa22876f290d/> add lattice@1.0.0 and @2.0.0 implementations
 - <csr-id-b8c2b999c979cd82f6772f3f98ec1d16c7f5565d/> add support for WASI 0.2.2
 - <csr-id-8575f732df33ca973ff340fc3e4bc7fbfeaf89f3/> Adds support for batch support to the host
   This enables keyvalue batch support inside of the host, along with a test
   to make sure it works. Not all of our providers implement batch yet, so
   this uses the Redis provider, which did have implementions. I did have to
   fix the redis provider to get the right type of data back and transform
   it. I also had to update our wRPC versions so we could pick up on some
   bug fixes for the types we are encoding in the batch interface.
 - <csr-id-26d7f64659dbf3263f36da92df89003c579077cc/> fallback to `wrpc:blobstore@0.1.0`
 - <csr-id-61641322dec02dd835e81b51de72cbd1007d13cf/> support for sending out config updates to providers
 - <csr-id-a570a3565e129fc13b437327eb1ba18835c69f57/> add Host level configurability for max_execution_time by flag and env variables
   - Introduce humantime::Duration for capturing human readable input time.
   - Add the `--max-execution-time` flag (alias: --max-time) to wasmcloud binary and wash up command, allowing for configuration of the max execution time for the Host runtime.
   - Set Default to 10min and Time format to Milliseconds.
 - <csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/> Add support for supplying additional CA certificates to OCI and OpenTelemetry clients
 - <csr-id-e0324d66e49be015b7b231626bc3b619d9374c91/> fetch secrets for providers and links
 - <csr-id-773780c59dc9af93b51abdf90a4f948ff2efb326/> add secrets handler impl for strings
 - <csr-id-c2bb9cb5e2ba1c6b055f6726e86ffc95dab90d2c/> set NATS queue group
 - <csr-id-659cb2eace33962e3ed05d69402607233b33a951/> conflate `wasi:blobstore/container` interface with `blobstore`
 - <csr-id-070751231e5bb4891b995e992e5206b3050ecc30/> pass original component instance through the context
 - <csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/> upgrade `wrpc`, `async-nats`, `wasmtime`
 - <csr-id-ed4b84661c08e43eadfce426474a49ad813ea6ec/> support ScaleComponentCommand w/ update
 - <csr-id-e17fe933ffdc9b4e6938c4a0f2943c4813b658b1/> allow empty payloads to trigger stop_host
 - <csr-id-a0a1b8c0c3d82feb19f42c4faa6de96b99bac13f/> add link name to wRPC invocations
   This commit adds the "link-name" header to invocations performed by
   the host using wRPC.
 - <csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/> Add support for configuring grpc protocol with opentelemetry
 - <csr-id-0aa01a92925dc12203bf9f06e13d21b7812b77eb/> Updates host to support new wasm artifact type
   This change is entirely backwards compatible as it still supports the
   old artifact type. I did test that this can download old and new
   manifest types
 - <csr-id-077a28a6567a436c99368c7eb1bd5dd2a6bc6103/> gracefully shutdown epoch interrupt thread
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
 - <csr-id-3eb453405aa144599f43bbaf56197566c9f0cf0a/> count epoch in a separate OS thread
 - <csr-id-b8c34346137edf5492fe70abeb22336a33e85bc0/> handle invocations in tasks
 - <csr-id-a66921edd9be3202d1296a165c34faf597b1dec1/> propagate `max_execution_time` to the runtime
 - <csr-id-e928020fd774abcc213fec560d89f128464da319/> limit max execution time to 10 minutes
 - <csr-id-33b50c2d258ca9744ed65b153a6580f893172e0c/> update to Wasmtime 20
 - <csr-id-9cd2b4034f8d5688ce250429dc14120eaf61b483/> update `wrpc:keyvalue` in providers
   part of this process is adopting `wit-bindgen-wrpc` in the host
 - <csr-id-a1754195fca5a13c8cdde713dad3e1a9765adaf5/> update `wasi:keyvalue`
 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`
 - <csr-id-dd0d449e5bfc3826675f3f744db44b3000c67197/> add label_changed event for label update/delete
   This commit adds a `label_changed` event that can be listened to in
   order to be notified of label changes on a host.
   
   The single event handles both updates and deletes.
 - <csr-id-07b5e70a7f1321d184962d7197a8d98d1ecaaf71/> use native TLS roots along webpki
 - <csr-id-5c3dc963783c71fc91ec916be64a6f67917d9740/> fetch configuration direct from bucket
 - <csr-id-383b3f3067dddc913d5a0c052f7bbb9c47fe8663/> implement `wrpc:blobstore/blobstore` for FS
 - <csr-id-614af7e3ed734c56b27cd1d2aacb0789a85e8b81/> implement Redis `wrpc:keyvalue/{atomic,eventual}`
 - <csr-id-e0dac9de4d3a74424e3138971753db9da143db5a/> implement `wasi:http/outgoing-handler` provider
 - <csr-id-e14d0405e9f746041001e101fc24320c9e6b4f9c/> deliver full config with link
 - <csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/> Add flags for overriding the default OpenTelemetry endpoint
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
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
 - <csr-id-abb81ebbf99ec3007b1d1d48a43cfe52d86bf3e7/> include actor_id on scaled events
 - <csr-id-be1e03c5281c9cf4b02fe5349a8cf5d0d7cd0892/> downgrade provider claims to optional metadata
 - <csr-id-8afb61fb6592db6a24c53f248e4f445f9b2db580/> downgrade actor claims to optional metadata
 - <csr-id-82c249b15dba4dbe4c14a6afd2b52c7d3dc99985/> Glues in named config to actors
   This introduces a new config bundle that can watch for config changes. There
   is probably a way to reduce the number of allocations here, but it is good
   enough for now.
   
   Also, sorry for the new file. I renamed `config.rs` to `host_config.rs` so
   I could reuse the `config.rs` file, but I forgot to git mv. So that file
   hasn't changed
 - <csr-id-1dc15a127ac9830f3ebd21e61a1cf3db404eed6d/> implement AcceptorWithHeaders
 - <csr-id-fd50dcfa07b759b01e32d7f974105615c8c47db4/> implement wasmcloud_transport wrappers
 - <csr-id-f2223a3f5378c3cebfec96b5322df619fcecc556/> implement `wrpc:http/incoming-handler`
 - <csr-id-fedfd92dbba773af048fe19d956f4c3625cc17de/> begin incoming wRPC invocation implementation
 - <csr-id-0c0c004bafb60323018fc1c86cb13493f72d29cd/> switch to `wrpc` for `wasmcloud:messaging`
 - <csr-id-5ede01b1fe0bc62234d2b7d6c72775d9e248a130/> switch to `wrpc:{keyvalue,blobstore}`
 - <csr-id-246384524cfe65ce6742558425b885247b461c5c/> implement `wrpc:http/outgoing-handler.handle`
 - <csr-id-5173aa5e679ffe446f10aa549f1120f1bd1ab033/> support component invoking polyfilled functions
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
 - <csr-id-fec6f5f1372a1de5737f5ec585ad735e14c20480/> remove module support
 - <csr-id-7d51408440509c687b01e00b77a3672a8e8c30c9/> add invocation and error counts for actor invocations
   Add two new metrics for actors:
   * the count of the number of invocations (`wasmcloud_host.actor.invocations`)
   * the count of errors (`wasmcloud_host.actor.invocation.errors`)
   
   This also adds a bunch of new attributes to the existing actor metrics so that they make sense in an environment with multiple hosts. Specifically this adds:
   * the lattice ID
   * the host ID
   * provider information if a provider invoked the actor: ** the contract ID
   ** the provider ID
   ** the name of the linkdef
   
   For actor to actor calls, instead of having the provider metadata it instead has the public key of the invoking actor.
   
   An example of what this looks like as an exported Prometheus metric:
   
   ```
   wasmcloud_host_actor_invocations_total{actor_ref="wasmcloud.azurecr.io/echo:0.3.8", caller_provider_contract_id="wasmcloud:httpserver", caller_provider_id="VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M", caller_provider_link_name="default", host="ND7L3RZ6NLYJGN25E6DKYS665ITWXAPXZXGZXLCUQEDDU65RB5TVUHEN", job="wasmcloud-host", lattice="default", operation="HttpServer.HandleRequest"}
   ```
   
   Provider metrics will likely need to wait until the wRPC work is finished.
 - <csr-id-17648fedc2a1907b2f0c6d053b9747e72999addb/> Add initial support for metrics
 - <csr-id-7b2d635949e2ebdb367eefb0b4ea69bf31590a7d/> remove requirement for actors to have capabilities in claims
 - <csr-id-6994a2202f856da93d0fe50e40c8e72dd3b7d9e6/> add event name as suffix on event topic
 - <csr-id-85cb573d29c75eae4fdaca14be808131383ca3cd/> enable updating host labels via the control interface
 - <csr-id-64d21b1f3d413e4c5da78d8045c1366c3782a190/> Adds some additional context around test failures I was seeing
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
   
   With that said, note that this is only going to be added for actors built against
   the component model. Since this is net new functionality, I didn't think it was
   worth it to try to backport.
   
   As for testing, I have tested that an actor can import the functions and get the values
   via the various e2e tests and also manually validated that all of the new topics
   work.
 - <csr-id-cfb66f81180a3b47d6e7df1a444a1ec945115b15/> implement wasifills for simple types
 - <csr-id-2e8982c962f1cbb15a7a0e34c5a7756e02bb56a3/> implement outgoing HTTP
 - <csr-id-44019a895bdb9780abea73a4dc740febf44dff6f/> handle ctl requests concurrently
 - <csr-id-977feaa1bca1ae4df625c8061f2f5330029739b4/> parse labels from args
 - <csr-id-ba675c868d6c76f4e717f64d0d6e93affea9398d/> support annotation filters for stop/scale
 - <csr-id-68c41586cbff172897c9ef3ed6358a66cd9cbb94/> publish periodic provider health status
 - <csr-id-05f452a6ec1644db0fd9416f755fe0cad9cce6d3/> implement `wasi:logging` for actors
 - <csr-id-9e61a113c750e885316144681946187e5c113b49/> ignore `stop_provider` annotations
 - <csr-id-2ebdab7551f6da93967d921316cae5d04a409a43/> support policy service
 - <csr-id-123cb2f9b8981c37bc333fece71c009ce875e30f/> add support for call aliases
 - <csr-id-813ce52a9c11270814eec051dfaa8817bf9f567d/> support chunking and dechunking of requests
 - <csr-id-bef159ab4d5ce6ba73e7c3465110c2990da64eac/> implement `wasi:blobstore`
 - <csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/> support OTEL traces end-to-end
 - <csr-id-c334d84d01b8b92ab9db105f8e6f0c4a6bcef8b1/> send OTEL config via HostData
 - <csr-id-002c9931e7fa309c39df26b313f16976e3a36001/> add support for putting registry credentials via control interface
 - <csr-id-48d4557c8ee895278055261bccb1293806b308b0/> support registry settings via config service and command-line flags
 - <csr-id-d434e148620d394856246ac34bb0a64c37181970/> partially implement `wasi:keyvalue/atomic`
 - <csr-id-50d0ed1086c5f417ed64dcce139cc3c2b50ca14c/> implement `wasmcloud:http/incoming-handler` support
 - <csr-id-31b76fd2754e1962df36340275ad5179576c8d07/> delete claims when actors or providers are stopped
 - <csr-id-958aad5ce94120322a920be71626c1aa6a349d0c/> remove actor links on deletion
 - <csr-id-2e3bd2bd7611e5de9fe123f53778f282613eb0de/> implement link names and a2a calls
 - <csr-id-6fd00493232a2c860e94f6263a3a0876ad7a6acb/> fill in missing data in host pings and heartbeat messages
 - <csr-id-3588b5f9ce2f0c0a4718d9bd576904ef77682304/> implement ctl_topic_prefix
 - <csr-id-d367812a666acced17f1c0f795c53ac8cf416cc6/> add claims and link query functionality
 - <csr-id-2b07909e484f13d64ad54b649a5b8e9c36b48227/> introduce `wasmcloud-compat` crate
 - <csr-id-556da3fb0666f61f140eefef509913f1d34384a3/> generate host name based on a random number
 - <csr-id-a5db5e5c0d13d66bf8fbf0da7c4f3c10021d0f90/> add support for non-default link names
 - <csr-id-c9fecb99793649a6f9321b9224f85b9472889dec/> add support for custom lattice prefix
 - <csr-id-77d663d3e1fd5590177ac8003a313a3edf29ab1f/> implement `wasmcloud:messaging/consumer` support
 - <csr-id-02c1ddc0d62b40f63afe4d270643ebc3bf39c081/> implement `wasi:keyvalue/readwrite` support
 - <csr-id-cf3c76a96c7fb411d0c286a687ccf1633cb5feeb/> handle launch commands concurrently
 - <csr-id-4de853a1d3e28126faf9efa51aaa97714af7b493/> implement actor -> provider linking
 - <csr-id-c486dbf6116884da916da700b77559a8dbef9389/> implement update actor
 - <csr-id-e943eca7512a0d96a617451e2e2af78718d0f685/> implement linkdef add/delete
 - <csr-id-d5beecd3d756a50f7b07e13afd688b2518039ee3/> implement start and stop provider commands
 - <csr-id-32cead5ec7c1559ad0c161568712140b7d89d196/> implement actor operations
 - <csr-id-0d88c2858ef950975bb0309bfb906881d6e8e7a6/> implement inventory
 - <csr-id-ec5675d11768ed9741a8d3e7c42cc1e5a823d41d/> implement host stop
 - <csr-id-239f8065b63dc5ea2460ae378840874ac660856b/> implement host ping
 - <csr-id-e26a5b65e445d694acf3d8283cd9e80e850f8fa5/> apply labels from environment
 - <csr-id-ef20466a04d475159088b127b46111b80a5e1eb2/> introduce wasmbus lattice
 - <csr-id-7364dd8afae5c8884ca923b39c5680c60d8d0e3d/> implement data streaming
   - make claims optional (at least for now)
   - add streaming support to `wasmcloud:bus`
   - rename `wasmcloud_host` -> `wasmcloud_runtime`
   - remove all `wasmcloud-interface-*` usages
   - add support for `command` executables (I/O actors)
   - add local lattice proving the concept, which is used for testing of the feature
   - implement an actor instance pool
 - <csr-id-caa965ac17eeda67c35f41b38a236f1b682cf462/> implement builtin capabilities via WIT

### Bug Fixes

 - <csr-id-8cc957a4ee878b3ccc85cf3e7bddcbf5c3ab376a/> ignore auctions for builtin providers when disabled
   This commit enables hosts to ignore auctions for builtin providers if
   they are enabled. While in the past a response to start_provider was
   assumed for every message, we loosen that to return an optional response.
 - <csr-id-2acf13b67e9e6f91598922c522619dfe24d3fb74/> check for host/component mismatches in http host router
 - <csr-id-1ed2c24bcbe77702855aa0525633dee948bf05f2/> disallow processing of 'hostcore.*' label puts
 - <csr-id-f7744ab9a93c6e0f525e25c42ac0838750722b9e/> use additional cas to pull providers
 - <csr-id-d7205f1a59cbde3270669d3b9c2ced06edd4b8ab/> nbf/exp in cloudevents
 - <csr-id-350fa8f6b796ba6ba302ce9bd2094798ef165bf2/> output host_id on provider_start_failed
 - <csr-id-9d82e3cff7703006da3b4de1a8b66e37e3d95e21/> components can not be deleted when wasm file can not be found: #2936
 - <csr-id-221eab623e4e94bf1e337366e0155453ffa64670/> reintroduce joinset for handles
 - <csr-id-d3df101596f8df6913f4bc07cc1395a2e361f09c/> default to `status: ok` in `/readyz`
   If the host returns `status: fail` *before* the connection to e.g.
   NATS.io is established, an eager observer may incorrectly conclude
   that the host has failed, even though it really *has not started yet*
   
   This aproach seems to align better with:
   https://kubernetes.io/docs/tasks/configure-pod-container/configure-liveness-readiness-startup-probes/
 - <csr-id-27266a01c561b48d6c85a7a5299fbc6ac2c6144d/> allow deletion of labels without providing value
 - <csr-id-3de0c70e1271e57f6e84a4eaa591343abe0f5bf2/> specify type for ambiguous as_ref
 - <csr-id-fdf399245df2feef7d7a372aaa332546d9ebef51/> allow unique interfaces across the same package
 - <csr-id-2f157ba0aa0d784663d24682361eadb8e7796f97/> enable experimental features in test host
 - <csr-id-7413aeb39e7f3b2e7550cdefafe982d6ffbe2da6/> connect function trace to context
 - <csr-id-c5c6be96a3fe6dd3b7264ed7acf6b105404652be/> propagate serve trace into handlers
 - <csr-id-cc3f219a4cbce29e322074284e4e8574c7f5b36a/> remove export joinset
 - <csr-id-acd6143f35ad25edf094e93d24118e2a8f13e1d8/> Fixes orphaned traces
   We had the correct code for extracting tracing headers from wRPC calls,
   but our use of `in_current_span` was creating unintentionally orphaned
   spans. This adds an explicit span for each incoming invocation. See
   the comment added in the code for additional information
 - <csr-id-44e1d0ef5acd63fbb08f5887557d39ce74680378/> end epoch interruption, don't await
 - <csr-id-bb53c9dfd62d8b886b3296901a5ce6716551304d/> warn if config sender is dropped
 - <csr-id-73631912ec94438fb527961dbb40a1e519f1d44b/> ensure config sender is not dropped
 - <csr-id-74dd7fc104c359213ad8330aee62a82c2dbe2629/> update usage of provider description
 - <csr-id-4da0105ac7bf463eeb79bc3047cb5e92664f8a7c/> rework `wasi:http` error handling
 - <csr-id-726dd689e6d64eb44930834425d69f21cefc61cd/> log handling errors
 - <csr-id-fc131edff75a7240fe519d8bbc4b08ac31d9bf1c/> Sets a higher value for the incoming events channel
   If you were running a high number of concurrent component invocations, it
   would result in a warning (and possible hang/dropped message) due to a
   full channel. This change attempts to set the channel size to the
   `max_instances` value with a minimum and a maximum possible value (i.e
   we don't want something with 20k instances to have a channel that large).
 - <csr-id-991cb21d5bceee681c613b314a9d2dfaeee890ee/> remove provider caching for local file references
   This commit removes provider caching for local file references -- when
   a file is loaded via a container registry, caching is enabled but if
   it is loaded via a local file on disk, caching is never employed.
 - <csr-id-77b1af98c1cdfdb5425a590856c0e27f2a7e805f/> prevent Provider ConfigBundle early drop, do cleanup
 - <csr-id-76265cdcbf2959f87961340576e71e085f1f4942/> always publish component_scale event
 - <csr-id-2695ad38f3338567de06f6a7ebc719a9421db7eb/> pass policy string directly to backend
 - <csr-id-1914c34317b673f3b7208863ba107c579700a133/> use name instead of key for secret map
 - <csr-id-5506c8b6eb78d8e4b793748072c4f026a4ed1863/> skip backwards-compat link with secret
 - <csr-id-5c68c898f8bd8351f5d16226480fbbe726efc163/> check provided secrets topic for non-empty
 - <csr-id-b014263cf3614995f597336bb40e51ab72bfa1c9/> setup debug traces
   This commit contains experimental code used to debug/replicate the
   o11y traces for making a call with http-client & http-provider.
   
   Running this requires the following hackery:
   
   - running the docker compose for o11y
   - (re) building dog-fetcher
   - modifying the WADM w/ dog fetcher (done by this commit)
   - build & create PAR for http-client
   - build & create PAR for http-server
   - set WASMCLOUD_OVERRIDE_TRACES_ENDPOINT before `wash up`
   - replacing existing wasmcloud host (in `~/.wash/downloads/v1.0.2`)
 - <csr-id-fa1fde185b47b055e511f6f2dee095e269db1651/> propagate traces through components
 - <csr-id-3cabf109f5b986079cceb7f125f75bf53348712e/> handle invocation handling errors
 - <csr-id-c87f3fe2654d5c874708974915bdd65f69f4afe1/> remove publish_event from stop_actor
 - <csr-id-9542e16b80c71dc7cc2f9e7175ebb25be050a242/> differentiate no config and config error
 - <csr-id-dcbbc843c5a571e1c33775c66bbd3cd528b02c26/> allow overwriting provider reference
 - <csr-id-804d67665fac39c08a536b0902a65a85035e685e/> warn scaling with different imageref
 - <csr-id-91c57b238c6e3aec5bd86f5c2103aaec21932725/> rename scaled ID from actor to component
 - <csr-id-ef50f046ade176cabbf690de59caad5d4f99c78f/> Don't clone targets with handlers
   This is a fix that ensures each component has its own set of link name
   targets. Before this, it was sharing the whole set of link names between
   all running component instances (of each individual component).
 - <csr-id-2b500d7a38cb338620f9c7834ca7fef882e42c92/> deliver target links to started provider
 - <csr-id-4ed38913f19fcd4dd44dfdcc9007e80e80cdc960/> fix `link_name` functionality, reorganize tests
 - <csr-id-ccb3c73dc1351b11233896abc068a200374df079/> correct name and data streaming, update WIT
 - <csr-id-5b4f75b7b843483566983c72c3a25e91c3de3adc/> Recreates polyfill imports on update
   This fixes an issue where if you add a new custom interface to an actor
   when updating it, it would fail to have the imports in place
 - <csr-id-fd85e254ee56abb65bee648ba0ea93b9a227a96f/> fix deadlock and slow ack of update
 - <csr-id-cab6fd2cae47f0a866f17dfdb593a48a9210bab8/> flatten claims response payload
 - <csr-id-9fe1fe8ce8d4434fb05635d7d1ae6ee07bc188c3/> race condition with initial config get
 - <csr-id-149f98b60c1e70d0e68153add3e30b8fb4483e11/> improve target lookup error handling
 - <csr-id-ec84fadfd819f203fe2e4906f5338f48f6ddec78/> update wrpc_client
 - <csr-id-152186f9940f6c9352ee5d9f91ddefe5673bdac1/> re-tag request type enum policy
 - <csr-id-a6ec7c3476daf63dc6f53afb7eb512cfc3d2b9d8/> instrument handle_invocation and call
 - <csr-id-4aa31f74bf84784af0207d2886f62d833dfe1b63/> Fixes write lock issue on policy service
   Our policy decision logic was taking a write lock even when reading the queue.
   This basically treated it like a mutex and slowed down the number of requests
   we could handle.
 - <csr-id-f3bc96128ed7033d08bc7da1ea7ba89c40880ede/> encode custom parameters as tuples
 - <csr-id-9e304cd7d19a2f7eef099703f168e8f155d4f8bc/> correctly invoke custom functions
 - <csr-id-e9bea42ed6189d903ea7fc6b7d4dc54a6fe88a12/> bindgen issues preventing builds
   This commit fixes the provider bindgen issues for non http-server
   builds (ex. kv-redis)
 - <csr-id-637810b996b59bb4d576b6c1321e0363b1396fe5/> set log_level for providers
 - <csr-id-c6fa704f001a394c10f8769d670941aff62d6414/> fix clippy warning, added ; for consistency, return directly the instance instead of wrapping the instance's components in a future
 - <csr-id-7db1183dbe84aeeb1967eb28d71876f6f175c2c2/> Add comments, remove useless future::ready
 - <csr-id-1d3fd96f2fe23c71b2ef70bb5199db8009c56154/> fmt
 - <csr-id-38faeace04d4a43ee87eafdfa129555370cddecb/> add subject to control interface logs
 - <csr-id-39849b5f2fde4d80ccfd48c3c765c258800645ea/> publish claims with actor_scaled
 - <csr-id-9d1f67f37082597c25ae8a7239321d8d2e752b4d/> override previous call alias on clash
 - <csr-id-37618a316baf573cc31311ad3ae78cd054e0e2b5/> update format for serialized claims
 - <csr-id-7e53ed56244bf4c3232b390dd1c3984dbc00be74/> disable handle_links trace until wadm sends fewer requests
 - <csr-id-1a86faa9af31af3836da95c4c312ebedaa90c6bc/> queue subscribe to linkdefs and get topics
 - <csr-id-774bb0401d141c59cdd8c73e716f5d8c00002ea0/> drop problematic write lock
 - <csr-id-8fdddccf5931cd10266a13f02681fdbfb34aba37/> publish correct number of actor events
 - <csr-id-e9a391726ad1b7a2e01bab5be09cd090f35fe661/> stop sending linkdef events on startup
 - <csr-id-3fb60eeca9e122f245b60885bdf13082c3697f04/> change expected host label prefix to remove collision with WASMCLOUD_HOST_SEED
 - <csr-id-ac935a8028d2ba6a3a356c6e28c3681492bc09a1/> fixes #746
 - <csr-id-214c5c4cce254b641d93882795b6f48d61dcc4f9/> return an InvocationResponse when failing to decode an invocation
 - <csr-id-88b2f2f5b2424413f80d71f855185304fb003de5/> deprecate HOST_ label prefix in favor of WASMCLOUD_HOST_
 - <csr-id-ebe70f3e8a2ae095a56a16b954d4ac20f4806364/> download actor to scale in task
 - <csr-id-691c3719b8030e437f565156ad5b9cff12fd4cf3/> proxy RUST_LOG to providers
 - <csr-id-2314f5f4d49c5b98949fe5d4a1eb692f1fad92b7/> rework host shutdown
   - Always include a timeout for graceful shutdown (e.g. if NATS
     connection dies, it will never finish)
   - Stop if one of the core wasmbus tasks dies
   - Flush NATS queues concurrently on shutdown
   - Handle `stopped` method errors
 - <csr-id-3cef088e82a9c35b2cef76ba34440213361563e4/> enforce unique image references for actors
 - <csr-id-28d2d6fc5e68ab8de12771fb3b0fb00617b32b30/> properly format actors_started claims
 - <csr-id-bdd0964cf6262c262ee167993f5d6d48994c941d/> Flushes clients when responding to ctl requests
   In cases where wadm was fairly busy, we started getting errors that the
   host wasn't acking our scale actor commands (even though it was actually
   scaling). So I added in some flushing when we send responses so we can be
   sure that the response actually got sent
 - <csr-id-f4ef770dda0af0c1e7df607abbe45888d819260a/> proxy SYSTEMROOT to providers on Windows
 - <csr-id-b2d2415a0370ff8cae65b530953f33a07bb7393a/> use named fields when publishing link definitions to providers
 - <csr-id-1829b27213e836cb347a542e9cdc771c74427892/> allow namespaces with slashes
 - <csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/> look for invocation responses from providers
 - <csr-id-43a75f3b222d99259c773f990ef8ae4754d3b6fc/> store claims on fetch
 - <csr-id-4e4d5856ae622650d1b74f2c595213ef12559d9d/> clean-up imports
 - <csr-id-d1042261b6b96658af4032f5f10e5144b9a14717/> expose registry as a public module
 - <csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/> attach traces on inbound and outbound messages
   Parse headers from CTL interface and RPC messages, and publish tracing headers
   on CTL and RPC responses
 - <csr-id-99aa2fe060f1e1fe7820d7f0cc41cc2584c1e533/> Flushes NATS clients on host stop
   Without this, sending responses to things like a host stop command or
   publishing the host stop event can fail as we don't ensure all messages
   in the NATS client queue have been sent
 - <csr-id-59e98a997a4b6cc371e4983c42fb6609b73f7b53/> unwrap expired
 - <csr-id-680def637270c23541d9263db47e9834a9081809/> handle stored claims without config_schema
 - <csr-id-c63b6500264128904e9021cea2e3490a74d04107/> return invocation responses for host failures
 - <csr-id-45b0fb0960921a4eebd335977fd8bc747def97a4/> pub the context mod only with the otel feature enabled
 - <csr-id-f2bf50dc6c2cda49c4d82a877aaf554f153f494a/> use cached links for queries
 - <csr-id-11ea950ee26e4b7b7909d04c3505c80b4939efbb/> remove redundant claim clone
 - <csr-id-64592ede426193873de52fde8cf98611b6a872a8/> always include cluster key as a valid issuer
 - <csr-id-47f45487b46891cfbab5611ee41f52c6582a1dd8/> pass OTEL settings to providers via deprecated env vars
 - <csr-id-02bc0c4f348da19f058787da9a314dd9b634c6ae/> ignore empty responses
 - <csr-id-75a1fb075357ac2566fef1b45c930e6c400a4041/> store typed keys, not strings
 - <csr-id-d9775af7c953749f37978802c690ee29838f0da6/> properly handle empty responses
 - <csr-id-33ef4f34a5748e445f01148ec7d00bb0f01c1606/> do not proxy env vars from host to providers
 - <csr-id-7a84469dae07cd31185dbb0ad6cfd0af02d0e3a3/> Matches up base64 encoding to what providers expected

### Other

 - <csr-id-c670a164afed54c56c9cfa81efb6adbf0865564a/> Makes sure to only refresh data ones
 - <csr-id-185c5d686ef2d8a058395936ac564984fd2a5d7a/> document `Message`
 - <csr-id-ef45f597710929d41be989110fc3c51621c9ee62/> bump wascap v0.15.2, provider-archive v0.14.0, wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wasmcloud-secrets-types v0.5.0, wash-cli v0.37.0, safety bump 9 crates
   SAFETY BUMP: wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wash-cli v0.37.0, wasmcloud-host v0.23.0, wasmcloud-runtime v0.7.0, wasmcloud-test-util v0.15.0, wasmcloud-secrets-client v0.6.0
 - <csr-id-2c335617918a20561ed33259e54b2451ad534343/> wasmcloud-host 0.20.0
 - <csr-id-9ac2e29babcaa3e9789c42d05d9d3ad4ccd5fcc7/> add links integration test
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
 - <csr-id-c71f153b84e4ac4f84bdb934c9f7ca735eddd482/> add secrecy
 - <csr-id-5225b1961038b815fe98c5250278d1aa483bdded/> fix outdated `ctl_seed` reference
 - <csr-id-c4b82f28947f06253aa997ae65ab11ebcc507f49/> document invocation handling failures
 - <csr-id-45a3d3f477b48e8a79e77880950bb785175a990d/> check component update ref and respond with a message before task
 - <csr-id-95081cacfc3fc04911c91c32f462d643be2e12ed/> check component image reference on component update
 - <csr-id-173bfa623328bd1790642ddd6d56c6f9e5b38831/> expect stop_actor function parameter host_id to be unused
 - <csr-id-c7a7ed73f5497f83a9dcfb509df580cdec3a4635/> update `wrpc-interface-http`
 - <csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/> update wRPC
 - <csr-id-a96b1f370392063f403e9f25e0ef21c30fdcdfa9/> update wRPC
 - <csr-id-49f3883c586c098d4b0be44793057b97566ec2e1/> update to wasmtime 17
 - <csr-id-0f967b065f30a0b5418f7ed519fdef3dc75a6205/> 'upstream/main' into `merge/wash`
 - <csr-id-d16324054a454347044f7cc052da1bbd4324a284/> bump crate versions
 - <csr-id-578c72d3333f1b9c343437946114c3cd6a0eead4/> bump to `0.79.0`
 - <csr-id-22276ff61bcb4992b557f7af6624c9715f72c32b/> update dependencies
 - <csr-id-801377a4445cfb4c1c61a8b8f9ecbe956996272b/> bump version to `0.78.0`
 - <csr-id-cb86378831e48368d31947b0a44ef39080fe6d70/> update dependencies
 - <csr-id-b2c6676987c6879fb4fcf17066dca6c9129f63b1/> remove `wit-deps` build scripts
 - <csr-id-ed4282c9ea1bb95e346c9a981acdc264b0fc9d3f/> update WIT dependencies
 - <csr-id-9ee32d6fa889db105608e6df3d7533a33b26f540/> update dependencies
 - <csr-id-b18cd737a830590d232287a0ca0218357cb35813/> update `preview2-prototyping`

### Refactor

 - <csr-id-3aec2348c4f59d5a667e344b7f06e86673f67da3/> add convenience fn for returning server err
 - <csr-id-e748d2df9303251a6ba88677164cc720ad1bdf2b/> simplify shutdown binary provider usage
 - <csr-id-c5d0f25ef7eec75efcd98332f1eef66d4d955524/> clean up impl of provider restart
 - <csr-id-9e40f22614611ef3930359d66f3d55cfe8fb01b7/> use futures for binary provider tasks
 - <csr-id-40aa7cb6ea87aee91aeaf5c1f5edebe3fbfb2a65/> remove unused parameters
 - <csr-id-d6d474f1393c2f1d7c6f29ea18c02a3a1f132ea6/> create claims and jetstream modules
 - <csr-id-ae761a00f323f17d1fe9728eaeb10470d0b7bb93/> implement ControlInterfaceServer trait
 - <csr-id-faa91332c3fc43087080100688f30be4120bf8bd/> remove redundant variables
 - <csr-id-4f34e06be4f2f6267e55fb8333f3695243ee2e7a/> use fewer extra closure vars
 - <csr-id-54f9be522be602922b44858e56e0b1b61ecf313a/> http_server to explicit address file
 - <csr-id-b1b03bfd5669fe5a7de2bd5b134d61c91e8d8bd0/> be more compliant in HTTP admin
   - Implement `HEAD` for `/livez` and `/readyz`.
     At least according to MDN docs on 501:
     https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/501
     servers are required to implement `GET` and `HEAD` methods
   
   - Return 500 in `/readyz` if failure state is reached
   
   - Return 404 on requests to unknown paths with an error message
 - <csr-id-2697a2cc8a3477ddb03f602abbf54c221515e006/> clarify log statements for HTTP admin
 - <csr-id-52ed8befa81bab75817394e1136c196a962f8ca9/> consistently name features
 - <csr-id-30ba77a2253122af3d85e1bce89d70de66cdb283/> reword feature methods
 - <csr-id-aa57a6a455cf772b08a774cfed55fc847671a05b/> introduce `providers` module
 - <csr-id-53a7eb5bf13dd986fe9435a8444d1ec4f09cd329/> minor changes, remove print
 - <csr-id-a952c771164066715dee66ef824c9df662e0cf58/> update for control interface v2.2.0
 - <csr-id-d511d74c21ab96f5913f5546e8253f34c73642a1/> remove missing caching code
 - <csr-id-ac188921856c9b5fe669531e309f3f416d1bb757/> remove unused deps
 - <csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/> move functionality into core
   This commit moves functionality that was previously located in the
   unreleased `wasmcloud-host` crate into core.
 - <csr-id-47e80cf949a2cb287be479653336def31c130ba2/> abort health check tasks on provider drop
 - <csr-id-c666ef50fecc1ee248bf78d486a915ee077e3b4a/> include name with secret config
 - <csr-id-2ea22a28ca9fd1838fc03451f33d75690fc28f2a/> move SecretConfig into crate
 - <csr-id-b56982f437209ecaff4fa6946f8fe4c3068a62cd/> address feedback, application name optional
 - <csr-id-388662a482442df3f74dfe8f9559fc4c07cedbe5/> collapse application field
 - <csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/> efficiency, pass optional vec secrets
 - <csr-id-d8ad4376cb4db282047de8c4f62f6b8b907c9356/> improve error representations, cleanup
 - <csr-id-f354008c318f49565eb023a91cd3a3781d73c36a/> light refactor from followup
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
 - <csr-id-7f4cd4cf5da514bb1d10c9d064bb905de8621d8e/> improve error handling
 - <csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/> improve error usage of bail
 - <csr-id-1610702ad0f8cd3ba221c1b6b8ba2ce8fe57c6ae/> remove redundant handler clone
 - <csr-id-ef1d3af1ccddf33cdb37763101e3fb7577bf1433/> Actor -> Component
 - <csr-id-c654448653db224c6a676ecf43150d880a9daf8c/> move wasmcloud wrpc transport client to core
   This commit moves the wasmcloud-specific wrpc transport client to the
   `wasmcloud-core` crate. From here, it can be used by both
   the host (`wasmbus`) and other places like tests.
 - <csr-id-fe7592b1a5501f3faa8bcf3bf45edf4032e92f0b/> move label parsing out of host library
 - <csr-id-7de31820034c4b70ab6edc772713e64aafe294a9/> remove deprecated code related to start actor cmd
 - <csr-id-bdb72eed8778a5d8c59d0b8939f147c374cb671f/> rename label to key
 - <csr-id-a8e1c0d6f9aa461bf8e26b68092135f90f523573/> drop write locks immediately
 - <csr-id-f4611f31e12227ed1257bb95809f9542d1de6353/> remove unnecessary mut
 - <csr-id-017e6d40841f14b2158cf2ff70ca2ac8940e4b84/> remove instance pooling
 - <csr-id-ec2d0c134cd02dcaf3981d94826935c17b512d4e/> implement `ResourceRef::authority`
 - <csr-id-0261297230f1be083af15e257c967635654c2b71/> introduce artifact fetchers
 - <csr-id-21a7e3f4728a8163a6916b5d1817bac238b6fd46/> derive `Default` for `Auth`
 - <csr-id-7799e38ecc91c13add5213b72f5e56a5b9e01c6e/> rename `RegistrySettings` -> `RegistryConfig`
 - <csr-id-0a86d89a7b57329145e032b3dc2ac999d5f0f812/> rework fetching logic
 - <csr-id-9f9d0e4da2fafb368fa11fd5e692ded6d912d6e5/> be explicit about `async_nats` imports
 - <csr-id-6c42d5c50375cdc2d12c86513a98b45135f0d187/> reduce verbosity on actor logs
 - <csr-id-463a2fbc7887ac7f78d32ccd19266630f5914f2e/> flatten optional annotations to always be set
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers
 - <csr-id-0db5a5ba5b20535e16af46fd92f7040c8174d636/> establish NATS connections concurrently
 - <csr-id-5ce8d6a241f36d76013de1cc5827bf690fc62911/> use `wasmcloud-compat` structs
 - <csr-id-a9f3ba05665d0fe7b36f0df5ed4c202dafadd0bf/> remove unnecessary allocations
 - <csr-id-6b3080a8f655ce36b0cc6ef381ae0bf40e0e2a67/> create bucket explicitly instead of stream
   This also gracefully handles errors where the bucket has already
   been provisioned with custom settings, allowing multiple hosts to
   run in the same pre-provisioned lattice
 - <csr-id-977260cb713f16cb2a42e4881dc4e2b5e03d481b/> exclude self from instruments
 - <csr-id-4e8ef1103a7943a8a6c921b632093e540a7b8a1b/> use `wasmcloud-control-interface`
 - <csr-id-8bfa14f0c25a9c279a12769328c4104b8ca0de74/> expand parameter names
 - <csr-id-805f9609dbc04fd4ed8afd2447896988cbcc4ab5/> remove `wasmbus-rpc` usage

### Style

 - <csr-id-ec3bae5c03c77a0b77884b84754e33e1a8361a89/> comment
 - <csr-id-019f63bd9b46f68fc4703242c17cc3e38f0f889c/> address nits
 - <csr-id-782a53ebb8a682197ebb47f4f7651dc075690e22/> use skip_all
 - <csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/> replace needs_chunking function with direct comparison
 - <csr-id-c47ee0cdd3225c25d2ef54bee1bbc42b39375b65/> move longer fields to their own lines
 - <csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/> update imports
 - <csr-id-f2246c07cf38a6f142d7ce58e0623f8da5adbe83/> satisfy clippy
 - <csr-id-594254af85aeaccae50337d3a8514714d11d2720/> stop unnecessarily satisfying clippy
 - <csr-id-ce93e4aad4148a51c2d30b58bdccd17ef38a9954/> remove constants
 - <csr-id-f3f6c21f25632940a6cc1d5290f8e84271609496/> rename most instances of lattice and wasmbus to host
 - <csr-id-c17d7426de06282d8f9d867ef227dc59d4227965/> use context

### Test

 - <csr-id-42daec5d0ab31db9871e3c99aed15eb2171c6387/> add builtin path http-server tests
 - <csr-id-450314effb4676207df048a2478f22cd0cd6455b/> test linking to different interface targets

### Chore (BREAKING)

 - <csr-id-5f7e0132362f7eda0710a1a69d5944140fd74b07/> Updates dependencies
   This does an update of pretty much all of the dependencies possible
   in the main tree. Any code changes were refactors maintaining the same
   behaviors, but using any updated APIs.
   
   This is noted as breaking because the updates to the crates bubble up
   through the `core` crate, so it technically breaks that API since we
   reexport. If we think that isn't worth it, I can revert that bit.
 - <csr-id-f418ad9c826e6ed6661175cf883882a37d5af1eb/> update host w/ new ctrl iface
 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-bcbb402c2efe3dc881b06e666c70e01e94d3ad72/> rename ctl actor to component
 - <csr-id-a1e8d3f09e039723d28d738d98b47bce54e4450d/> update ctl to 0.31.0

### New Features (BREAKING)

 - <csr-id-2e02f107a6cd6a10c5df59228f61b7ed96027490/> implement traits for extending host functionality
 - <csr-id-30281b33a5d7d75292b421954938d9602ecb9665/> supervise and restart providers on exit
 - <csr-id-5f05bcc468b3e67e67a22c666d93176b44164fbc/> add checked set_link_name
 - <csr-id-be7233e730dce14578651a17d16410d7a7dbe91c/> introduce linkdef_set_failed event
 - <csr-id-f4b4eeb64a6eab4f6dfb540eacd7e2256d80aa71/> allow tuning runtime parameters
 - <csr-id-d9281e2d54ac72e94f9afb61b3167690fe1fd89b/> encrypt link secrets, generate xkeys for providers
 - <csr-id-2378057bbbabbfa5a2159b6621d6009396411dd7/> configure observability with trace_level option
 - <csr-id-98b3986aca562d7f5439d3618d1eaf70f1b7e75a/> add secrets backend topic flag
 - <csr-id-6b2e1b5915a0e894a567622ffc193230e5654c1f/> Removes old guest config and uses runtime config instead
   Most of the changes are related to wit updates, but this removes the
   guest config from `wasmcloud:bus` and pulls down `wasi:config` in its
   place
 - <csr-id-9e23be23131bbcdad746f7e85d33d5812e5f2ff9/> rename actor_scale* events
 - <csr-id-f34aac419d124aba6b6e252f85627847f67d01f4/> remove capabilities
 - <csr-id-3f2d2f44470d44809fb83de2fa34b29ad1e6cb30/> Adds version to control API
   This should be the final breaking change of the API and it will require
   a two phased rollout. I'll need to cut new core and host versions first
   and then update wash to use the new host for tests.
 - <csr-id-91874e9f4bf2b37b895a4654250203144e12815c/> convert to `wrpc:blobstore`
 - <csr-id-716d251478cf174085f6ff274854ddebd9e0d772/> use `wasmcloud:messaging` in providers
   Also implement statically invoking the `handler` on components in the
   host
 - <csr-id-5c1a0a57e761d405cdbb8ea4cbca0fe13b7e8737/> start providers with named config
 - <csr-id-188f0965e911067b5ffe9c62083fd4fbba2713f4/> refactor componentspec, deliver links to providers
 - <csr-id-df01397bce61344d3429aff081a9f9b23fad0b84/> cache request by unique data
 - <csr-id-1fb6266826f47221ec3f9413f54a4c395622dcbd/> formalize policy service
 - <csr-id-4a4b300515e9984a1befe6aaab1a6298d8ea49b1/> wrap all ctl operations in CtlResponse
 - <csr-id-e16da6614ad9ae28e8c3e6ac3ebb36faf12cb4d1/> remove collection type aliases
 - <csr-id-5275937c2c9b25139f3c208af7909889362df308/> flatten instances on actor/providers
 - <csr-id-48fc893ba2de576511aeea98a3da4cc97024c53e/> fully support interface links, remove aliases
 - <csr-id-49e5943d9a087b5ef5428f73281c36030d77502c/> support wrpc component exports
 - <csr-id-5af1138da6afa3ca6424d4ff10aa49211952c898/> support interface link put, component spec
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
 - <csr-id-2e8893af27700b86dbeb63e5e7fc4252ec6771e1/> add heartbeat fields to inventory
 - <csr-id-032e50925e2e64c865a82cbb90de7da1f99d995e/> change heartbeat payload to inventory
 - <csr-id-df01bbd89fd2b690c2d1bcfe68455fb827646a10/> remove singular actor events, add actor_scaled
 - <csr-id-5cca9ee0a88d63cb53e8d352c16a5d9d59966bc8/> upgrade max_instances to u32
 - <csr-id-d8eb9f3ee9df65e96d076a6ba11d2600d0513207/> rename max-concurrent to max-instances, simplify scale
 - <csr-id-97ecbb34f81f26a36d26f458c8487e05dafa101e/> use max concurrency instead of count
 - <csr-id-ccec9edac6c91def872ca6a1a56f62ea716e84a2/> validate invocations for antiforgery and claims
 - <csr-id-72b7609076ca3b97faf1c4a14489d1f466cf477a/> implement provider health checks
 - <csr-id-ed64180714873bd9be1f9008d29b09cbf276bba1/> implement structured logging
 - <csr-id-ff024913d3107dc65dd8aad69a1f598390de6d1a/> respect allow_file_load
 - <csr-id-39da3e77462d26c8d8d2a290ce33f29a954e83ba/> enforce rpc_timeout
 - <csr-id-921fa784ba3853b6b0a622c6850bb6d71437a011/> implement rpc,ctl,prov_rpc connections
 - <csr-id-7c389bee17d34db732babde7724286656c268f65/> use allow_latest and allowed_insecure config
 - <csr-id-9897b90e845470faa35e8caf4816c29e6dcefd91/> use js_domain provided by cli
 - <csr-id-7d290aa08b2196a6082972a4d662bf1a93d07dec/> implement graceful provider shutdown delay
 - <csr-id-194f791c16ad6a7106393b4bcf0d0c51a70f638d/> maintain cluster issuers list

### Bug Fixes (BREAKING)

 - <csr-id-8db14d3bb320e6732c62c3abfe936d72e45fe734/> ensure links are unique on source+interface+name
 - <csr-id-2798858880004225ebe49aa1d873019a02f29e49/> consistent host operations
 - <csr-id-545c21cedd1475def0648e3e700bcdd15f800c2a/> remove support for prov_rpc NATS connection

### Refactor (BREAKING)

 - <csr-id-c36dee94832d111c2a3ba5ff9f5e26baf2f3e4d9/> Removes dependencies from host on provider libraries
   Our current host had a circular dependency loop with itself because it
   depended on the http and messaging providers. In order to break this, I
   moved the common http and messaging types we use for builtins into
   `wasmcloud-core` behind feature flags that aren't on by default. As such,
   this is a breaking change as I moved stuff around, but mostly didn't
   change any code.
   
   Please note that I only bumped version numbers on things we had released
   already. Some of these crates had not yet been released and do not need
   another bump
 - <csr-id-47775f0da33b36f9b2707df63c416a4edc51caf6/> remove functionality from host (moved to core)
 - <csr-id-1931aba6d2bf46967eb6f7b66fdffde96a10ae4d/> use result for changed()
 - <csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/> rename lattice prefix to just lattice
 - <csr-id-642874717b6aab760d4692f9e8b12803548314e2/> make content_length a required field

### Commit Statistics

<csr-read-only-do-not-edit/>

 - 487 commits contributed to the release over the course of 763 calendar days.
 - 1475 days passed between releases.
 - 481 commits were understood as [conventional](https://www.conventionalcommits.org).
 - 0 issues like '(#ID)' were seen in commit messages

### Commit Details

<csr-read-only-do-not-edit/>

<details><summary>view details</summary>

 * **Uncategorized**
    - Reference wasmcloud_core::rpc helper methods consistently for provider subjects ([`467e7ca`](https://github.com/wasmCloud/wasmCloud/commit/467e7ca03142ee926b5f4aa4cf6d8a0e3a69eb46))
    - Remove TODOs scattered in code ([`ee7d4bb`](https://github.com/wasmCloud/wasmCloud/commit/ee7d4bbca9b0d07847b0e08af6251c6b4ee5ea2f))
    - Embed README, fix broken doc comment links ([`1b357dd`](https://github.com/wasmCloud/wasmCloud/commit/1b357dd07183ce673f9ac4af97aef40cb9c3cee1))
    - Implement traits for extending host functionality ([`2e02f10`](https://github.com/wasmCloud/wasmCloud/commit/2e02f107a6cd6a10c5df59228f61b7ed96027490))
    - Add names to nats connections ([`bf2d6e6`](https://github.com/wasmCloud/wasmCloud/commit/bf2d6e6033ca2fe631c0eab57d50c480787b693b))
    - Track max component instances ([`f9c1131`](https://github.com/wasmCloud/wasmCloud/commit/f9c1131f7aa06542ab23059a2bbeda9fe6a7cc12))
    - Track active component instances ([`7e59529`](https://github.com/wasmCloud/wasmCloud/commit/7e59529c7942a5d4616323bbf35970f0c3d8bea1))
    - Support max core instances per component ([`a6d01ec`](https://github.com/wasmCloud/wasmCloud/commit/a6d01eca1eebc2e18291d483e29452a73bd20f13))
    - Ignore auctions for builtin providers when disabled ([`8cc957a`](https://github.com/wasmCloud/wasmCloud/commit/8cc957a4ee878b3ccc85cf3e7bddcbf5c3ab376a))
    - Add convenience fn for returning server err ([`3aec234`](https://github.com/wasmCloud/wasmCloud/commit/3aec2348c4f59d5a667e344b7f06e86673f67da3))
    - Check for host/component mismatches in http host router ([`2acf13b`](https://github.com/wasmCloud/wasmCloud/commit/2acf13b67e9e6f91598922c522619dfe24d3fb74))
    - Add builtin http host routing ([`f66049d`](https://github.com/wasmCloud/wasmCloud/commit/f66049dc5072b92cc427111c3b1c0acb6b12aa0f))
    - Disallow processing of 'hostcore.*' label puts ([`1ed2c24`](https://github.com/wasmCloud/wasmCloud/commit/1ed2c24bcbe77702855aa0525633dee948bf05f2))
    - Add `wasmcloud:bus/error.error` ([`6061eab`](https://github.com/wasmCloud/wasmCloud/commit/6061eabcd60c56de88ed275aa1b988afec9a426a))
    - Warn on large ctl respose payload ([`27efae8`](https://github.com/wasmCloud/wasmCloud/commit/27efae8904182ab43e7a93afd52f56280d360bf7))
    - Warn on large event payload ([`06ecdca`](https://github.com/wasmCloud/wasmCloud/commit/06ecdcae83ba1e5886a7b102a0534fcaf694e653))
    - Bump patch versions of tracing and host ([`68c048f`](https://github.com/wasmCloud/wasmCloud/commit/68c048f8ac90efe33805fe019cdd90d43bd9b538))
    - Adds support for resource attributes ([`129fd42`](https://github.com/wasmCloud/wasmCloud/commit/129fd42074ce46f260798840539a12c585fea893))
    - Removes dependencies from host on provider libraries ([`c36dee9`](https://github.com/wasmCloud/wasmCloud/commit/c36dee94832d111c2a3ba5ff9f5e26baf2f3e4d9))
    - Add tests for parse_selectors_from_host_labels ([`0b03432`](https://github.com/wasmCloud/wasmCloud/commit/0b034328ebbbccfb596f7a41d0d60412e491aa22))
    - Address feedback ([`edb544e`](https://github.com/wasmCloud/wasmCloud/commit/edb544ef42f24995f7a823cd209d299af409d46c))
    - Mark host-label parsing code as unix-only ([`670c43d`](https://github.com/wasmCloud/wasmCloud/commit/670c43d1cdd9df97b3b765196734fec4f4b1d239))
    - Add wasmcloud:identity interface for fetching component workload identity ([`e9e32ce`](https://github.com/wasmCloud/wasmCloud/commit/e9e32ced1cdc7d00e13bd268ddb78388fb03d1a0))
    - Rename workload-identity feature to workload-identity-auth to better represent its intended purpose ([`ae33ae1`](https://github.com/wasmCloud/wasmCloud/commit/ae33ae1d3ebdf44bef23a19362d89274f9d57212))
    - Bump provider-archive v0.16.0, wasmcloud-core v0.17.0, wasmcloud-tracing v0.13.0, wasmcloud-provider-sdk v0.14.0, wasmcloud-provider-http-server v0.27.0, wasmcloud-provider-messaging-nats v0.26.0, wasmcloud-runtime v0.9.0, wasmcloud-secrets-types v0.6.0, wasmcloud-secrets-client v0.7.0, wasmcloud-host v0.25.0, wasmcloud-test-util v0.17.0, secrets-nats-kv v0.2.0, wash v0.41.0 ([`3078c88`](https://github.com/wasmCloud/wasmCloud/commit/3078c88f0ebed96027e20997bccc1c125583fad4))
    - Match non-workload identity first ([`5a97c3a`](https://github.com/wasmCloud/wasmCloud/commit/5a97c3abc50e5289fb76af764f0d82983b4962df))
    - Updates dependencies ([`5f7e013`](https://github.com/wasmCloud/wasmCloud/commit/5f7e0132362f7eda0710a1a69d5944140fd74b07))
    - Address feedback ([`383fb22`](https://github.com/wasmCloud/wasmCloud/commit/383fb22ede84002851081ba21f760b35cf9a2263))
    - Update workload identity to be unix-only feature ([`015bb52`](https://github.com/wasmCloud/wasmCloud/commit/015bb52602f68a76d4cea2e666d19a75df7d9aa8))
    - Add workload identity integration test ([`9d9d1c5`](https://github.com/wasmCloud/wasmCloud/commit/9d9d1c52b260f8fa66140ca9951893b482363a8a))
    - Match method and variable naming from suggestions ([`d54eb0f`](https://github.com/wasmCloud/wasmCloud/commit/d54eb0f035a4269ea163dbb5c3282a613f8e78e4))
    - Apply suggestions from code review ([`be4391e`](https://github.com/wasmCloud/wasmCloud/commit/be4391e35031a395c252bf036038a8093c14e626))
    - Remove the use of channels for passing SVIDs around ([`ec331af`](https://github.com/wasmCloud/wasmCloud/commit/ec331af4d90eb8d369a5f5de51afcf8a45f476a3))
    - Add experimental support for workload identity ([`441bdfb`](https://github.com/wasmCloud/wasmCloud/commit/441bdfb0ab40cb82a2a2922a094f9b1fd19923a5))
    - Makes sure to only refresh data ones ([`c670a16`](https://github.com/wasmCloud/wasmCloud/commit/c670a164afed54c56c9cfa81efb6adbf0865564a))
    - More host metrics & attributes ([`b1e656f`](https://github.com/wasmCloud/wasmCloud/commit/b1e656f8380ded7d7461fa01e403c3f145f79b54))
    - Add wasi:keyvalue/watch handler and redis provider ([`ca530c7`](https://github.com/wasmCloud/wasmCloud/commit/ca530c77e94d755f3e4dff9e7d486dd6b1f7c2e7))
    - Bump v0.24.1 ([`3d0c7ea`](https://github.com/wasmCloud/wasmCloud/commit/3d0c7ea40fba0b7f93357a0ad060eb3594872843))
    - Use additional cas to pull providers ([`f7744ab`](https://github.com/wasmCloud/wasmCloud/commit/f7744ab9a93c6e0f525e25c42ac0838750722b9e))
    - Simplify shutdown binary provider usage ([`e748d2d`](https://github.com/wasmCloud/wasmCloud/commit/e748d2df9303251a6ba88677164cc720ad1bdf2b))
    - Interpolate tracing logs for provider restarts ([`f85c6be`](https://github.com/wasmCloud/wasmCloud/commit/f85c6be0e355cf0e8284865122d0be35a9de728c))
    - Clean up impl of provider restart ([`c5d0f25`](https://github.com/wasmCloud/wasmCloud/commit/c5d0f25ef7eec75efcd98332f1eef66d4d955524))
    - Refresh config on provider restart ([`388c3f4`](https://github.com/wasmCloud/wasmCloud/commit/388c3f48e1141c2e927b4034943f72b63c9378ea))
    - Support pausing provider restarts during shutdown ([`a89b3c5`](https://github.com/wasmCloud/wasmCloud/commit/a89b3c54ef01ea9756dc83b8a6bc178a035a37c3))
    - Use futures for binary provider tasks ([`9e40f22`](https://github.com/wasmCloud/wasmCloud/commit/9e40f22614611ef3930359d66f3d55cfe8fb01b7))
    - Supervise and restart providers on exit ([`30281b3`](https://github.com/wasmCloud/wasmCloud/commit/30281b33a5d7d75292b421954938d9602ecb9665))
    - Remove unused parameters ([`40aa7cb`](https://github.com/wasmCloud/wasmCloud/commit/40aa7cb6ea87aee91aeaf5c1f5edebe3fbfb2a65))
    - Create claims and jetstream modules ([`d6d474f`](https://github.com/wasmCloud/wasmCloud/commit/d6d474f1393c2f1d7c6f29ea18c02a3a1f132ea6))
    - Implement ControlInterfaceServer trait ([`ae761a0`](https://github.com/wasmCloud/wasmCloud/commit/ae761a00f323f17d1fe9728eaeb10470d0b7bb93))
    - Fix spelling ([`6659528`](https://github.com/wasmCloud/wasmCloud/commit/6659528a4531f8d8024785296a36874b7e409f31))
    - Nbf/exp in cloudevents ([`d7205f1`](https://github.com/wasmCloud/wasmCloud/commit/d7205f1a59cbde3270669d3b9c2ced06edd4b8ab))
    - Output host_id on provider_start_failed ([`350fa8f`](https://github.com/wasmCloud/wasmCloud/commit/350fa8f6b796ba6ba302ce9bd2094798ef165bf2))
    - Improve error message ([`b46467d`](https://github.com/wasmCloud/wasmCloud/commit/b46467dbd662e0c5e277fdb4349369a22b4f7f67))
    - Improve flow ([`dfd66aa`](https://github.com/wasmCloud/wasmCloud/commit/dfd66aa879ef42b3a0cfc6c2242c60101122b76c))
    - Cargo fmt + clippy ([`fd2572d`](https://github.com/wasmCloud/wasmCloud/commit/fd2572d205158fca69fa8afbe4b2f70f5e3651d5))
    - Components can not be deleted when wasm file can not be found: #2936 ([`9d82e3c`](https://github.com/wasmCloud/wasmCloud/commit/9d82e3cff7703006da3b4de1a8b66e37e3d95e21))
    - Remove redundant variables ([`faa9133`](https://github.com/wasmCloud/wasmCloud/commit/faa91332c3fc43087080100688f30be4120bf8bd))
    - Use fewer extra closure vars ([`4f34e06`](https://github.com/wasmCloud/wasmCloud/commit/4f34e06be4f2f6267e55fb8333f3695243ee2e7a))
    - Reintroduce joinset for handles ([`221eab6`](https://github.com/wasmCloud/wasmCloud/commit/221eab623e4e94bf1e337366e0155453ffa64670))
    - Add http-server path integration test ([`bf82eb4`](https://github.com/wasmCloud/wasmCloud/commit/bf82eb498ab5fee4e8dd0cec937facadf92558dc))
    - Add builtin path http-server tests ([`42daec5`](https://github.com/wasmCloud/wasmCloud/commit/42daec5d0ab31db9871e3c99aed15eb2171c6387))
    - Implement builtin path based routing ([`a0e41b6`](https://github.com/wasmCloud/wasmCloud/commit/a0e41b67f4aee71ccf14b31e222f0ec58196e2be))
    - Http_server to explicit address file ([`54f9be5`](https://github.com/wasmCloud/wasmCloud/commit/54f9be522be602922b44858e56e0b1b61ecf313a))
    - Bump wasmcloud-core v0.16.0, wash-lib v0.32.0, wash-cli v0.38.0, safety bump 6 crates ([`4f30198`](https://github.com/wasmCloud/wasmCloud/commit/4f30198215220b3f9ce0c2aa6da8aa7d31a6a72d))
    - Be more compliant in HTTP admin ([`b1b03bf`](https://github.com/wasmCloud/wasmCloud/commit/b1b03bfd5669fe5a7de2bd5b134d61c91e8d8bd0))
    - Clarify log statements for HTTP admin ([`2697a2c`](https://github.com/wasmCloud/wasmCloud/commit/2697a2cc8a3477ddb03f602abbf54c221515e006))
    - Default to `status: ok` in `/readyz` ([`d3df101`](https://github.com/wasmCloud/wasmCloud/commit/d3df101596f8df6913f4bc07cc1395a2e361f09c))
    - Allow deletion of labels without providing value ([`27266a0`](https://github.com/wasmCloud/wasmCloud/commit/27266a01c561b48d6c85a7a5299fbc6ac2c6144d))
    - Allow disabling auction participation support ([`b6eb0a8`](https://github.com/wasmCloud/wasmCloud/commit/b6eb0a8640c611948a051133370647c519748f64))
    - Upgrade opentelemetry libraries to 0.27 ([`97c4360`](https://github.com/wasmCloud/wasmCloud/commit/97c436019740568f22ad8e4ff633fcd3f70260dc))
    - Specify type for ambiguous as_ref ([`3de0c70`](https://github.com/wasmCloud/wasmCloud/commit/3de0c70e1271e57f6e84a4eaa591343abe0f5bf2))
    - Test linking to different interface targets ([`450314e`](https://github.com/wasmCloud/wasmCloud/commit/450314effb4676207df048a2478f22cd0cd6455b))
    - Allow unique interfaces across the same package ([`fdf3992`](https://github.com/wasmCloud/wasmCloud/commit/fdf399245df2feef7d7a372aaa332546d9ebef51))
    - Gate messaging@v3 behind feature flag ([`febf47d`](https://github.com/wasmCloud/wasmCloud/commit/febf47d0ed7d70a245006710960a1a3bd695e6bf))
    - Consistently name features ([`52ed8be`](https://github.com/wasmCloud/wasmCloud/commit/52ed8befa81bab75817394e1136c196a962f8ca9))
    - Implement health check API ([`85ede3c`](https://github.com/wasmCloud/wasmCloud/commit/85ede3c9de6259908ad36225099c78e15364fa6f))
    - Add JetStream consumer support ([`2772b1b`](https://github.com/wasmCloud/wasmCloud/commit/2772b1b3025b579fa016b86c472330bc756706af))
    - Ignore `content-type` values with a `warn` log ([`7cf33ec`](https://github.com/wasmCloud/wasmCloud/commit/7cf33ece6eb6ff9fd46043402096e51f9ca1ac60))
    - Document `Message` ([`185c5d6`](https://github.com/wasmCloud/wasmCloud/commit/185c5d686ef2d8a058395936ac564984fd2a5d7a))
    - Implement `wasmcloud:messaging@0.3.0` ([`8afa8ff`](https://github.com/wasmCloud/wasmCloud/commit/8afa8ff95db21c93d3b280edc39d477dc8a2aaac))
    - Add `wasmcloud:messaging@0.3.0` dependency ([`b7bdfed`](https://github.com/wasmCloud/wasmCloud/commit/b7bdfed2f043c6fbcf17ef6588c6b099057014a2))
    - Reword feature methods ([`30ba77a`](https://github.com/wasmCloud/wasmCloud/commit/30ba77a2253122af3d85e1bce89d70de66cdb283))
    - Enable experimental features in test host ([`2f157ba`](https://github.com/wasmCloud/wasmCloud/commit/2f157ba0aa0d784663d24682361eadb8e7796f97))
    - Gate builtins with feature flags ([`0edd6d7`](https://github.com/wasmCloud/wasmCloud/commit/0edd6d739e97141cd5a7efda2809d9be5341111d))
    - Trace builtin providers ([`2aa0240`](https://github.com/wasmCloud/wasmCloud/commit/2aa024056642683fbb6c3d93a87bb3e680b3237e))
    - Propagate outgoing context via store not handler ([`d1feac0`](https://github.com/wasmCloud/wasmCloud/commit/d1feac0b8abb8cb8e2eec665b4d9b38ec8cd6d7f))
    - Connect function trace to context ([`7413aeb`](https://github.com/wasmCloud/wasmCloud/commit/7413aeb39e7f3b2e7550cdefafe982d6ffbe2da6))
    - Propagate serve trace into handlers ([`c5c6be9`](https://github.com/wasmCloud/wasmCloud/commit/c5c6be96a3fe6dd3b7264ed7acf6b105404652be))
    - Account for `max_instances` in builtin providers ([`d70329d`](https://github.com/wasmCloud/wasmCloud/commit/d70329d15042da16d9cfa8ae2bdcea76ded2784e))
    - Handle incoming NATS messages concurrently ([`5200f4d`](https://github.com/wasmCloud/wasmCloud/commit/5200f4d86cd6f58f7c571765555add944fe8ecd0))
    - Introduce `providers` module ([`aa57a6a`](https://github.com/wasmCloud/wasmCloud/commit/aa57a6a455cf772b08a774cfed55fc847671a05b))
    - Add metrics to builtin providers ([`13a4de8`](https://github.com/wasmCloud/wasmCloud/commit/13a4de8e5fe7d6b3641269e10a9aae2e8269164f))
    - Add builtin `wasmcloud:messaging`, HTTP server ([`45fa6b0`](https://github.com/wasmCloud/wasmCloud/commit/45fa6b06216da820045e4d39838a6ac3bdf97075))
    - Remove export joinset ([`cc3f219`](https://github.com/wasmCloud/wasmCloud/commit/cc3f219a4cbce29e322074284e4e8574c7f5b36a))
    - Adds the ability to configure sampling and buffer size ([`f051d9d`](https://github.com/wasmCloud/wasmCloud/commit/f051d9d3b8afb0ea1baafd87941babb29830fe4d))
    - Removed unused dependencies ([`eb52eca`](https://github.com/wasmCloud/wasmCloud/commit/eb52eca817fe24b33e7f1a65c1ba5c46c50bef4e))
    - Fixes orphaned traces ([`acd6143`](https://github.com/wasmCloud/wasmCloud/commit/acd6143f35ad25edf094e93d24118e2a8f13e1d8))
    - Bump wascap v0.15.2, provider-archive v0.14.0, wasmcloud-core v0.15.0, wash-lib v0.31.0, wasmcloud-tracing v0.11.0, wasmcloud-provider-sdk v0.12.0, wasmcloud-secrets-types v0.5.0, wash-cli v0.37.0, safety bump 9 crates ([`ef45f59`](https://github.com/wasmCloud/wasmCloud/commit/ef45f597710929d41be989110fc3c51621c9ee62))
    - Update `wrpc-transport-nats` ([`6d250ff`](https://github.com/wasmCloud/wasmCloud/commit/6d250ffa473385baae59b6f83b35ff38f119c054))
    - Convert wasi-logging levels into more conventional format ([`68ea303`](https://github.com/wasmCloud/wasmCloud/commit/68ea303e2cb3a3bbfd6878bcc1884f4951ed693d))
    - End epoch interruption, don't await ([`44e1d0e`](https://github.com/wasmCloud/wasmCloud/commit/44e1d0ef5acd63fbb08f5887557d39ce74680378))
    - Bump wascap v0.15.1, wasmcloud-core v0.13.0, wash-lib v0.29.0, wasmcloud-tracing v0.10.0, wasmcloud-provider-sdk v0.11.0, wash-cli v0.36.0, safety bump 7 crates ([`c5ba85c`](https://github.com/wasmCloud/wasmCloud/commit/c5ba85cfe6ad63227445b0a5e21d58a8f3e15e33))
    - Warn if config sender is dropped ([`bb53c9d`](https://github.com/wasmCloud/wasmCloud/commit/bb53c9dfd62d8b886b3296901a5ce6716551304d))
    - Updates tests and examples to support the new wkg deps ([`f0f3fd7`](https://github.com/wasmCloud/wasmCloud/commit/f0f3fd7011724137e5f8a4c47a8e4e97be0edbb2))
    - Ensure config sender is not dropped ([`7363191`](https://github.com/wasmCloud/wasmCloud/commit/73631912ec94438fb527961dbb40a1e519f1d44b))
    - Add lattice@1.0.0 and @2.0.0 implementations ([`a7015f8`](https://github.com/wasmCloud/wasmCloud/commit/a7015f83af4d8d46284d3f49398ffa22876f290d))
    - Minor changes, remove print ([`53a7eb5`](https://github.com/wasmCloud/wasmCloud/commit/53a7eb5bf13dd986fe9435a8444d1ec4f09cd329))
    - Add checked set_link_name ([`5f05bcc`](https://github.com/wasmCloud/wasmCloud/commit/5f05bcc468b3e67e67a22c666d93176b44164fbc))
    - Add support for WASI 0.2.2 ([`b8c2b99`](https://github.com/wasmCloud/wasmCloud/commit/b8c2b999c979cd82f6772f3f98ec1d16c7f5565d))
    - Bump wasmcloud-core v0.12.0, wash-lib v0.28.0, wasmcloud-tracing v0.9.0, wasmcloud-provider-sdk v0.10.0, wash-cli v0.35.0, safety bump 7 crates ([`44bf4c8`](https://github.com/wasmCloud/wasmCloud/commit/44bf4c8793b3989aebbbc28c2f2ce3ebbd4d6a0a))
    - Update for control interface v2.2.0 ([`a952c77`](https://github.com/wasmCloud/wasmCloud/commit/a952c771164066715dee66ef824c9df662e0cf58))
    - Update usage of provider description ([`74dd7fc`](https://github.com/wasmCloud/wasmCloud/commit/74dd7fc104c359213ad8330aee62a82c2dbe2629))
    - Wasmcloud-host 0.20.0 ([`2c33561`](https://github.com/wasmCloud/wasmCloud/commit/2c335617918a20561ed33259e54b2451ad534343))
    - Update host w/ new ctrl iface ([`f418ad9`](https://github.com/wasmCloud/wasmCloud/commit/f418ad9c826e6ed6661175cf883882a37d5af1eb))
    - Rework `wasi:http` error handling ([`4da0105`](https://github.com/wasmCloud/wasmCloud/commit/4da0105ac7bf463eeb79bc3047cb5e92664f8a7c))
    - Log handling errors ([`726dd68`](https://github.com/wasmCloud/wasmCloud/commit/726dd689e6d64eb44930834425d69f21cefc61cd))
    - Switch to using oci feature ([`fbd1dd1`](https://github.com/wasmCloud/wasmCloud/commit/fbd1dd10a7c92a40a69c21b2cbba21c07ae8e893))
    - Sets a higher value for the incoming events channel ([`fc131ed`](https://github.com/wasmCloud/wasmCloud/commit/fc131edff75a7240fe519d8bbc4b08ac31d9bf1c))
    - Fix clippy lints ([`fa01304`](https://github.com/wasmCloud/wasmCloud/commit/fa01304b62e349be3ac3cf00aa43c2f5ead93dd5))
    - Remove missing caching code ([`d511d74`](https://github.com/wasmCloud/wasmCloud/commit/d511d74c21ab96f5913f5546e8253f34c73642a1))
    - Remove unused deps ([`ac18892`](https://github.com/wasmCloud/wasmCloud/commit/ac188921856c9b5fe669531e309f3f416d1bb757))
    - Remove functionality from host (moved to core) ([`47775f0`](https://github.com/wasmCloud/wasmCloud/commit/47775f0da33b36f9b2707df63c416a4edc51caf6))
    - Move functionality into core ([`0547e3a`](https://github.com/wasmCloud/wasmCloud/commit/0547e3a429059b15ec969a0fa36d7823a6b7331f))
    - Adds support for batch support to the host ([`8575f73`](https://github.com/wasmCloud/wasmCloud/commit/8575f732df33ca973ff340fc3e4bc7fbfeaf89f3))
    - Remove provider caching for local file references ([`991cb21`](https://github.com/wasmCloud/wasmCloud/commit/991cb21d5bceee681c613b314a9d2dfaeee890ee))
    - Add links integration test ([`9ac2e29`](https://github.com/wasmCloud/wasmCloud/commit/9ac2e29babcaa3e9789c42d05d9d3ad4ccd5fcc7))
    - Ensure links are unique on source+interface+name ([`8db14d3`](https://github.com/wasmCloud/wasmCloud/commit/8db14d3bb320e6732c62c3abfe936d72e45fe734))
    - Set missing link log to warn ([`d21d2a9`](https://github.com/wasmCloud/wasmCloud/commit/d21d2a9e7dffd16315eeb565e2cd0e1f1aeeac6c))
    - Fallback to `wrpc:blobstore@0.1.0` ([`26d7f64`](https://github.com/wasmCloud/wasmCloud/commit/26d7f64659dbf3263f36da92df89003c579077cc))
    - Introduce linkdef_set_failed event ([`be7233e`](https://github.com/wasmCloud/wasmCloud/commit/be7233e730dce14578651a17d16410d7a7dbe91c))
    - Abort health check tasks on provider drop ([`47e80cf`](https://github.com/wasmCloud/wasmCloud/commit/47e80cf949a2cb287be479653336def31c130ba2))
    - Use result for changed() ([`1931aba`](https://github.com/wasmCloud/wasmCloud/commit/1931aba6d2bf46967eb6f7b66fdffde96a10ae4d))
    - Prevent Provider ConfigBundle early drop, do cleanup ([`77b1af9`](https://github.com/wasmCloud/wasmCloud/commit/77b1af98c1cdfdb5425a590856c0e27f2a7e805f))
    - Support for sending out config updates to providers ([`6164132`](https://github.com/wasmCloud/wasmCloud/commit/61641322dec02dd835e81b51de72cbd1007d13cf))
    - Allow tuning runtime parameters ([`f4b4eeb`](https://github.com/wasmCloud/wasmCloud/commit/f4b4eeb64a6eab4f6dfb540eacd7e2256d80aa71))
    - Make wasmcloud host heartbeat interval configurable ([`40e5edf`](https://github.com/wasmCloud/wasmCloud/commit/40e5edfc0ee48fadccd0f0fb8f8d0eb53db026f0))
    - Always publish component_scale event ([`76265cd`](https://github.com/wasmCloud/wasmCloud/commit/76265cdcbf2959f87961340576e71e085f1f4942))
    - Update component/runtime/host crate READMEs ([`51c8ceb`](https://github.com/wasmCloud/wasmCloud/commit/51c8ceb895b0069af9671e895b9f1ecb841ea6c3))
    - Bump for test-util release ([`7cd2e71`](https://github.com/wasmCloud/wasmCloud/commit/7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4))
    - Clarify missing secret config error ([`da461ed`](https://github.com/wasmCloud/wasmCloud/commit/da461edd4e5ede0220cb9923b1d9a62808f560dc))
    - Fix clippy lint ([`f36471d`](https://github.com/wasmCloud/wasmCloud/commit/f36471d7620fd66ff642518ae96188fef6fde5e0))
    - Include name with secret config ([`c666ef5`](https://github.com/wasmCloud/wasmCloud/commit/c666ef50fecc1ee248bf78d486a915ee077e3b4a))
    - Move SecretConfig into crate ([`2ea22a2`](https://github.com/wasmCloud/wasmCloud/commit/2ea22a28ca9fd1838fc03451f33d75690fc28f2a))
    - Address feedback, application name optional ([`b56982f`](https://github.com/wasmCloud/wasmCloud/commit/b56982f437209ecaff4fa6946f8fe4c3068a62cd))
    - Collapse application field ([`388662a`](https://github.com/wasmCloud/wasmCloud/commit/388662a482442df3f74dfe8f9559fc4c07cedbe5))
    - Pass policy string directly to backend ([`2695ad3`](https://github.com/wasmCloud/wasmCloud/commit/2695ad38f3338567de06f6a7ebc719a9421db7eb))
    - Use name instead of key for secret map ([`1914c34`](https://github.com/wasmCloud/wasmCloud/commit/1914c34317b673f3b7208863ba107c579700a133))
    - Update secrets integration to use the update config structure ([`da879d3`](https://github.com/wasmCloud/wasmCloud/commit/da879d3e50d32fe1c09edcf2b58cb2db9c9e2661))
    - Skip backwards-compat link with secret ([`5506c8b`](https://github.com/wasmCloud/wasmCloud/commit/5506c8b6eb78d8e4b793748072c4f026a4ed1863))
    - Efficiency, pass optional vec secrets ([`cfbf232`](https://github.com/wasmCloud/wasmCloud/commit/cfbf23226f34f3e7245a5d36cd7bb15e1796850c))
    - Improve error representations, cleanup ([`d8ad437`](https://github.com/wasmCloud/wasmCloud/commit/d8ad4376cb4db282047de8c4f62f6b8b907c9356))
    - Encrypt link secrets, generate xkeys for providers ([`d9281e2`](https://github.com/wasmCloud/wasmCloud/commit/d9281e2d54ac72e94f9afb61b3167690fe1fd89b))
    - Add Host level configurability for max_execution_time by flag and env variables ([`a570a35`](https://github.com/wasmCloud/wasmCloud/commit/a570a3565e129fc13b437327eb1ba18835c69f57))
    - Light refactor from followup ([`f354008`](https://github.com/wasmCloud/wasmCloud/commit/f354008c318f49565eb023a91cd3a3781d73c36a))
    - Remove extra trace_level field ([`4e1d6da`](https://github.com/wasmCloud/wasmCloud/commit/4e1d6da189ff49790d876cd244aed89114efba98))
    - Configure observability with trace_level option ([`2378057`](https://github.com/wasmCloud/wasmCloud/commit/2378057bbbabbfa5a2159b6621d6009396411dd7))
    - Add support for supplying additional CA certificates to OCI and OpenTelemetry clients ([`24e77b7`](https://github.com/wasmCloud/wasmCloud/commit/24e77b7f1f29580ca348a758302cdc6e75cc3afd))
    - Improve error handling ([`7f4cd4c`](https://github.com/wasmCloud/wasmCloud/commit/7f4cd4cf5da514bb1d10c9d064bb905de8621d8e))
    - Check provided secrets topic for non-empty ([`5c68c89`](https://github.com/wasmCloud/wasmCloud/commit/5c68c898f8bd8351f5d16226480fbbe726efc163))
    - Improve error usage of bail ([`c30bf33`](https://github.com/wasmCloud/wasmCloud/commit/c30bf33f754c15122ead7f041b7d3e063dd1db33))
    - Fetch secrets for providers and links ([`e0324d6`](https://github.com/wasmCloud/wasmCloud/commit/e0324d66e49be015b7b231626bc3b619d9374c91))
    - Add secrets handler impl for strings ([`773780c`](https://github.com/wasmCloud/wasmCloud/commit/773780c59dc9af93b51abdf90a4f948ff2efb326))
    - Add secrecy ([`c71f153`](https://github.com/wasmCloud/wasmCloud/commit/c71f153b84e4ac4f84bdb934c9f7ca735eddd482))
    - Add secrets backend topic flag ([`98b3986`](https://github.com/wasmCloud/wasmCloud/commit/98b3986aca562d7f5439d3618d1eaf70f1b7e75a))
    - Set NATS queue group ([`c2bb9cb`](https://github.com/wasmCloud/wasmCloud/commit/c2bb9cb5e2ba1c6b055f6726e86ffc95dab90d2c))
    - Fix outdated `ctl_seed` reference ([`5225b19`](https://github.com/wasmCloud/wasmCloud/commit/5225b1961038b815fe98c5250278d1aa483bdded))
    - Enable `ring` feature for `async-nats` ([`81ab591`](https://github.com/wasmCloud/wasmCloud/commit/81ab5914e7d08740eb9371c9b718f13f0419c23f))
    - Address clippy warnings ([`bd50166`](https://github.com/wasmCloud/wasmCloud/commit/bd50166619b8810ccdc2bcd80c33ff80d94bc909))
    - Update to stream-based serving ([`0f70936`](https://github.com/wasmCloud/wasmCloud/commit/0f7093660a1ef09ff745daf5e1a96fd72c88984d))
    - Conflate `wasi:blobstore/container` interface with `blobstore` ([`659cb2e`](https://github.com/wasmCloud/wasmCloud/commit/659cb2eace33962e3ed05d69402607233b33a951))
    - Pass original component instance through the context ([`0707512`](https://github.com/wasmCloud/wasmCloud/commit/070751231e5bb4891b995e992e5206b3050ecc30))
    - Upgrade `wrpc`, `async-nats`, `wasmtime` ([`9cb1b78`](https://github.com/wasmCloud/wasmCloud/commit/9cb1b784fe7a8892d73bdb40d1172b1879fcd932))
    - Support ScaleComponentCommand w/ update ([`ed4b846`](https://github.com/wasmCloud/wasmCloud/commit/ed4b84661c08e43eadfce426474a49ad813ea6ec))
    - Improve error messages for missing links ([`e7c3040`](https://github.com/wasmCloud/wasmCloud/commit/e7c30405302fcccc612209335179f0bc47d8e996))
    - Improve error messages for missing links ([`20a7259`](https://github.com/wasmCloud/wasmCloud/commit/20a72597d17db8fcf0c70a7e9172edadcaad5b22))
    - Allow empty payloads to trigger stop_host ([`e17fe93`](https://github.com/wasmCloud/wasmCloud/commit/e17fe933ffdc9b4e6938c4a0f2943c4813b658b1))
    - Add link name to wRPC invocations ([`a0a1b8c`](https://github.com/wasmCloud/wasmCloud/commit/a0a1b8c0c3d82feb19f42c4faa6de96b99bac13f))
    - Downgrade link/claims log/trace ([`d9a8c62`](https://github.com/wasmCloud/wasmCloud/commit/d9a8c62d6fce6e71edadcf7de78cac749cf58126))
    - Setup debug traces ([`b014263`](https://github.com/wasmCloud/wasmCloud/commit/b014263cf3614995f597336bb40e51ab72bfa1c9))
    - Propagate traces through components ([`fa1fde1`](https://github.com/wasmCloud/wasmCloud/commit/fa1fde185b47b055e511f6f2dee095e269db1651))
    - Add support for configuring grpc protocol with opentelemetry ([`378b7c8`](https://github.com/wasmCloud/wasmCloud/commit/378b7c89c8b00a5dcee76c06bc8de615dc58f8aa))
    - Replace actor references by component in crates ([`20c72ce`](https://github.com/wasmCloud/wasmCloud/commit/20c72ce0ed423561ae6dbd5a91959bec24ff7cf3))
    - Updates host to support new wasm artifact type ([`0aa01a9`](https://github.com/wasmCloud/wasmCloud/commit/0aa01a92925dc12203bf9f06e13d21b7812b77eb))
    - Bump oci-distribution to 0.11.0 ([`88c07bf`](https://github.com/wasmCloud/wasmCloud/commit/88c07bf3be18da4f4afac3e7e356ddc507a6d85e))
    - Gracefully shutdown epoch interrupt thread ([`077a28a`](https://github.com/wasmCloud/wasmCloud/commit/077a28a6567a436c99368c7eb1bd5dd2a6bc6103))
    - Generate changelogs after 1.0.1 release ([`4e0313a`](https://github.com/wasmCloud/wasmCloud/commit/4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e))
    - Updated with newest features ([`0f03f1f`](https://github.com/wasmCloud/wasmCloud/commit/0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6))
    - Generate crate changelogs ([`f986e39`](https://github.com/wasmCloud/wasmCloud/commit/f986e39450676dc598b92f13cb6e52b9c3200c0b))
    - Count epoch in a separate OS thread ([`3eb4534`](https://github.com/wasmCloud/wasmCloud/commit/3eb453405aa144599f43bbaf56197566c9f0cf0a))
    - Document invocation handling failures ([`c4b82f2`](https://github.com/wasmCloud/wasmCloud/commit/c4b82f28947f06253aa997ae65ab11ebcc507f49))
    - Handle invocation handling errors ([`3cabf10`](https://github.com/wasmCloud/wasmCloud/commit/3cabf109f5b986079cceb7f125f75bf53348712e))
    - Handle invocations in tasks ([`b8c3434`](https://github.com/wasmCloud/wasmCloud/commit/b8c34346137edf5492fe70abeb22336a33e85bc0))
    - Remove redundant handler clone ([`1610702`](https://github.com/wasmCloud/wasmCloud/commit/1610702ad0f8cd3ba221c1b6b8ba2ce8fe57c6ae))
    - Propagate `max_execution_time` to the runtime ([`a66921e`](https://github.com/wasmCloud/wasmCloud/commit/a66921edd9be3202d1296a165c34faf597b1dec1))
    - Comment ([`ec3bae5`](https://github.com/wasmCloud/wasmCloud/commit/ec3bae5c03c77a0b77884b84754e33e1a8361a89))
    - Check component update ref and respond with a message before task ([`45a3d3f`](https://github.com/wasmCloud/wasmCloud/commit/45a3d3f477b48e8a79e77880950bb785175a990d))
    - Check component image reference on component update ([`95081ca`](https://github.com/wasmCloud/wasmCloud/commit/95081cacfc3fc04911c91c32f462d643be2e12ed))
    - Limit max execution time to 10 minutes ([`e928020`](https://github.com/wasmCloud/wasmCloud/commit/e928020fd774abcc213fec560d89f128464da319))
    - Replace references to 'actor' with 'component' ([`e6dd0b2`](https://github.com/wasmCloud/wasmCloud/commit/e6dd0b2809510e785f4ee4c531f5666e6ab21998))
    - Update to Wasmtime 20 ([`33b50c2`](https://github.com/wasmCloud/wasmCloud/commit/33b50c2d258ca9744ed65b153a6580f893172e0c))
    - Remove unnecessary todo comments ([`bdb519f`](https://github.com/wasmCloud/wasmCloud/commit/bdb519f91125c3f32f60ad9e9d1ce7bc1f147dc4))
    - Expect stop_actor function parameter host_id to be unused ([`173bfa6`](https://github.com/wasmCloud/wasmCloud/commit/173bfa623328bd1790642ddd6d56c6f9e5b38831))
    - Remove publish_event from stop_actor ([`c87f3fe`](https://github.com/wasmCloud/wasmCloud/commit/c87f3fe2654d5c874708974915bdd65f69f4afe1))
    - Show provider ID on healthcheck failure messages ([`9f1b278`](https://github.com/wasmCloud/wasmCloud/commit/9f1b2787255cb106d98481019d26e3208c11fc9f))
    - Improve error message for forceful provider shutdown ([`863296d`](https://github.com/wasmCloud/wasmCloud/commit/863296d7db28ca4815820f8b9a96a63dfe626904))
    - Update URLs to `wrpc` org ([`e1ab91d`](https://github.com/wasmCloud/wasmCloud/commit/e1ab91d678d8191f28e2496a68e52c7b93ad90c3))
    - Address clippy warnings ([`5957fce`](https://github.com/wasmCloud/wasmCloud/commit/5957fce86a928c7398370547d0f43c9498185441))
    - Differentiate no config and config error ([`9542e16`](https://github.com/wasmCloud/wasmCloud/commit/9542e16b80c71dc7cc2f9e7175ebb25be050a242))
    - Allow overwriting provider reference ([`dcbbc84`](https://github.com/wasmCloud/wasmCloud/commit/dcbbc843c5a571e1c33775c66bbd3cd528b02c26))
    - Bumps host version to rc.2 ([`346753a`](https://github.com/wasmCloud/wasmCloud/commit/346753ab823f911b12de763225dfd154272f1d3a))
    - Update `wrpc:keyvalue` in providers ([`9cd2b40`](https://github.com/wasmCloud/wasmCloud/commit/9cd2b4034f8d5688ce250429dc14120eaf61b483))
    - Update `wasi:keyvalue` ([`a175419`](https://github.com/wasmCloud/wasmCloud/commit/a1754195fca5a13c8cdde713dad3e1a9765adaf5))
    - Remove cluster_seed/cluster_issuers ([`bc5d296`](https://github.com/wasmCloud/wasmCloud/commit/bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f))
    - Imrpove wording for spec/provider ref mismatch ([`e8aac21`](https://github.com/wasmCloud/wasmCloud/commit/e8aac21cbc094f87fb486a903eaab9a132a7ee07))
    - Warn scaling with different imageref ([`804d676`](https://github.com/wasmCloud/wasmCloud/commit/804d67665fac39c08a536b0902a65a85035e685e))
    - Switch to `wit-bindgen-wrpc` ([`322f471`](https://github.com/wasmCloud/wasmCloud/commit/322f471f9a8154224a50ec33517c9f5b1716d2d5))
    - Update `messaging` to `0.2.0` ([`955a689`](https://github.com/wasmCloud/wasmCloud/commit/955a6893792e86292883e76de57434616c28d380))
    - Rename scaled ID from actor to component ([`91c57b2`](https://github.com/wasmCloud/wasmCloud/commit/91c57b238c6e3aec5bd86f5c2103aaec21932725))
    - Don't clone targets with handlers ([`ef50f04`](https://github.com/wasmCloud/wasmCloud/commit/ef50f046ade176cabbf690de59caad5d4f99c78f))
    - Rename ctl actor to component ([`bcbb402`](https://github.com/wasmCloud/wasmCloud/commit/bcbb402c2efe3dc881b06e666c70e01e94d3ad72))
    - Removes old guest config and uses runtime config instead ([`6b2e1b5`](https://github.com/wasmCloud/wasmCloud/commit/6b2e1b5915a0e894a567622ffc193230e5654c1f))
    - Rename actor_scale* events ([`9e23be2`](https://github.com/wasmCloud/wasmCloud/commit/9e23be23131bbcdad746f7e85d33d5812e5f2ff9))
    - Deliver target links to started provider ([`2b500d7`](https://github.com/wasmCloud/wasmCloud/commit/2b500d7a38cb338620f9c7834ca7fef882e42c92))
    - Actor -> Component ([`ef1d3af`](https://github.com/wasmCloud/wasmCloud/commit/ef1d3af1ccddf33cdb37763101e3fb7577bf1433))
    - Remove capabilities ([`f34aac4`](https://github.com/wasmCloud/wasmCloud/commit/f34aac419d124aba6b6e252f85627847f67d01f4))
    - Adds version to control API ([`3f2d2f4`](https://github.com/wasmCloud/wasmCloud/commit/3f2d2f44470d44809fb83de2fa34b29ad1e6cb30))
    - Add label_changed event for label update/delete ([`dd0d449`](https://github.com/wasmCloud/wasmCloud/commit/dd0d449e5bfc3826675f3f744db44b3000c67197))
    - Use native TLS roots along webpki ([`07b5e70`](https://github.com/wasmCloud/wasmCloud/commit/07b5e70a7f1321d184962d7197a8d98d1ecaaf71))
    - Fix `link_name` functionality, reorganize tests ([`4ed3891`](https://github.com/wasmCloud/wasmCloud/commit/4ed38913f19fcd4dd44dfdcc9007e80e80cdc960))
    - Convert to `wrpc:blobstore` ([`91874e9`](https://github.com/wasmCloud/wasmCloud/commit/91874e9f4bf2b37b895a4654250203144e12815c))
    - Correct name and data streaming, update WIT ([`ccb3c73`](https://github.com/wasmCloud/wasmCloud/commit/ccb3c73dc1351b11233896abc068a200374df079))
    - Use `wasmcloud:messaging` in providers ([`716d251`](https://github.com/wasmCloud/wasmCloud/commit/716d251478cf174085f6ff274854ddebd9e0d772))
    - Recreates polyfill imports on update ([`5b4f75b`](https://github.com/wasmCloud/wasmCloud/commit/5b4f75b7b843483566983c72c3a25e91c3de3adc))
    - Fetch configuration direct from bucket ([`5c3dc96`](https://github.com/wasmCloud/wasmCloud/commit/5c3dc963783c71fc91ec916be64a6f67917d9740))
    - Fix deadlock and slow ack of update ([`fd85e25`](https://github.com/wasmCloud/wasmCloud/commit/fd85e254ee56abb65bee648ba0ea93b9a227a96f))
    - Flatten claims response payload ([`cab6fd2`](https://github.com/wasmCloud/wasmCloud/commit/cab6fd2cae47f0a866f17dfdb593a48a9210bab8))
    - Implement `wrpc:blobstore/blobstore` for FS ([`383b3f3`](https://github.com/wasmCloud/wasmCloud/commit/383b3f3067dddc913d5a0c052f7bbb9c47fe8663))
    - Implement Redis `wrpc:keyvalue/{atomic,eventual}` ([`614af7e`](https://github.com/wasmCloud/wasmCloud/commit/614af7e3ed734c56b27cd1d2aacb0789a85e8b81))
    - Implement `wasi:http/outgoing-handler` provider ([`e0dac9d`](https://github.com/wasmCloud/wasmCloud/commit/e0dac9de4d3a74424e3138971753db9da143db5a))
    - Remove compat crate ([`f2aed15`](https://github.com/wasmCloud/wasmCloud/commit/f2aed15288300989aca03f899b095d3a71f8e5cd))
    - Update `wrpc-interface-http` ([`c7a7ed7`](https://github.com/wasmCloud/wasmCloud/commit/c7a7ed73f5497f83a9dcfb509df580cdec3a4635))
    - Address clippy warnings ([`adb08b7`](https://github.com/wasmCloud/wasmCloud/commit/adb08b70ecc37ec14bb9b7eea41c8110696d9b98))
    - Race condition with initial config get ([`9fe1fe8`](https://github.com/wasmCloud/wasmCloud/commit/9fe1fe8ce8d4434fb05635d7d1ae6ee07bc188c3))
    - Deliver full config with link ([`e14d040`](https://github.com/wasmCloud/wasmCloud/commit/e14d0405e9f746041001e101fc24320c9e6b4f9c))
    - Update wRPC ([`95cfb6d`](https://github.com/wasmCloud/wasmCloud/commit/95cfb6d99f0e54243b2fb2618de39210d8f3694f))
    - Start providers with named config ([`5c1a0a5`](https://github.com/wasmCloud/wasmCloud/commit/5c1a0a57e761d405cdbb8ea4cbca0fe13b7e8737))
    - Refactor componentspec, deliver links to providers ([`188f096`](https://github.com/wasmCloud/wasmCloud/commit/188f0965e911067b5ffe9c62083fd4fbba2713f4))
    - Use `&str` directly ([`6b369d4`](https://github.com/wasmCloud/wasmCloud/commit/6b369d49cd37a87dca1f92f31c4d4d3e33dec501))
    - Use traces instead of tracing user-facing language to align with OTEL signal names ([`d65512b`](https://github.com/wasmCloud/wasmCloud/commit/d65512b5e86eb4d13e64cffa220a5a842c7bb72b))
    - Add flags for overriding the default OpenTelemetry endpoint ([`6fe14b8`](https://github.com/wasmCloud/wasmCloud/commit/6fe14b89d4c26e5c01e54773268c6d0f04236e71))
    - Switch to using --enable-observability and --enable-<signal> flags ([`868570b`](https://github.com/wasmCloud/wasmCloud/commit/868570be8d94a6d73608c7cde5d2422e15f9eb0c))
    - Refactor component invocation tracking ([`95a9d7d`](https://github.com/wasmCloud/wasmCloud/commit/95a9d7d3b8c6367df93b65a2e218315cc3ec42eb))
    - Improve target lookup error handling ([`149f98b`](https://github.com/wasmCloud/wasmCloud/commit/149f98b60c1e70d0e68153add3e30b8fb4483e11))
    - Update wrpc_client ([`ec84fad`](https://github.com/wasmCloud/wasmCloud/commit/ec84fadfd819f203fe2e4906f5338f48f6ddec78))
    - Move wasmcloud wrpc transport client to core ([`c654448`](https://github.com/wasmCloud/wasmCloud/commit/c654448653db224c6a676ecf43150d880a9daf8c))
    - Re-tag request type enum policy ([`152186f`](https://github.com/wasmCloud/wasmCloud/commit/152186f9940f6c9352ee5d9f91ddefe5673bdac1))
    - Support pubsub on wRPC subjects ([`76c1ed7`](https://github.com/wasmCloud/wasmCloud/commit/76c1ed7b5c49152aabd83d27f0b8955d7f874864))
    - Include actor_id on scaled events ([`abb81eb`](https://github.com/wasmCloud/wasmCloud/commit/abb81ebbf99ec3007b1d1d48a43cfe52d86bf3e7))
    - Cache request by unique data ([`df01397`](https://github.com/wasmCloud/wasmCloud/commit/df01397bce61344d3429aff081a9f9b23fad0b84))
    - Formalize policy service ([`1fb6266`](https://github.com/wasmCloud/wasmCloud/commit/1fb6266826f47221ec3f9413f54a4c395622dcbd))
    - Downgrade provider claims to optional metadata ([`be1e03c`](https://github.com/wasmCloud/wasmCloud/commit/be1e03c5281c9cf4b02fe5349a8cf5d0d7cd0892))
    - Downgrade actor claims to optional metadata ([`8afb61f`](https://github.com/wasmCloud/wasmCloud/commit/8afb61fb6592db6a24c53f248e4f445f9b2db580))
    - Glues in named config to actors ([`82c249b`](https://github.com/wasmCloud/wasmCloud/commit/82c249b15dba4dbe4c14a6afd2b52c7d3dc99985))
    - Update wRPC ([`a96b1f3`](https://github.com/wasmCloud/wasmCloud/commit/a96b1f370392063f403e9f25e0ef21c30fdcdfa9))
    - Implement AcceptorWithHeaders ([`1dc15a1`](https://github.com/wasmCloud/wasmCloud/commit/1dc15a127ac9830f3ebd21e61a1cf3db404eed6d))
    - Instrument handle_invocation and call ([`a6ec7c3`](https://github.com/wasmCloud/wasmCloud/commit/a6ec7c3476daf63dc6f53afb7eb512cfc3d2b9d8))
    - Implement wasmcloud_transport wrappers ([`fd50dcf`](https://github.com/wasmCloud/wasmCloud/commit/fd50dcfa07b759b01e32d7f974105615c8c47db4))
    - Fixes write lock issue on policy service ([`4aa31f7`](https://github.com/wasmCloud/wasmCloud/commit/4aa31f74bf84784af0207d2886f62d833dfe1b63))
    - Implement `wrpc:http/incoming-handler` ([`f2223a3`](https://github.com/wasmCloud/wasmCloud/commit/f2223a3f5378c3cebfec96b5322df619fcecc556))
    - Begin incoming wRPC invocation implementation ([`fedfd92`](https://github.com/wasmCloud/wasmCloud/commit/fedfd92dbba773af048fe19d956f4c3625cc17de))
    - Remove functionality related to wasmbus invocations ([`6784710`](https://github.com/wasmCloud/wasmCloud/commit/67847106d968a515ff89427454b7b14dfb486a3d))
    - Switch to `wrpc` for `wasmcloud:messaging` ([`0c0c004`](https://github.com/wasmCloud/wasmCloud/commit/0c0c004bafb60323018fc1c86cb13493f72d29cd))
    - Switch to `wrpc:{keyvalue,blobstore}` ([`5ede01b`](https://github.com/wasmCloud/wasmCloud/commit/5ede01b1fe0bc62234d2b7d6c72775d9e248a130))
    - Encode custom parameters as tuples ([`f3bc961`](https://github.com/wasmCloud/wasmCloud/commit/f3bc96128ed7033d08bc7da1ea7ba89c40880ede))
    - Implement `wrpc:http/outgoing-handler.handle` ([`2463845`](https://github.com/wasmCloud/wasmCloud/commit/246384524cfe65ce6742558425b885247b461c5c))
    - Correctly invoke custom functions ([`9e304cd`](https://github.com/wasmCloud/wasmCloud/commit/9e304cd7d19a2f7eef099703f168e8f155d4f8bc))
    - Update wRPC ([`49d8650`](https://github.com/wasmCloud/wasmCloud/commit/49d86501487f6811bb8b65641c40ab353f6e110d))
    - Wrap all ctl operations in CtlResponse ([`4a4b300`](https://github.com/wasmCloud/wasmCloud/commit/4a4b300515e9984a1befe6aaab1a6298d8ea49b1))
    - Consistent host operations ([`2798858`](https://github.com/wasmCloud/wasmCloud/commit/2798858880004225ebe49aa1d873019a02f29e49))
    - Revert incorrectly handled config conficts ([`e12ec1d`](https://github.com/wasmCloud/wasmCloud/commit/e12ec1d6655a9aa319236a8d62a53fd6521bd683))
    - Remove collection type aliases ([`e16da66`](https://github.com/wasmCloud/wasmCloud/commit/e16da6614ad9ae28e8c3e6ac3ebb36faf12cb4d1))
    - Remove plural actor events ([`9957ca7`](https://github.com/wasmCloud/wasmCloud/commit/9957ca7f8b21444b2d4e32f20a50b09f92a5b6ee))
    - Flatten instances on actor/providers ([`5275937`](https://github.com/wasmCloud/wasmCloud/commit/5275937c2c9b25139f3c208af7909889362df308))
    - Fully support interface links, remove aliases ([`48fc893`](https://github.com/wasmCloud/wasmCloud/commit/48fc893ba2de576511aeea98a3da4cc97024c53e))
    - Bindgen issues preventing builds ([`e9bea42`](https://github.com/wasmCloud/wasmCloud/commit/e9bea42ed6189d903ea7fc6b7d4dc54a6fe88a12))
    - Integrate set-link-name and wrpc ([`4f55396`](https://github.com/wasmCloud/wasmCloud/commit/4f55396a0340d65dbebdf6d4f0ca070d6f990fc4))
    - Support component invoking polyfilled functions ([`5173aa5`](https://github.com/wasmCloud/wasmCloud/commit/5173aa5e679ffe446f10aa549f1120f1bd1ab033))
    - Remove unused imports/functions ([`5990b00`](https://github.com/wasmCloud/wasmCloud/commit/5990b00ea49b1bfeac3ee913dc0a9188729abeff))
    - Support wrpc component exports ([`49e5943`](https://github.com/wasmCloud/wasmCloud/commit/49e5943d9a087b5ef5428f73281c36030d77502c))
    - Change set-target to set-link-name ([`5d19ba1`](https://github.com/wasmCloud/wasmCloud/commit/5d19ba16a98dca9439628e8449309ccaa763ab10))
    - Remove module support ([`fec6f5f`](https://github.com/wasmCloud/wasmCloud/commit/fec6f5f1372a1de5737f5ec585ad735e14c20480))
    - Remove unused function ([`1bda5cd`](https://github.com/wasmCloud/wasmCloud/commit/1bda5cd0da34dcf2d2613fca13430fac2484b5d9))
    - Reintroduce wasmbus over wrpc ([`a90b0ea`](https://github.com/wasmCloud/wasmCloud/commit/a90b0eaccdeb095ef147bed58e262440fb5f8486))
    - Support interface link put, component spec ([`5af1138`](https://github.com/wasmCloud/wasmCloud/commit/5af1138da6afa3ca6424d4ff10aa49211952c898))
    - Add invocation and error counts for actor invocations ([`7d51408`](https://github.com/wasmCloud/wasmCloud/commit/7d51408440509c687b01e00b77a3672a8e8c30c9))
    - Changes config topic to be for named config ([`1d46c28`](https://github.com/wasmCloud/wasmCloud/commit/1d46c284e32d2623d0b105014ef0c2f6ebc7e079))
    - Updates topics to the new standard ([`42d069e`](https://github.com/wasmCloud/wasmCloud/commit/42d069eee87d1b5befff1a95b49973064f1a1d1b))
    - Add initial support for metrics ([`17648fe`](https://github.com/wasmCloud/wasmCloud/commit/17648fedc2a1907b2f0c6d053b9747e72999addb))
    - Set log_level for providers ([`637810b`](https://github.com/wasmCloud/wasmCloud/commit/637810b996b59bb4d576b6c1321e0363b1396fe5))
    - Fix clippy warning, added ; for consistency, return directly the instance instead of wrapping the instance's components in a future ([`c6fa704`](https://github.com/wasmCloud/wasmCloud/commit/c6fa704f001a394c10f8769d670941aff62d6414))
    - Add comments, remove useless future::ready ([`7db1183`](https://github.com/wasmCloud/wasmCloud/commit/7db1183dbe84aeeb1967eb28d71876f6f175c2c2))
    - Fmt ([`1d3fd96`](https://github.com/wasmCloud/wasmCloud/commit/1d3fd96f2fe23c71b2ef70bb5199db8009c56154))
    - Fix(1365): Encapsulate in spawn instance's process in response to a nats event. Also encapsulate the .clone on the wasmtime module. After this two modification the workload spread on all core of the CPU. Relates to issue https://github.com/wasmCloud/wasmCloud/issues/1365 ([`ba7590a`](https://github.com/wasmCloud/wasmCloud/commit/ba7590ab56083173f2abbe214add648e32c2591d))
    - Fix `wasmcloud-host` clippy warning ([`50c8244`](https://github.com/wasmCloud/wasmCloud/commit/50c82440b34932ed5c03cb24a45fbacfe0c3e4d3))
    - Update version to 0.82.0 ([`aa03d41`](https://github.com/wasmCloud/wasmCloud/commit/aa03d411b571e446a842fa0e6b506436e5a04e4c))
    - Implement preview 2 interfaces ([`08b8a3c`](https://github.com/wasmCloud/wasmCloud/commit/08b8a3c72902e6d8ff4f9dcaa95b9649f3716e75))
    - Update to wasmtime 17 ([`49f3883`](https://github.com/wasmCloud/wasmCloud/commit/49f3883c586c098d4b0be44793057b97566ec2e1))
    - Move label parsing out of host library ([`fe7592b`](https://github.com/wasmCloud/wasmCloud/commit/fe7592b1a5501f3faa8bcf3bf45edf4032e92f0b))
    - Add subject to control interface logs ([`38faeac`](https://github.com/wasmCloud/wasmCloud/commit/38faeace04d4a43ee87eafdfa129555370cddecb))
    - Remove requirement for actors to have capabilities in claims ([`7b2d635`](https://github.com/wasmCloud/wasmCloud/commit/7b2d635949e2ebdb367eefb0b4ea69bf31590a7d))
    - Add README for the host crate ([`7bf02ed`](https://github.com/wasmCloud/wasmCloud/commit/7bf02ede2e92aed19bbf7ef5162e2a87dc8f5cb8))
    - Rename lattice prefix to just lattice ([`6e8faab`](https://github.com/wasmCloud/wasmCloud/commit/6e8faab6a6e9f9bb7327ffb71ded2a83718920f7))
    - Publish claims with actor_scaled ([`39849b5`](https://github.com/wasmCloud/wasmCloud/commit/39849b5f2fde4d80ccfd48c3c765c258800645ea))
    - Add heartbeat fields to inventory ([`2e8893a`](https://github.com/wasmCloud/wasmCloud/commit/2e8893af27700b86dbeb63e5e7fc4252ec6771e1))
    - Change heartbeat payload to inventory ([`032e509`](https://github.com/wasmCloud/wasmCloud/commit/032e50925e2e64c865a82cbb90de7da1f99d995e))
    - Remove singular actor events, add actor_scaled ([`df01bbd`](https://github.com/wasmCloud/wasmCloud/commit/df01bbd89fd2b690c2d1bcfe68455fb827646a10))
    - Upgrade max_instances to u32 ([`5cca9ee`](https://github.com/wasmCloud/wasmCloud/commit/5cca9ee0a88d63cb53e8d352c16a5d9d59966bc8))
    - Rename max-concurrent to max-instances, simplify scale ([`d8eb9f3`](https://github.com/wasmCloud/wasmCloud/commit/d8eb9f3ee9df65e96d076a6ba11d2600d0513207))
    - Bump wasmcloud to 0.81 ([`c038aa7`](https://github.com/wasmCloud/wasmCloud/commit/c038aa74a257664780719103c7362a747fc5a539))
    - Remove deprecated code related to start actor cmd ([`7de3182`](https://github.com/wasmCloud/wasmCloud/commit/7de31820034c4b70ab6edc772713e64aafe294a9))
    - Override previous call alias on clash ([`9d1f67f`](https://github.com/wasmCloud/wasmCloud/commit/9d1f67f37082597c25ae8a7239321d8d2e752b4d))
    - Update format for serialized claims ([`37618a3`](https://github.com/wasmCloud/wasmCloud/commit/37618a316baf573cc31311ad3ae78cd054e0e2b5))
    - Add event name as suffix on event topic ([`6994a22`](https://github.com/wasmCloud/wasmCloud/commit/6994a2202f856da93d0fe50e40c8e72dd3b7d9e6))
    - Rename label to key ([`bdb72ee`](https://github.com/wasmCloud/wasmCloud/commit/bdb72eed8778a5d8c59d0b8939f147c374cb671f))
    - Use consistent message prefix ([`9a086ec`](https://github.com/wasmCloud/wasmCloud/commit/9a086ec818dcb0292d332f606f49e04c503866b4))
    - Enable updating host labels via the control interface ([`85cb573`](https://github.com/wasmCloud/wasmCloud/commit/85cb573d29c75eae4fdaca14be808131383ca3cd))
    - Adds some additional context around test failures I was seeing ([`64d21b1`](https://github.com/wasmCloud/wasmCloud/commit/64d21b1f3d413e4c5da78d8045c1366c3782a190))
    - Adds support for actor config ([`1a048a7`](https://github.com/wasmCloud/wasmCloud/commit/1a048a71320dbbf58f331e7e958f4b1cd5ed4537))
    - Address clippy issue ([`9f9ca40`](https://github.com/wasmCloud/wasmCloud/commit/9f9ca40e7a4b1d2d553fabee8a8bfc3f49e85a3f))
    - Remove support for prov_rpc NATS connection ([`545c21c`](https://github.com/wasmCloud/wasmCloud/commit/545c21cedd1475def0648e3e700bcdd15f800c2a))
    - Remove `local` host ([`c8240e2`](https://github.com/wasmCloud/wasmCloud/commit/c8240e200c5fab84cfc558efc6445ecc91a9fa24))
    - Remove support for bindle references ([`5301084`](https://github.com/wasmCloud/wasmCloud/commit/5301084bde0db0c65811aa30c48de2a63e091fcf))
    - Polish tracing and logging levels ([`2389f27`](https://github.com/wasmCloud/wasmCloud/commit/2389f27f0b570164a895a37abd462be2d68f20be))
    - Implement wasifills for simple types ([`cfb66f8`](https://github.com/wasmCloud/wasmCloud/commit/cfb66f81180a3b47d6e7df1a444a1ec945115b15))
    - Disambiguate traced function names ([`2c77841`](https://github.com/wasmCloud/wasmCloud/commit/2c778413dd347ade2ade472365545fc954da20d0))
    - Disable handle_links trace until wadm sends fewer requests ([`7e53ed5`](https://github.com/wasmCloud/wasmCloud/commit/7e53ed56244bf4c3232b390dd1c3984dbc00be74))
    - Queue subscribe to linkdefs and get topics ([`1a86faa`](https://github.com/wasmCloud/wasmCloud/commit/1a86faa9af31af3836da95c4c312ebedaa90c6bc))
    - Update ctl to 0.31.0 ([`a1e8d3f`](https://github.com/wasmCloud/wasmCloud/commit/a1e8d3f09e039723d28d738d98b47bce54e4450d))
    - 'upstream/main' into `merge/wash` ([`0f967b0`](https://github.com/wasmCloud/wasmCloud/commit/0f967b065f30a0b5418f7ed519fdef3dc75a6205))
    - Drop write locks immediately ([`a8e1c0d`](https://github.com/wasmCloud/wasmCloud/commit/a8e1c0d6f9aa461bf8e26b68092135f90f523573))
    - Drop problematic write lock ([`774bb04`](https://github.com/wasmCloud/wasmCloud/commit/774bb0401d141c59cdd8c73e716f5d8c00002ea0))
    - Implement outgoing HTTP ([`2e8982c`](https://github.com/wasmCloud/wasmCloud/commit/2e8982c962f1cbb15a7a0e34c5a7756e02bb56a3))
    - Publish correct number of actor events ([`8fdddcc`](https://github.com/wasmCloud/wasmCloud/commit/8fdddccf5931cd10266a13f02681fdbfb34aba37))
    - Stop sending linkdef events on startup ([`e9a3917`](https://github.com/wasmCloud/wasmCloud/commit/e9a391726ad1b7a2e01bab5be09cd090f35fe661))
    - Bump crate versions ([`d163240`](https://github.com/wasmCloud/wasmCloud/commit/d16324054a454347044f7cc052da1bbd4324a284))
    - Improve reference parsing ([`d377cb4`](https://github.com/wasmCloud/wasmCloud/commit/d377cb4553519413e420f9a547fef7ecf2421591))
    - Add logs related to registry config ([`75c200d`](https://github.com/wasmCloud/wasmCloud/commit/75c200da45e383d02b2557df0bc9db5edb5f9979))
    - Add some control interface logs ([`02ae070`](https://github.com/wasmCloud/wasmCloud/commit/02ae07006c9b2bb7b58b79b9e581ba255027fc7d))
    - Change expected host label prefix to remove collision with WASMCLOUD_HOST_SEED ([`3fb60ee`](https://github.com/wasmCloud/wasmCloud/commit/3fb60eeca9e122f245b60885bdf13082c3697f04))
    - Resolve 1.73.0 warnings ([`93c0981`](https://github.com/wasmCloud/wasmCloud/commit/93c0981a4d69bc8f8fe06e6139e78e7f700a3115))
    - Fixes #746 ([`ac935a8`](https://github.com/wasmCloud/wasmCloud/commit/ac935a8028d2ba6a3a356c6e28c3681492bc09a1))
    - Give NATS 2 secs to start in test ([`a4b284c`](https://github.com/wasmCloud/wasmCloud/commit/a4b284c182278542b25056f32c86480c490a67b4))
    - Use max concurrency instead of count ([`97ecbb3`](https://github.com/wasmCloud/wasmCloud/commit/97ecbb34f81f26a36d26f458c8487e05dafa101e))
    - Return an InvocationResponse when failing to decode an invocation ([`214c5c4`](https://github.com/wasmCloud/wasmCloud/commit/214c5c4cce254b641d93882795b6f48d61dcc4f9))
    - Remove unnecessary mut ([`f4611f3`](https://github.com/wasmCloud/wasmCloud/commit/f4611f31e12227ed1257bb95809f9542d1de6353))
    - Deprecate HOST_ label prefix in favor of WASMCLOUD_HOST_ ([`88b2f2f`](https://github.com/wasmCloud/wasmCloud/commit/88b2f2f5b2424413f80d71f855185304fb003de5))
    - Rename SUCCESS to ACCEPTED, None concurrent max ([`cd8f69e`](https://github.com/wasmCloud/wasmCloud/commit/cd8f69e8d155f3e2aa5169344ff827e1f7d965cf))
    - Handle ctl requests concurrently ([`44019a8`](https://github.com/wasmCloud/wasmCloud/commit/44019a895bdb9780abea73a4dc740febf44dff6f))
    - Download actor to scale in task ([`ebe70f3`](https://github.com/wasmCloud/wasmCloud/commit/ebe70f3e8a2ae095a56a16b954d4ac20f4806364))
    - Parse labels from args ([`977feaa`](https://github.com/wasmCloud/wasmCloud/commit/977feaa1bca1ae4df625c8061f2f5330029739b4))
    - Drop logging level to trace ([`8ffa131`](https://github.com/wasmCloud/wasmCloud/commit/8ffa1317b1f106d6dcd2ec01c41fa14e6e41966e))
    - Proxy RUST_LOG to providers ([`691c371`](https://github.com/wasmCloud/wasmCloud/commit/691c3719b8030e437f565156ad5b9cff12fd4cf3))
    - Reduce verbosity of instrumented functions ([`0023f7e`](https://github.com/wasmCloud/wasmCloud/commit/0023f7e86d5a40a534f623b7220743f27871549e))
    - Rework host shutdown ([`2314f5f`](https://github.com/wasmCloud/wasmCloud/commit/2314f5f4d49c5b98949fe5d4a1eb692f1fad92b7))
    - Bump to `0.79.0` ([`578c72d`](https://github.com/wasmCloud/wasmCloud/commit/578c72d3333f1b9c343437946114c3cd6a0eead4))
    - Update dependencies ([`22276ff`](https://github.com/wasmCloud/wasmCloud/commit/22276ff61bcb4992b557f7af6624c9715f72c32b))
    - Remove instance pooling ([`017e6d4`](https://github.com/wasmCloud/wasmCloud/commit/017e6d40841f14b2158cf2ff70ca2ac8940e4b84))
    - Enforce unique image references for actors ([`3cef088`](https://github.com/wasmCloud/wasmCloud/commit/3cef088e82a9c35b2cef76ba34440213361563e4))
    - Properly format actors_started claims ([`28d2d6f`](https://github.com/wasmCloud/wasmCloud/commit/28d2d6fc5e68ab8de12771fb3b0fb00617b32b30))
    - Satisfy clippy linting ([`1a80eea`](https://github.com/wasmCloud/wasmCloud/commit/1a80eeaa1f1ba333891092f8a27e924511c0bd68))
    - Support annotation filters for stop/scale ([`ba675c8`](https://github.com/wasmCloud/wasmCloud/commit/ba675c868d6c76f4e717f64d0d6e93affea9398d))
    - Flushes clients when responding to ctl requests ([`bdd0964`](https://github.com/wasmCloud/wasmCloud/commit/bdd0964cf6262c262ee167993f5d6d48994c941d))
    - Proxy SYSTEMROOT to providers on Windows ([`f4ef770`](https://github.com/wasmCloud/wasmCloud/commit/f4ef770dda0af0c1e7df607abbe45888d819260a))
    - Publish periodic provider health status ([`68c4158`](https://github.com/wasmCloud/wasmCloud/commit/68c41586cbff172897c9ef3ed6358a66cd9cbb94))
    - Use named fields when publishing link definitions to providers ([`b2d2415`](https://github.com/wasmCloud/wasmCloud/commit/b2d2415a0370ff8cae65b530953f33a07bb7393a))
    - Allow namespaces with slashes ([`1829b27`](https://github.com/wasmCloud/wasmCloud/commit/1829b27213e836cb347a542e9cdc771c74427892))
    - Look for invocation responses from providers ([`7502bcb`](https://github.com/wasmCloud/wasmCloud/commit/7502bcb569420e2d402bf66d8a5eff2e6481a80b))
    - Store claims on fetch ([`43a75f3`](https://github.com/wasmCloud/wasmCloud/commit/43a75f3b222d99259c773f990ef8ae4754d3b6fc))
    - Implement `ResourceRef::authority` ([`ec2d0c1`](https://github.com/wasmCloud/wasmCloud/commit/ec2d0c134cd02dcaf3981d94826935c17b512d4e))
    - Introduce artifact fetchers ([`0261297`](https://github.com/wasmCloud/wasmCloud/commit/0261297230f1be083af15e257c967635654c2b71))
    - Derive `Default` for `Auth` ([`21a7e3f`](https://github.com/wasmCloud/wasmCloud/commit/21a7e3f4728a8163a6916b5d1817bac238b6fd46))
    - Rename `RegistrySettings` -> `RegistryConfig` ([`7799e38`](https://github.com/wasmCloud/wasmCloud/commit/7799e38ecc91c13add5213b72f5e56a5b9e01c6e))
    - Rework fetching logic ([`0a86d89`](https://github.com/wasmCloud/wasmCloud/commit/0a86d89a7b57329145e032b3dc2ac999d5f0f812))
    - Be explicit about `async_nats` imports ([`9f9d0e4`](https://github.com/wasmCloud/wasmCloud/commit/9f9d0e4da2fafb368fa11fd5e692ded6d912d6e5))
    - Clean-up imports ([`4e4d585`](https://github.com/wasmCloud/wasmCloud/commit/4e4d5856ae622650d1b74f2c595213ef12559d9d))
    - Expose registry as a public module ([`d104226`](https://github.com/wasmCloud/wasmCloud/commit/d1042261b6b96658af4032f5f10e5144b9a14717))
    - Emit more clear start message ([`5923e34`](https://github.com/wasmCloud/wasmCloud/commit/5923e34245c498bd9e7206bbe4ac6690192c7c60))
    - Attach traces on inbound and outbound messages ([`74142c4`](https://github.com/wasmCloud/wasmCloud/commit/74142c4cff683565fb321b7b65fbb158b5a9c990))
    - Flushes NATS clients on host stop ([`99aa2fe`](https://github.com/wasmCloud/wasmCloud/commit/99aa2fe060f1e1fe7820d7f0cc41cc2584c1e533))
    - Reduce verbosity on actor logs ([`6c42d5c`](https://github.com/wasmCloud/wasmCloud/commit/6c42d5c50375cdc2d12c86513a98b45135f0d187))
    - Implement `wasi:logging` for actors ([`05f452a`](https://github.com/wasmCloud/wasmCloud/commit/05f452a6ec1644db0fd9416f755fe0cad9cce6d3))
    - Ignore `stop_provider` annotations ([`9e61a11`](https://github.com/wasmCloud/wasmCloud/commit/9e61a113c750e885316144681946187e5c113b49))
    - Bump version to `0.78.0` ([`801377a`](https://github.com/wasmCloud/wasmCloud/commit/801377a4445cfb4c1c61a8b8f9ecbe956996272b))
    - Flatten optional annotations to always be set ([`463a2fb`](https://github.com/wasmCloud/wasmCloud/commit/463a2fbc7887ac7f78d32ccd19266630f5914f2e))
    - Address nits ([`019f63b`](https://github.com/wasmCloud/wasmCloud/commit/019f63bd9b46f68fc4703242c17cc3e38f0f889c))
    - Unwrap expired ([`59e98a9`](https://github.com/wasmCloud/wasmCloud/commit/59e98a997a4b6cc371e4983c42fb6609b73f7b53))
    - Handle stored claims without config_schema ([`680def6`](https://github.com/wasmCloud/wasmCloud/commit/680def637270c23541d9263db47e9834a9081809))
    - Reduce noise from REFMAP entries ([`9091898`](https://github.com/wasmCloud/wasmCloud/commit/90918988a075ea7c0a110cf5301ce917f5822c3b))
    - Reduce noise on instruments ([`11c932b`](https://github.com/wasmCloud/wasmCloud/commit/11c932b6838aa987eb0122bc50067cee3417025b))
    - Return invocation responses for host failures ([`c63b650`](https://github.com/wasmCloud/wasmCloud/commit/c63b6500264128904e9021cea2e3490a74d04107))
    - Support policy service ([`2ebdab7`](https://github.com/wasmCloud/wasmCloud/commit/2ebdab7551f6da93967d921316cae5d04a409a43))
    - Pub the context mod only with the otel feature enabled ([`45b0fb0`](https://github.com/wasmCloud/wasmCloud/commit/45b0fb0960921a4eebd335977fd8bc747def97a4))
    - Use cached links for queries ([`f2bf50d`](https://github.com/wasmCloud/wasmCloud/commit/f2bf50dc6c2cda49c4d82a877aaf554f153f494a))
    - Add support for call aliases ([`123cb2f`](https://github.com/wasmCloud/wasmCloud/commit/123cb2f9b8981c37bc333fece71c009ce875e30f))
    - Make content_length a required field ([`6428747`](https://github.com/wasmCloud/wasmCloud/commit/642874717b6aab760d4692f9e8b12803548314e2))
    - Use skip_all ([`782a53e`](https://github.com/wasmCloud/wasmCloud/commit/782a53ebb8a682197ebb47f4f7651dc075690e22))
    - Replace needs_chunking function with direct comparison ([`6de67aa`](https://github.com/wasmCloud/wasmCloud/commit/6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06))
    - Move longer fields to their own lines ([`c47ee0c`](https://github.com/wasmCloud/wasmCloud/commit/c47ee0cdd3225c25d2ef54bee1bbc42b39375b65))
    - Remove noisy fields from instruments ([`4fb8206`](https://github.com/wasmCloud/wasmCloud/commit/4fb8206e1d5fb21892a01b9e4f009e48c8bea2df))
    - Support chunking and dechunking of requests ([`813ce52`](https://github.com/wasmCloud/wasmCloud/commit/813ce52a9c11270814eec051dfaa8817bf9f567d))
    - Implement `wasi:blobstore` ([`bef159a`](https://github.com/wasmCloud/wasmCloud/commit/bef159ab4d5ce6ba73e7c3465110c2990da64eac))
    - Remove redundant claim clone ([`11ea950`](https://github.com/wasmCloud/wasmCloud/commit/11ea950ee26e4b7b7909d04c3505c80b4939efbb))
    - Always include cluster key as a valid issuer ([`64592ed`](https://github.com/wasmCloud/wasmCloud/commit/64592ede426193873de52fde8cf98611b6a872a8))
    - Update imports ([`a8538fb`](https://github.com/wasmCloud/wasmCloud/commit/a8538fb7926b190a180bdd2b46ad00757d98759a))
    - Pass OTEL settings to providers via deprecated env vars ([`47f4548`](https://github.com/wasmCloud/wasmCloud/commit/47f45487b46891cfbab5611ee41f52c6582a1dd8))
    - Construct a strongly typed HostData to send to providers ([`23f1759`](https://github.com/wasmCloud/wasmCloud/commit/23f1759e818117f007df8d9b1bdfdfa7710c98c5))
    - Support OTEL traces end-to-end ([`675d364`](https://github.com/wasmCloud/wasmCloud/commit/675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6))
    - Satisfy clippy ([`f2246c0`](https://github.com/wasmCloud/wasmCloud/commit/f2246c07cf38a6f142d7ce58e0623f8da5adbe83))
    - Validate invocations for antiforgery and claims ([`ccec9ed`](https://github.com/wasmCloud/wasmCloud/commit/ccec9edac6c91def872ca6a1a56f62ea716e84a2))
    - Send OTEL config via HostData ([`c334d84`](https://github.com/wasmCloud/wasmCloud/commit/c334d84d01b8b92ab9db105f8e6f0c4a6bcef8b1))
    - Stop unnecessarily satisfying clippy ([`594254a`](https://github.com/wasmCloud/wasmCloud/commit/594254af85aeaccae50337d3a8514714d11d2720))
    - Add support for putting registry credentials via control interface ([`002c993`](https://github.com/wasmCloud/wasmCloud/commit/002c9931e7fa309c39df26b313f16976e3a36001))
    - Support registry settings via config service and command-line flags ([`48d4557`](https://github.com/wasmCloud/wasmCloud/commit/48d4557c8ee895278055261bccb1293806b308b0))
    - Store claims when the scale triggers the startup actor ([`a44cd43`](https://github.com/wasmCloud/wasmCloud/commit/a44cd43b7baad0d9c332d3e7c3a52eb0f4e5b6c8))
    - Partially implement `wasi:keyvalue/atomic` ([`d434e14`](https://github.com/wasmCloud/wasmCloud/commit/d434e148620d394856246ac34bb0a64c37181970))
    - Implement provider health checks ([`72b7609`](https://github.com/wasmCloud/wasmCloud/commit/72b7609076ca3b97faf1c4a14489d1f466cf477a))
    - Ignore empty responses ([`02bc0c4`](https://github.com/wasmCloud/wasmCloud/commit/02bc0c4f348da19f058787da9a314dd9b634c6ae))
    - Store typed keys, not strings ([`75a1fb0`](https://github.com/wasmCloud/wasmCloud/commit/75a1fb075357ac2566fef1b45c930e6c400a4041))
    - Establish NATS connections concurrently ([`0db5a5b`](https://github.com/wasmCloud/wasmCloud/commit/0db5a5ba5b20535e16af46fd92f7040c8174d636))
    - Implement `wasmcloud:http/incoming-handler` support ([`50d0ed1`](https://github.com/wasmCloud/wasmCloud/commit/50d0ed1086c5f417ed64dcce139cc3c2b50ca14c))
    - Rename friendly noun ([`b77767e`](https://github.com/wasmCloud/wasmCloud/commit/b77767e6d3c32ceba0b4e5b421b532ac0788dc15))
    - Implement structured logging ([`ed64180`](https://github.com/wasmCloud/wasmCloud/commit/ed64180714873bd9be1f9008d29b09cbf276bba1))
    - Respect allow_file_load ([`ff02491`](https://github.com/wasmCloud/wasmCloud/commit/ff024913d3107dc65dd8aad69a1f598390de6d1a))
    - Revert "feat: delete claims when actors or providers are stopped" ([`1089ca1`](https://github.com/wasmCloud/wasmCloud/commit/1089ca1f5c35a9c75c2e397738a1c2a871f4cc2e))
    - Delete claims when actors or providers are stopped ([`31b76fd`](https://github.com/wasmCloud/wasmCloud/commit/31b76fd2754e1962df36340275ad5179576c8d07))
    - Properly handle empty responses ([`d9775af`](https://github.com/wasmCloud/wasmCloud/commit/d9775af7c953749f37978802c690ee29838f0da6))
    - Use `wasmcloud-compat` structs ([`5ce8d6a`](https://github.com/wasmCloud/wasmCloud/commit/5ce8d6a241f36d76013de1cc5827bf690fc62911))
    - Remove actor links on deletion ([`958aad5`](https://github.com/wasmCloud/wasmCloud/commit/958aad5ce94120322a920be71626c1aa6a349d0c))
    - Implement link names and a2a calls ([`2e3bd2b`](https://github.com/wasmCloud/wasmCloud/commit/2e3bd2bd7611e5de9fe123f53778f282613eb0de))
    - Do not proxy env vars from host to providers ([`33ef4f3`](https://github.com/wasmCloud/wasmCloud/commit/33ef4f34a5748e445f01148ec7d00bb0f01c1606))
    - Fill in missing data in host pings and heartbeat messages ([`6fd0049`](https://github.com/wasmCloud/wasmCloud/commit/6fd00493232a2c860e94f6263a3a0876ad7a6acb))
    - Refactor connection opts ([`5cd8afe`](https://github.com/wasmCloud/wasmCloud/commit/5cd8afe68e4c481dcf09c9bebb125a9e4667ed1e))
    - Enforce rpc_timeout ([`39da3e7`](https://github.com/wasmCloud/wasmCloud/commit/39da3e77462d26c8d8d2a290ce33f29a954e83ba))
    - Implement ctl_topic_prefix ([`3588b5f`](https://github.com/wasmCloud/wasmCloud/commit/3588b5f9ce2f0c0a4718d9bd576904ef77682304))
    - Implement rpc,ctl,prov_rpc connections ([`921fa78`](https://github.com/wasmCloud/wasmCloud/commit/921fa784ba3853b6b0a622c6850bb6d71437a011))
    - Remove constants ([`ce93e4a`](https://github.com/wasmCloud/wasmCloud/commit/ce93e4aad4148a51c2d30b58bdccd17ef38a9954))
    - Remove unnecessary allocations ([`a9f3ba0`](https://github.com/wasmCloud/wasmCloud/commit/a9f3ba05665d0fe7b36f0df5ed4c202dafadd0bf))
    - Add claims and link query functionality ([`d367812`](https://github.com/wasmCloud/wasmCloud/commit/d367812a666acced17f1c0f795c53ac8cf416cc6))
    - Introduce `wasmcloud-compat` crate ([`2b07909`](https://github.com/wasmCloud/wasmCloud/commit/2b07909e484f13d64ad54b649a5b8e9c36b48227))
    - Avoid fetching actor bytes twice when scaling actor ([`02d18b6`](https://github.com/wasmCloud/wasmCloud/commit/02d18b62d6777ca714e2de618d3fab914ce47ab1))
    - Generate host name based on a random number ([`556da3f`](https://github.com/wasmCloud/wasmCloud/commit/556da3fb0666f61f140eefef509913f1d34384a3))
    - Made fetch arg ordering consistent ([`478f775`](https://github.com/wasmCloud/wasmCloud/commit/478f775eb79bc955af691a7b5c7911cc36e8c98f))
    - Use allow_latest and allowed_insecure config ([`7c389be`](https://github.com/wasmCloud/wasmCloud/commit/7c389bee17d34db732babde7724286656c268f65))
    - Use js_domain provided by cli ([`9897b90`](https://github.com/wasmCloud/wasmCloud/commit/9897b90e845470faa35e8caf4816c29e6dcefd91))
    - Implement graceful provider shutdown delay ([`7d290aa`](https://github.com/wasmCloud/wasmCloud/commit/7d290aa08b2196a6082972a4d662bf1a93d07dec))
    - Maintain cluster issuers list ([`194f791`](https://github.com/wasmCloud/wasmCloud/commit/194f791c16ad6a7106393b4bcf0d0c51a70f638d))
    - Matches up base64 encoding to what providers expected ([`7a84469`](https://github.com/wasmCloud/wasmCloud/commit/7a84469dae07cd31185dbb0ad6cfd0af02d0e3a3))
    - Add support for non-default link names ([`a5db5e5`](https://github.com/wasmCloud/wasmCloud/commit/a5db5e5c0d13d66bf8fbf0da7c4f3c10021d0f90))
    - Add support for custom lattice prefix ([`c9fecb9`](https://github.com/wasmCloud/wasmCloud/commit/c9fecb99793649a6f9321b9224f85b9472889dec))
    - Implement `wasmcloud:messaging/consumer` support ([`77d663d`](https://github.com/wasmCloud/wasmCloud/commit/77d663d3e1fd5590177ac8003a313a3edf29ab1f))
    - Implement `wasi:keyvalue/readwrite` support ([`02c1ddc`](https://github.com/wasmCloud/wasmCloud/commit/02c1ddc0d62b40f63afe4d270643ebc3bf39c081))
    - Handle launch commands concurrently ([`cf3c76a`](https://github.com/wasmCloud/wasmCloud/commit/cf3c76a96c7fb411d0c286a687ccf1633cb5feeb))
    - Rename most instances of lattice and wasmbus to host ([`f3f6c21`](https://github.com/wasmCloud/wasmCloud/commit/f3f6c21f25632940a6cc1d5290f8e84271609496))
    - Use context ([`c17d742`](https://github.com/wasmCloud/wasmCloud/commit/c17d7426de06282d8f9d867ef227dc59d4227965))
    - Create bucket explicitly instead of stream ([`6b3080a`](https://github.com/wasmCloud/wasmCloud/commit/6b3080a8f655ce36b0cc6ef381ae0bf40e0e2a67))
    - Implement actor -> provider linking ([`4de853a`](https://github.com/wasmCloud/wasmCloud/commit/4de853a1d3e28126faf9efa51aaa97714af7b493))
    - Exclude self from instruments ([`977260c`](https://github.com/wasmCloud/wasmCloud/commit/977260cb713f16cb2a42e4881dc4e2b5e03d481b))
    - Implement update actor ([`c486dbf`](https://github.com/wasmCloud/wasmCloud/commit/c486dbf6116884da916da700b77559a8dbef9389))
    - Update dependencies ([`cb86378`](https://github.com/wasmCloud/wasmCloud/commit/cb86378831e48368d31947b0a44ef39080fe6d70))
    - Merge pull request #396 from rvolosatovs/feat/provider-sdk ([`6ed04f0`](https://github.com/wasmCloud/wasmCloud/commit/6ed04f00a335333196f6bafb96f2c40155537df3))
    - Implement linkdef add/delete ([`e943eca`](https://github.com/wasmCloud/wasmCloud/commit/e943eca7512a0d96a617451e2e2af78718d0f685))
    - Implement start and stop provider commands ([`d5beecd`](https://github.com/wasmCloud/wasmCloud/commit/d5beecd3d756a50f7b07e13afd688b2518039ee3))
    - Use `wasmcloud-control-interface` ([`4e8ef11`](https://github.com/wasmCloud/wasmCloud/commit/4e8ef1103a7943a8a6c921b632093e540a7b8a1b))
    - Implement actor operations ([`32cead5`](https://github.com/wasmCloud/wasmCloud/commit/32cead5ec7c1559ad0c161568712140b7d89d196))
    - Implement inventory ([`0d88c28`](https://github.com/wasmCloud/wasmCloud/commit/0d88c2858ef950975bb0309bfb906881d6e8e7a6))
    - Implement host stop ([`ec5675d`](https://github.com/wasmCloud/wasmCloud/commit/ec5675d11768ed9741a8d3e7c42cc1e5a823d41d))
    - Implement host ping ([`239f806`](https://github.com/wasmCloud/wasmCloud/commit/239f8065b63dc5ea2460ae378840874ac660856b))
    - Apply labels from environment ([`e26a5b6`](https://github.com/wasmCloud/wasmCloud/commit/e26a5b65e445d694acf3d8283cd9e80e850f8fa5))
    - Introduce wasmbus lattice ([`ef20466`](https://github.com/wasmCloud/wasmCloud/commit/ef20466a04d475159088b127b46111b80a5e1eb2))
    - Implement data streaming ([`7364dd8`](https://github.com/wasmCloud/wasmCloud/commit/7364dd8afae5c8884ca923b39c5680c60d8d0e3d))
    - Remove `wit-deps` build scripts ([`b2c6676`](https://github.com/wasmCloud/wasmCloud/commit/b2c6676987c6879fb4fcf17066dca6c9129f63b1))
    - Update WIT dependencies ([`ed4282c`](https://github.com/wasmCloud/wasmCloud/commit/ed4282c9ea1bb95e346c9a981acdc264b0fc9d3f))
    - Expand parameter names ([`8bfa14f`](https://github.com/wasmCloud/wasmCloud/commit/8bfa14f0c25a9c279a12769328c4104b8ca0de74))
    - Remove `wasmbus-rpc` usage ([`805f960`](https://github.com/wasmCloud/wasmCloud/commit/805f9609dbc04fd4ed8afd2447896988cbcc4ab5))
    - Update dependencies ([`9ee32d6`](https://github.com/wasmCloud/wasmCloud/commit/9ee32d6fa889db105608e6df3d7533a33b26f540))
    - Update `preview2-prototyping` ([`b18cd73`](https://github.com/wasmCloud/wasmCloud/commit/b18cd737a830590d232287a0ca0218357cb35813))
    - Implement builtin capabilities via WIT ([`caa965a`](https://github.com/wasmCloud/wasmCloud/commit/caa965ac17eeda67c35f41b38a236f1b682cf462))
</details>

## 0.82.0 (2024-07-31)

<csr-id-bdb519f91125c3f32f60ad9e9d1ce7bc1f147dc4/>
<csr-id-9f1b2787255cb106d98481019d26e3208c11fc9f/>
<csr-id-863296d7db28ca4815820f8b9a96a63dfe626904/>
<csr-id-e1ab91d678d8191f28e2496a68e52c7b93ad90c3/>
<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-346753ab823f911b12de763225dfd154272f1d3a/>
<csr-id-e8aac21cbc094f87fb486a903eaab9a132a7ee07/>
<csr-id-955a6893792e86292883e76de57434616c28d380/>
<csr-id-f2aed15288300989aca03f899b095d3a71f8e5cd/>
<csr-id-adb08b70ecc37ec14bb9b7eea41c8110696d9b98/>
<csr-id-6b369d49cd37a87dca1f92f31c4d4d3e33dec501/>
<csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/>
<csr-id-95a9d7d3b8c6367df93b65a2e218315cc3ec42eb/>
<csr-id-67847106d968a515ff89427454b7b14dfb486a3d/>
<csr-id-49d86501487f6811bb8b65641c40ab353f6e110d/>
<csr-id-e12ec1d6655a9aa319236a8d62a53fd6521bd683/>
<csr-id-9957ca7f8b21444b2d4e32f20a50b09f92a5b6ee/>
<csr-id-4f55396a0340d65dbebdf6d4f0ca070d6f990fc4/>
<csr-id-5990b00ea49b1bfeac3ee913dc0a9188729abeff/>
<csr-id-1bda5cd0da34dcf2d2613fca13430fac2484b5d9/>
<csr-id-a90b0eaccdeb095ef147bed58e262440fb5f8486/>
<csr-id-50c82440b34932ed5c03cb24a45fbacfe0c3e4d3/>
<csr-id-aa03d411b571e446a842fa0e6b506436e5a04e4c/>
<csr-id-08b8a3c72902e6d8ff4f9dcaa95b9649f3716e75/>
<csr-id-c038aa74a257664780719103c7362a747fc5a539/>
<csr-id-9a086ec818dcb0292d332f606f49e04c503866b4/>
<csr-id-9f9ca40e7a4b1d2d553fabee8a8bfc3f49e85a3f/>
<csr-id-c8240e200c5fab84cfc558efc6445ecc91a9fa24/>
<csr-id-5301084bde0db0c65811aa30c48de2a63e091fcf/>
<csr-id-2389f27f0b570164a895a37abd462be2d68f20be/>
<csr-id-2c778413dd347ade2ade472365545fc954da20d0/>
<csr-id-d377cb4553519413e420f9a547fef7ecf2421591/>
<csr-id-75c200da45e383d02b2557df0bc9db5edb5f9979/>
<csr-id-02ae07006c9b2bb7b58b79b9e581ba255027fc7d/>
<csr-id-93c0981a4d69bc8f8fe06e6139e78e7f700a3115/>
<csr-id-a4b284c182278542b25056f32c86480c490a67b4/>
<csr-id-cd8f69e8d155f3e2aa5169344ff827e1f7d965cf/>
<csr-id-8ffa1317b1f106d6dcd2ec01c41fa14e6e41966e/>
<csr-id-0023f7e86d5a40a534f623b7220743f27871549e/>
<csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/>
<csr-id-5923e34245c498bd9e7206bbe4ac6690192c7c60/>
<csr-id-90918988a075ea7c0a110cf5301ce917f5822c3b/>
<csr-id-11c932b6838aa987eb0122bc50067cee3417025b/>
<csr-id-4fb8206e1d5fb21892a01b9e4f009e48c8bea2df/>
<csr-id-b77767e6d3c32ceba0b4e5b421b532ac0788dc15/>
<csr-id-5cd8afe68e4c481dcf09c9bebb125a9e4667ed1e/>
<csr-id-478f775eb79bc955af691a7b5c7911cc36e8c98f/>
<csr-id-173bfa623328bd1790642ddd6d56c6f9e5b38831/>
<csr-id-c7a7ed73f5497f83a9dcfb509df580cdec3a4635/>
<csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/>
<csr-id-a96b1f370392063f403e9f25e0ef21c30fdcdfa9/>
<csr-id-49f3883c586c098d4b0be44793057b97566ec2e1/>
<csr-id-0f967b065f30a0b5418f7ed519fdef3dc75a6205/>
<csr-id-d16324054a454347044f7cc052da1bbd4324a284/>
<csr-id-578c72d3333f1b9c343437946114c3cd6a0eead4/>
<csr-id-22276ff61bcb4992b557f7af6624c9715f72c32b/>
<csr-id-801377a4445cfb4c1c61a8b8f9ecbe956996272b/>
<csr-id-cb86378831e48368d31947b0a44ef39080fe6d70/>
<csr-id-b2c6676987c6879fb4fcf17066dca6c9129f63b1/>
<csr-id-ed4282c9ea1bb95e346c9a981acdc264b0fc9d3f/>
<csr-id-9ee32d6fa889db105608e6df3d7533a33b26f540/>
<csr-id-b18cd737a830590d232287a0ca0218357cb35813/>
<csr-id-ef1d3af1ccddf33cdb37763101e3fb7577bf1433/>
<csr-id-c654448653db224c6a676ecf43150d880a9daf8c/>
<csr-id-fe7592b1a5501f3faa8bcf3bf45edf4032e92f0b/>
<csr-id-7de31820034c4b70ab6edc772713e64aafe294a9/>
<csr-id-bdb72eed8778a5d8c59d0b8939f147c374cb671f/>
<csr-id-a8e1c0d6f9aa461bf8e26b68092135f90f523573/>
<csr-id-f4611f31e12227ed1257bb95809f9542d1de6353/>
<csr-id-017e6d40841f14b2158cf2ff70ca2ac8940e4b84/>
<csr-id-ec2d0c134cd02dcaf3981d94826935c17b512d4e/>
<csr-id-0261297230f1be083af15e257c967635654c2b71/>
<csr-id-21a7e3f4728a8163a6916b5d1817bac238b6fd46/>
<csr-id-7799e38ecc91c13add5213b72f5e56a5b9e01c6e/>
<csr-id-0a86d89a7b57329145e032b3dc2ac999d5f0f812/>
<csr-id-9f9d0e4da2fafb368fa11fd5e692ded6d912d6e5/>
<csr-id-6c42d5c50375cdc2d12c86513a98b45135f0d187/>
<csr-id-463a2fbc7887ac7f78d32ccd19266630f5914f2e/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-0db5a5ba5b20535e16af46fd92f7040c8174d636/>
<csr-id-5ce8d6a241f36d76013de1cc5827bf690fc62911/>
<csr-id-a9f3ba05665d0fe7b36f0df5ed4c202dafadd0bf/>
<csr-id-6b3080a8f655ce36b0cc6ef381ae0bf40e0e2a67/>
<csr-id-977260cb713f16cb2a42e4881dc4e2b5e03d481b/>
<csr-id-4e8ef1103a7943a8a6c921b632093e540a7b8a1b/>
<csr-id-8bfa14f0c25a9c279a12769328c4104b8ca0de74/>
<csr-id-805f9609dbc04fd4ed8afd2447896988cbcc4ab5/>
<csr-id-019f63bd9b46f68fc4703242c17cc3e38f0f889c/>
<csr-id-782a53ebb8a682197ebb47f4f7651dc075690e22/>
<csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/>
<csr-id-c47ee0cdd3225c25d2ef54bee1bbc42b39375b65/>
<csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/>
<csr-id-f2246c07cf38a6f142d7ce58e0623f8da5adbe83/>
<csr-id-594254af85aeaccae50337d3a8514714d11d2720/>
<csr-id-ce93e4aad4148a51c2d30b58bdccd17ef38a9954/>
<csr-id-f3f6c21f25632940a6cc1d5290f8e84271609496/>
<csr-id-c17d7426de06282d8f9d867ef227dc59d4227965/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-bcbb402c2efe3dc881b06e666c70e01e94d3ad72/>
<csr-id-a1e8d3f09e039723d28d738d98b47bce54e4450d/>
<csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/>
<csr-id-642874717b6aab760d4692f9e8b12803548314e2/>
<csr-id-ec3bae5c03c77a0b77884b84754e33e1a8361a89/>
<csr-id-45a3d3f477b48e8a79e77880950bb785175a990d/>
<csr-id-95081cacfc3fc04911c91c32f462d643be2e12ed/>
<csr-id-e6dd0b2809510e785f4ee4c531f5666e6ab21998/>
<csr-id-1610702ad0f8cd3ba221c1b6b8ba2ce8fe57c6ae/>
<csr-id-c4b82f28947f06253aa997ae65ab11ebcc507f49/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-c666ef50fecc1ee248bf78d486a915ee077e3b4a/>
<csr-id-2ea22a28ca9fd1838fc03451f33d75690fc28f2a/>
<csr-id-b56982f437209ecaff4fa6946f8fe4c3068a62cd/>
<csr-id-388662a482442df3f74dfe8f9559fc4c07cedbe5/>
<csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/>
<csr-id-d8ad4376cb4db282047de8c4f62f6b8b907c9356/>
<csr-id-f354008c318f49565eb023a91cd3a3781d73c36a/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
<csr-id-7f4cd4cf5da514bb1d10c9d064bb905de8621d8e/>
<csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/>
<csr-id-c71f153b84e4ac4f84bdb934c9f7ca735eddd482/>
<csr-id-5225b1961038b815fe98c5250278d1aa483bdded/>
<csr-id-da461edd4e5ede0220cb9923b1d9a62808f560dc/>
<csr-id-f36471d7620fd66ff642518ae96188fef6fde5e0/>
<csr-id-da879d3e50d32fe1c09edcf2b58cb2db9c9e2661/>
<csr-id-81ab5914e7d08740eb9371c9b718f13f0419c23f/>
<csr-id-bd50166619b8810ccdc2bcd80c33ff80d94bc909/>
<csr-id-0f7093660a1ef09ff745daf5e1a96fd72c88984d/>
<csr-id-e7c30405302fcccc612209335179f0bc47d8e996/>
<csr-id-20a72597d17db8fcf0c70a7e9172edadcaad5b22/>
<csr-id-d9a8c62d6fce6e71edadcf7de78cac749cf58126/>
<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-88c07bf3be18da4f4afac3e7e356ddc507a6d85e/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>

### Chore

 - <csr-id-bdb519f91125c3f32f60ad9e9d1ce7bc1f147dc4/> remove unnecessary todo comments
 - <csr-id-9f1b2787255cb106d98481019d26e3208c11fc9f/> show provider ID on healthcheck failure messages
 - <csr-id-863296d7db28ca4815820f8b9a96a63dfe626904/> improve error message for forceful provider shutdown
 - <csr-id-e1ab91d678d8191f28e2496a68e52c7b93ad90c3/> update URLs to `wrpc` org
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings
 - <csr-id-346753ab823f911b12de763225dfd154272f1d3a/> Bumps host version to rc.2
   While I was here, I fixed the issue where we were using the host crate
   version instead of the top level binary host version in our events and
   ctl API responses
 - <csr-id-e8aac21cbc094f87fb486a903eaab9a132a7ee07/> imrpove wording for spec/provider ref mismatch
   This commit slightly improves the wording when a provider ID and
   component specification URL mismatch occurs, along with specifying a
   possible solution.
   
   This error is thrown by `wash` and it's a bit difficult to figure out
   what to resolve it otherwise.
 - <csr-id-955a6893792e86292883e76de57434616c28d380/> update `messaging` to `0.2.0`
 - <csr-id-f2aed15288300989aca03f899b095d3a71f8e5cd/> remove compat crate
 - <csr-id-adb08b70ecc37ec14bb9b7eea41c8110696d9b98/> address clippy warnings
 - <csr-id-6b369d49cd37a87dca1f92f31c4d4d3e33dec501/> use `&str` directly
 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-95a9d7d3b8c6367df93b65a2e218315cc3ec42eb/> refactor component invocation tracking
 - <csr-id-67847106d968a515ff89427454b7b14dfb486a3d/> remove functionality related to wasmbus invocations
 - <csr-id-49d86501487f6811bb8b65641c40ab353f6e110d/> update wRPC
 - <csr-id-e12ec1d6655a9aa319236a8d62a53fd6521bd683/> revert incorrectly handled config conficts
 - <csr-id-9957ca7f8b21444b2d4e32f20a50b09f92a5b6ee/> remove plural actor events
 - <csr-id-4f55396a0340d65dbebdf6d4f0ca070d6f990fc4/> integrate set-link-name and wrpc
 - <csr-id-5990b00ea49b1bfeac3ee913dc0a9188729abeff/> remove unused imports/functions
 - <csr-id-1bda5cd0da34dcf2d2613fca13430fac2484b5d9/> remove unused function
 - <csr-id-a90b0eaccdeb095ef147bed58e262440fb5f8486/> reintroduce wasmbus over wrpc
 - <csr-id-50c82440b34932ed5c03cb24a45fbacfe0c3e4d3/> fix `wasmcloud-host` clippy warning
 - <csr-id-aa03d411b571e446a842fa0e6b506436e5a04e4c/> update version to 0.82.0
 - <csr-id-08b8a3c72902e6d8ff4f9dcaa95b9649f3716e75/> implement preview 2 interfaces
 - <csr-id-c038aa74a257664780719103c7362a747fc5a539/> bump wasmcloud to 0.81
 - <csr-id-9a086ec818dcb0292d332f606f49e04c503866b4/> use consistent message prefix
 - <csr-id-9f9ca40e7a4b1d2d553fabee8a8bfc3f49e85a3f/> address clippy issue
   This is caused by Rust update
 - <csr-id-c8240e200c5fab84cfc558efc6445ecc91a9fa24/> remove `local` host
 - <csr-id-5301084bde0db0c65811aa30c48de2a63e091fcf/> remove support for bindle references
 - <csr-id-2389f27f0b570164a895a37abd462be2d68f20be/> polish tracing and logging levels
 - <csr-id-2c778413dd347ade2ade472365545fc954da20d0/> disambiguate traced function names
 - <csr-id-d377cb4553519413e420f9a547fef7ecf2421591/> improve reference parsing
 - <csr-id-75c200da45e383d02b2557df0bc9db5edb5f9979/> add logs related to registry config
 - <csr-id-02ae07006c9b2bb7b58b79b9e581ba255027fc7d/> add some control interface logs
 - <csr-id-93c0981a4d69bc8f8fe06e6139e78e7f700a3115/> resolve 1.73.0 warnings
 - <csr-id-a4b284c182278542b25056f32c86480c490a67b4/> give NATS 2 secs to start in test
 - <csr-id-cd8f69e8d155f3e2aa5169344ff827e1f7d965cf/> rename SUCCESS to ACCEPTED, None concurrent max
 - <csr-id-8ffa1317b1f106d6dcd2ec01c41fa14e6e41966e/> drop logging level to trace
 - <csr-id-0023f7e86d5a40a534f623b7220743f27871549e/> reduce verbosity of instrumented functions
 - <csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/> satisfy clippy linting
 - <csr-id-5923e34245c498bd9e7206bbe4ac6690192c7c60/> emit more clear start message
 - <csr-id-90918988a075ea7c0a110cf5301ce917f5822c3b/> reduce noise from REFMAP entries
 - <csr-id-11c932b6838aa987eb0122bc50067cee3417025b/> reduce noise on instruments
 - <csr-id-4fb8206e1d5fb21892a01b9e4f009e48c8bea2df/> remove noisy fields from instruments
 - <csr-id-b77767e6d3c32ceba0b4e5b421b532ac0788dc15/> rename friendly noun
 - <csr-id-5cd8afe68e4c481dcf09c9bebb125a9e4667ed1e/> refactor connection opts
 - <csr-id-478f775eb79bc955af691a7b5c7911cc36e8c98f/> made fetch arg ordering consistent

### Refactor

 - <csr-id-c666ef50fecc1ee248bf78d486a915ee077e3b4a/> include name with secret config
 - <csr-id-2ea22a28ca9fd1838fc03451f33d75690fc28f2a/> move SecretConfig into crate
 - <csr-id-b56982f437209ecaff4fa6946f8fe4c3068a62cd/> address feedback, application name optional
 - <csr-id-388662a482442df3f74dfe8f9559fc4c07cedbe5/> collapse application field
 - <csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/> efficiency, pass optional vec secrets
 - <csr-id-d8ad4376cb4db282047de8c4f62f6b8b907c9356/> improve error representations, cleanup
 - <csr-id-f354008c318f49565eb023a91cd3a3781d73c36a/> light refactor from followup
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
 - <csr-id-7f4cd4cf5da514bb1d10c9d064bb905de8621d8e/> improve error handling
 - <csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/> improve error usage of bail

### Other

 - <csr-id-c71f153b84e4ac4f84bdb934c9f7ca735eddd482/> add secrecy
 - <csr-id-5225b1961038b815fe98c5250278d1aa483bdded/> fix outdated `ctl_seed` reference

### Chore

 - <csr-id-da461edd4e5ede0220cb9923b1d9a62808f560dc/> clarify missing secret config error
 - <csr-id-f36471d7620fd66ff642518ae96188fef6fde5e0/> fix clippy lint
 - <csr-id-da879d3e50d32fe1c09edcf2b58cb2db9c9e2661/> update secrets integration to use the update config structure
   Update the secrets integration in a wasmCloud host to include
   information about the policy that determines which backend to
   communicate with. This is a change that comes in from wadm where the
   policy block now contains the information about which backend to use.
   
   This also passes any propertes defined on the policy to the correct
   backend, which are stored as a versioned string-encoded JSON object.
 - <csr-id-81ab5914e7d08740eb9371c9b718f13f0419c23f/> enable `ring` feature for `async-nats`
 - <csr-id-bd50166619b8810ccdc2bcd80c33ff80d94bc909/> address clippy warnings
 - <csr-id-0f7093660a1ef09ff745daf5e1a96fd72c88984d/> update to stream-based serving
 - <csr-id-e7c30405302fcccc612209335179f0bc47d8e996/> improve error messages for missing links
   When known interfaces are accessed, we show a message that notes that
   the target is unknown, but we can improve on that by alerting the user
   to a possibly missing link.
 - <csr-id-20a72597d17db8fcf0c70a7e9172edadcaad5b22/> improve error messages for missing links
   When known interfaces are accessed, we show a message that notes that
   the target is unknown, but we can improve on that by alerting the user
   to a possibly missing link.
 - <csr-id-d9a8c62d6fce6e71edadcf7de78cac749cf58126/> downgrade link/claims log/trace
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-88c07bf3be18da4f4afac3e7e356ddc507a6d85e/> Bump oci-distribution to 0.11.0
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release

### Refactor

 - <csr-id-1610702ad0f8cd3ba221c1b6b8ba2ce8fe57c6ae/> remove redundant handler clone

### Other

 - <csr-id-c4b82f28947f06253aa997ae65ab11ebcc507f49/> document invocation handling failures

### Chore

 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features

### Style

 - <csr-id-ec3bae5c03c77a0b77884b84754e33e1a8361a89/> comment

### Other

 - <csr-id-45a3d3f477b48e8a79e77880950bb785175a990d/> check component update ref and respond with a message before task
 - <csr-id-95081cacfc3fc04911c91c32f462d643be2e12ed/> check component image reference on component update

### Chore

 - <csr-id-e6dd0b2809510e785f4ee4c531f5666e6ab21998/> replace references to 'actor' with 'component'

### Documentation

 - <csr-id-7bf02ede2e92aed19bbf7ef5162e2a87dc8f5cb8/> add README for the host crate

### New Features

<csr-id-17648fedc2a1907b2f0c6d053b9747e72999addb/>
<csr-id-7b2d635949e2ebdb367eefb0b4ea69bf31590a7d/>
<csr-id-6994a2202f856da93d0fe50e40c8e72dd3b7d9e6/>
<csr-id-85cb573d29c75eae4fdaca14be808131383ca3cd/>
<csr-id-64d21b1f3d413e4c5da78d8045c1366c3782a190/>
<csr-id-1a048a71320dbbf58f331e7e958f4b1cd5ed4537/>
<csr-id-cfb66f81180a3b47d6e7df1a444a1ec945115b15/>
<csr-id-2e8982c962f1cbb15a7a0e34c5a7756e02bb56a3/>
<csr-id-44019a895bdb9780abea73a4dc740febf44dff6f/>
<csr-id-977feaa1bca1ae4df625c8061f2f5330029739b4/>
<csr-id-ba675c868d6c76f4e717f64d0d6e93affea9398d/>
<csr-id-68c41586cbff172897c9ef3ed6358a66cd9cbb94/>
<csr-id-05f452a6ec1644db0fd9416f755fe0cad9cce6d3/>
<csr-id-9e61a113c750e885316144681946187e5c113b49/>
<csr-id-2ebdab7551f6da93967d921316cae5d04a409a43/>
<csr-id-123cb2f9b8981c37bc333fece71c009ce875e30f/>
<csr-id-813ce52a9c11270814eec051dfaa8817bf9f567d/>
<csr-id-bef159ab4d5ce6ba73e7c3465110c2990da64eac/>
<csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/>
<csr-id-c334d84d01b8b92ab9db105f8e6f0c4a6bcef8b1/>
<csr-id-002c9931e7fa309c39df26b313f16976e3a36001/>
<csr-id-48d4557c8ee895278055261bccb1293806b308b0/>
<csr-id-d434e148620d394856246ac34bb0a64c37181970/>
<csr-id-50d0ed1086c5f417ed64dcce139cc3c2b50ca14c/>
<csr-id-31b76fd2754e1962df36340275ad5179576c8d07/>
<csr-id-958aad5ce94120322a920be71626c1aa6a349d0c/>
<csr-id-2e3bd2bd7611e5de9fe123f53778f282613eb0de/>
<csr-id-6fd00493232a2c860e94f6263a3a0876ad7a6acb/>
<csr-id-3588b5f9ce2f0c0a4718d9bd576904ef77682304/>
<csr-id-d367812a666acced17f1c0f795c53ac8cf416cc6/>
<csr-id-2b07909e484f13d64ad54b649a5b8e9c36b48227/>
<csr-id-556da3fb0666f61f140eefef509913f1d34384a3/>
<csr-id-a5db5e5c0d13d66bf8fbf0da7c4f3c10021d0f90/>
<csr-id-c9fecb99793649a6f9321b9224f85b9472889dec/>
<csr-id-77d663d3e1fd5590177ac8003a313a3edf29ab1f/>
<csr-id-02c1ddc0d62b40f63afe4d270643ebc3bf39c081/>
<csr-id-cf3c76a96c7fb411d0c286a687ccf1633cb5feeb/>
<csr-id-4de853a1d3e28126faf9efa51aaa97714af7b493/>
<csr-id-c486dbf6116884da916da700b77559a8dbef9389/>
<csr-id-e943eca7512a0d96a617451e2e2af78718d0f685/>
<csr-id-d5beecd3d756a50f7b07e13afd688b2518039ee3/>
<csr-id-32cead5ec7c1559ad0c161568712140b7d89d196/>
<csr-id-0d88c2858ef950975bb0309bfb906881d6e8e7a6/>
<csr-id-ec5675d11768ed9741a8d3e7c42cc1e5a823d41d/>
<csr-id-239f8065b63dc5ea2460ae378840874ac660856b/>
<csr-id-e26a5b65e445d694acf3d8283cd9e80e850f8fa5/>
<csr-id-ef20466a04d475159088b127b46111b80a5e1eb2/>
<csr-id-7364dd8afae5c8884ca923b39c5680c60d8d0e3d/>
<csr-id-caa965ac17eeda67c35f41b38a236f1b682cf462/>
<csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/>
<csr-id-e0324d66e49be015b7b231626bc3b619d9374c91/>
<csr-id-773780c59dc9af93b51abdf90a4f948ff2efb326/>
<csr-id-c2bb9cb5e2ba1c6b055f6726e86ffc95dab90d2c/>
<csr-id-659cb2eace33962e3ed05d69402607233b33a951/>
<csr-id-070751231e5bb4891b995e992e5206b3050ecc30/>
<csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/>
<csr-id-ed4b84661c08e43eadfce426474a49ad813ea6ec/>
<csr-id-e17fe933ffdc9b4e6938c4a0f2943c4813b658b1/>
<csr-id-a0a1b8c0c3d82feb19f42c4faa6de96b99bac13f/>
<csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/>
<csr-id-0aa01a92925dc12203bf9f06e13d21b7812b77eb/>
<csr-id-077a28a6567a436c99368c7eb1bd5dd2a6bc6103/>

 - <csr-id-9cd2b4034f8d5688ce250429dc14120eaf61b483/> update `wrpc:keyvalue` in providers
   part of this process is adopting `wit-bindgen-wrpc` in the host
 - <csr-id-a1754195fca5a13c8cdde713dad3e1a9765adaf5/> update `wasi:keyvalue`
 - <csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/> switch to `wit-bindgen-wrpc`
 - <csr-id-dd0d449e5bfc3826675f3f744db44b3000c67197/> add label_changed event for label update/delete
   This commit adds a `label_changed` event that can be listened to in
   order to be notified of label changes on a host.
   
   The single event handles both updates and deletes.
 - <csr-id-07b5e70a7f1321d184962d7197a8d98d1ecaaf71/> use native TLS roots along webpki
 - <csr-id-5c3dc963783c71fc91ec916be64a6f67917d9740/> fetch configuration direct from bucket
 - <csr-id-383b3f3067dddc913d5a0c052f7bbb9c47fe8663/> implement `wrpc:blobstore/blobstore` for FS
 - <csr-id-614af7e3ed734c56b27cd1d2aacb0789a85e8b81/> implement Redis `wrpc:keyvalue/{atomic,eventual}`
 - <csr-id-e0dac9de4d3a74424e3138971753db9da143db5a/> implement `wasi:http/outgoing-handler` provider
 - <csr-id-e14d0405e9f746041001e101fc24320c9e6b4f9c/> deliver full config with link
 - <csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/> Add flags for overriding the default OpenTelemetry endpoint
 - <csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/> Switch to using --enable-observability and --enable-<signal> flags
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
 - <csr-id-abb81ebbf99ec3007b1d1d48a43cfe52d86bf3e7/> include actor_id on scaled events
 - <csr-id-be1e03c5281c9cf4b02fe5349a8cf5d0d7cd0892/> downgrade provider claims to optional metadata
 - <csr-id-8afb61fb6592db6a24c53f248e4f445f9b2db580/> downgrade actor claims to optional metadata
 - <csr-id-82c249b15dba4dbe4c14a6afd2b52c7d3dc99985/> Glues in named config to actors
   This introduces a new config bundle that can watch for config changes. There
   is probably a way to reduce the number of allocations here, but it is good
   enough for now.
   
   Also, sorry for the new file. I renamed `config.rs` to `host_config.rs` so
   I could reuse the `config.rs` file, but I forgot to git mv. So that file
   hasn't changed
 - <csr-id-1dc15a127ac9830f3ebd21e61a1cf3db404eed6d/> implement AcceptorWithHeaders
 - <csr-id-fd50dcfa07b759b01e32d7f974105615c8c47db4/> implement wasmcloud_transport wrappers
 - <csr-id-f2223a3f5378c3cebfec96b5322df619fcecc556/> implement `wrpc:http/incoming-handler`
 - <csr-id-fedfd92dbba773af048fe19d956f4c3625cc17de/> begin incoming wRPC invocation implementation
 - <csr-id-0c0c004bafb60323018fc1c86cb13493f72d29cd/> switch to `wrpc` for `wasmcloud:messaging`
 - <csr-id-5ede01b1fe0bc62234d2b7d6c72775d9e248a130/> switch to `wrpc:{keyvalue,blobstore}`
 - <csr-id-246384524cfe65ce6742558425b885247b461c5c/> implement `wrpc:http/outgoing-handler.handle`
 - <csr-id-5173aa5e679ffe446f10aa549f1120f1bd1ab033/> support component invoking polyfilled functions
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
 - <csr-id-fec6f5f1372a1de5737f5ec585ad735e14c20480/> remove module support
 - <csr-id-7d51408440509c687b01e00b77a3672a8e8c30c9/> add invocation and error counts for actor invocations
   Add two new metrics for actors:
   * the count of the number of invocations (`wasmcloud_host.actor.invocations`)
* the count of errors (`wasmcloud_host.actor.invocation.errors`)
* the lattice ID
* the host ID
* provider information if a provider invoked the actor: ** the contract ID
   ** the provider ID
   ** the name of the linkdef
1. We have been needing something like this for a while, at the very least for
      being able to configure link names in an actor at runtime
2. There aren't currently any active (yes there were some in the past) efforts
      to add a generic `wasi:cloud/guest-config` interface that can allow any host
      to provide config values to a component. I want to use this as a springboard
      for the conversation in wasi-cloud as we will start to use it and can give
      active feedback as to how the interface should be shaped
- make claims optional (at least for now)
- add streaming support to `wasmcloud:bus`
- rename `wasmcloud_host` -> `wasmcloud_runtime`
- remove all `wasmcloud-interface-*` usages
- add support for `command` executables (I/O actors)
- add local lattice proving the concept, which is used for testing of the feature
- implement an actor instance pool
 - <csr-id-cda9f724d2d2e4ea55006a43b166d18875148c48/> generate crate changelogs
 - <csr-id-e928020fd774abcc213fec560d89f128464da319/> limit max execution time to 10 minutes
 - <csr-id-33b50c2d258ca9744ed65b153a6580f893172e0c/> update to Wasmtime 20
 - <csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/> generate crate changelogs
 - <csr-id-3eb453405aa144599f43bbaf56197566c9f0cf0a/> count epoch in a separate OS thread
 - <csr-id-b8c34346137edf5492fe70abeb22336a33e85bc0/> handle invocations in tasks
 - <csr-id-a66921edd9be3202d1296a165c34faf597b1dec1/> propagate `max_execution_time` to the runtime
 - <csr-id-a570a3565e129fc13b437327eb1ba18835c69f57/> add Host level configurability for max_execution_time by flag and env variables
   - Introduce humantime::Duration for capturing human readable input time.

### Bug Fixes

<csr-id-3cef088e82a9c35b2cef76ba34440213361563e4/>
<csr-id-28d2d6fc5e68ab8de12771fb3b0fb00617b32b30/>
<csr-id-bdd0964cf6262c262ee167993f5d6d48994c941d/>
<csr-id-f4ef770dda0af0c1e7df607abbe45888d819260a/>
<csr-id-b2d2415a0370ff8cae65b530953f33a07bb7393a/>
<csr-id-1829b27213e836cb347a542e9cdc771c74427892/>
<csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/>
<csr-id-43a75f3b222d99259c773f990ef8ae4754d3b6fc/>
<csr-id-4e4d5856ae622650d1b74f2c595213ef12559d9d/>
<csr-id-d1042261b6b96658af4032f5f10e5144b9a14717/>
<csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/>
<csr-id-99aa2fe060f1e1fe7820d7f0cc41cc2584c1e533/>
<csr-id-59e98a997a4b6cc371e4983c42fb6609b73f7b53/>
<csr-id-680def637270c23541d9263db47e9834a9081809/>
<csr-id-c63b6500264128904e9021cea2e3490a74d04107/>
<csr-id-45b0fb0960921a4eebd335977fd8bc747def97a4/>
<csr-id-f2bf50dc6c2cda49c4d82a877aaf554f153f494a/>
<csr-id-11ea950ee26e4b7b7909d04c3505c80b4939efbb/>
<csr-id-64592ede426193873de52fde8cf98611b6a872a8/>
<csr-id-47f45487b46891cfbab5611ee41f52c6582a1dd8/>
<csr-id-02bc0c4f348da19f058787da9a314dd9b634c6ae/>
<csr-id-75a1fb075357ac2566fef1b45c930e6c400a4041/>
<csr-id-d9775af7c953749f37978802c690ee29838f0da6/>
<csr-id-33ef4f34a5748e445f01148ec7d00bb0f01c1606/>
<csr-id-7a84469dae07cd31185dbb0ad6cfd0af02d0e3a3/>
<csr-id-3cabf109f5b986079cceb7f125f75bf53348712e/>
<csr-id-2695ad38f3338567de06f6a7ebc719a9421db7eb/>
<csr-id-1914c34317b673f3b7208863ba107c579700a133/>
<csr-id-5506c8b6eb78d8e4b793748072c4f026a4ed1863/>
<csr-id-5c68c898f8bd8351f5d16226480fbbe726efc163/>
<csr-id-b014263cf3614995f597336bb40e51ab72bfa1c9/>
<csr-id-fa1fde185b47b055e511f6f2dee095e269db1651/>

 - <csr-id-c87f3fe2654d5c874708974915bdd65f69f4afe1/> remove publish_event from stop_actor
 - <csr-id-9542e16b80c71dc7cc2f9e7175ebb25be050a242/> differentiate no config and config error
 - <csr-id-dcbbc843c5a571e1c33775c66bbd3cd528b02c26/> allow overwriting provider reference
 - <csr-id-804d67665fac39c08a536b0902a65a85035e685e/> warn scaling with different imageref
 - <csr-id-91c57b238c6e3aec5bd86f5c2103aaec21932725/> rename scaled ID from actor to component
 - <csr-id-ef50f046ade176cabbf690de59caad5d4f99c78f/> Don't clone targets with handlers
   This is a fix that ensures each component has its own set of link name
   targets. Before this, it was sharing the whole set of link names between
   all running component instances (of each individual component).
 - <csr-id-2b500d7a38cb338620f9c7834ca7fef882e42c92/> deliver target links to started provider
 - <csr-id-4ed38913f19fcd4dd44dfdcc9007e80e80cdc960/> fix `link_name` functionality, reorganize tests
 - <csr-id-ccb3c73dc1351b11233896abc068a200374df079/> correct name and data streaming, update WIT
 - <csr-id-5b4f75b7b843483566983c72c3a25e91c3de3adc/> Recreates polyfill imports on update
   This fixes an issue where if you add a new custom interface to an actor
   when updating it, it would fail to have the imports in place
 - <csr-id-fd85e254ee56abb65bee648ba0ea93b9a227a96f/> fix deadlock and slow ack of update
 - <csr-id-cab6fd2cae47f0a866f17dfdb593a48a9210bab8/> flatten claims response payload
 - <csr-id-9fe1fe8ce8d4434fb05635d7d1ae6ee07bc188c3/> race condition with initial config get
 - <csr-id-149f98b60c1e70d0e68153add3e30b8fb4483e11/> improve target lookup error handling
 - <csr-id-ec84fadfd819f203fe2e4906f5338f48f6ddec78/> update wrpc_client
 - <csr-id-152186f9940f6c9352ee5d9f91ddefe5673bdac1/> re-tag request type enum policy
 - <csr-id-a6ec7c3476daf63dc6f53afb7eb512cfc3d2b9d8/> instrument handle_invocation and call
 - <csr-id-4aa31f74bf84784af0207d2886f62d833dfe1b63/> Fixes write lock issue on policy service
   Our policy decision logic was taking a write lock even when reading the queue.
   This basically treated it like a mutex and slowed down the number of requests
   we could handle.
 - <csr-id-f3bc96128ed7033d08bc7da1ea7ba89c40880ede/> encode custom parameters as tuples
 - <csr-id-9e304cd7d19a2f7eef099703f168e8f155d4f8bc/> correctly invoke custom functions
 - <csr-id-e9bea42ed6189d903ea7fc6b7d4dc54a6fe88a12/> bindgen issues preventing builds
   This commit fixes the provider bindgen issues for non http-server
   builds (ex. kv-redis)
 - <csr-id-637810b996b59bb4d576b6c1321e0363b1396fe5/> set log_level for providers
 - <csr-id-c6fa704f001a394c10f8769d670941aff62d6414/> fix clippy warning, added ; for consistency, return directly the instance instead of wrapping the instance's components in a future
 - <csr-id-7db1183dbe84aeeb1967eb28d71876f6f175c2c2/> Add comments, remove useless future::ready
 - <csr-id-1d3fd96f2fe23c71b2ef70bb5199db8009c56154/> fmt
 - <csr-id-38faeace04d4a43ee87eafdfa129555370cddecb/> add subject to control interface logs
 - <csr-id-39849b5f2fde4d80ccfd48c3c765c258800645ea/> publish claims with actor_scaled
 - <csr-id-9d1f67f37082597c25ae8a7239321d8d2e752b4d/> override previous call alias on clash
 - <csr-id-37618a316baf573cc31311ad3ae78cd054e0e2b5/> update format for serialized claims
 - <csr-id-7e53ed56244bf4c3232b390dd1c3984dbc00be74/> disable handle_links trace until wadm sends fewer requests
 - <csr-id-1a86faa9af31af3836da95c4c312ebedaa90c6bc/> queue subscribe to linkdefs and get topics
 - <csr-id-774bb0401d141c59cdd8c73e716f5d8c00002ea0/> drop problematic write lock
 - <csr-id-8fdddccf5931cd10266a13f02681fdbfb34aba37/> publish correct number of actor events
 - <csr-id-e9a391726ad1b7a2e01bab5be09cd090f35fe661/> stop sending linkdef events on startup
 - <csr-id-3fb60eeca9e122f245b60885bdf13082c3697f04/> change expected host label prefix to remove collision with WASMCLOUD_HOST_SEED
 - <csr-id-ac935a8028d2ba6a3a356c6e28c3681492bc09a1/> fixes #746
 - <csr-id-214c5c4cce254b641d93882795b6f48d61dcc4f9/> return an InvocationResponse when failing to decode an invocation
 - <csr-id-88b2f2f5b2424413f80d71f855185304fb003de5/> deprecate HOST_ label prefix in favor of WASMCLOUD_HOST_
 - <csr-id-ebe70f3e8a2ae095a56a16b954d4ac20f4806364/> download actor to scale in task
 - <csr-id-691c3719b8030e437f565156ad5b9cff12fd4cf3/> proxy RUST_LOG to providers
 - <csr-id-2314f5f4d49c5b98949fe5d4a1eb692f1fad92b7/> rework host shutdown
   - Always include a timeout for graceful shutdown (e.g. if NATS
   connection dies, it will never finish)

### Other

 - <csr-id-173bfa623328bd1790642ddd6d56c6f9e5b38831/> expect stop_actor function parameter host_id to be unused
 - <csr-id-c7a7ed73f5497f83a9dcfb509df580cdec3a4635/> update `wrpc-interface-http`
 - <csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/> update wRPC
 - <csr-id-a96b1f370392063f403e9f25e0ef21c30fdcdfa9/> update wRPC
 - <csr-id-49f3883c586c098d4b0be44793057b97566ec2e1/> update to wasmtime 17
 - <csr-id-0f967b065f30a0b5418f7ed519fdef3dc75a6205/> 'upstream/main' into `merge/wash`
 - <csr-id-d16324054a454347044f7cc052da1bbd4324a284/> bump crate versions
 - <csr-id-578c72d3333f1b9c343437946114c3cd6a0eead4/> bump to `0.79.0`
 - <csr-id-22276ff61bcb4992b557f7af6624c9715f72c32b/> update dependencies
 - <csr-id-801377a4445cfb4c1c61a8b8f9ecbe956996272b/> bump version to `0.78.0`
 - <csr-id-cb86378831e48368d31947b0a44ef39080fe6d70/> update dependencies
 - <csr-id-b2c6676987c6879fb4fcf17066dca6c9129f63b1/> remove `wit-deps` build scripts
 - <csr-id-ed4282c9ea1bb95e346c9a981acdc264b0fc9d3f/> update WIT dependencies
 - <csr-id-9ee32d6fa889db105608e6df3d7533a33b26f540/> update dependencies
 - <csr-id-b18cd737a830590d232287a0ca0218357cb35813/> update `preview2-prototyping`

### Refactor

 - <csr-id-ef1d3af1ccddf33cdb37763101e3fb7577bf1433/> Actor -> Component
 - <csr-id-c654448653db224c6a676ecf43150d880a9daf8c/> move wasmcloud wrpc transport client to core
   This commit moves the wasmcloud-specific wrpc transport client to the
   `wasmcloud-core` crate. From here, it can be used by both
   the host (`wasmbus`) and other places like tests.
 - <csr-id-fe7592b1a5501f3faa8bcf3bf45edf4032e92f0b/> move label parsing out of host library
 - <csr-id-7de31820034c4b70ab6edc772713e64aafe294a9/> remove deprecated code related to start actor cmd
 - <csr-id-bdb72eed8778a5d8c59d0b8939f147c374cb671f/> rename label to key
 - <csr-id-a8e1c0d6f9aa461bf8e26b68092135f90f523573/> drop write locks immediately
 - <csr-id-f4611f31e12227ed1257bb95809f9542d1de6353/> remove unnecessary mut
 - <csr-id-017e6d40841f14b2158cf2ff70ca2ac8940e4b84/> remove instance pooling
 - <csr-id-ec2d0c134cd02dcaf3981d94826935c17b512d4e/> implement `ResourceRef::authority`
 - <csr-id-0261297230f1be083af15e257c967635654c2b71/> introduce artifact fetchers
 - <csr-id-21a7e3f4728a8163a6916b5d1817bac238b6fd46/> derive `Default` for `Auth`
 - <csr-id-7799e38ecc91c13add5213b72f5e56a5b9e01c6e/> rename `RegistrySettings` -> `RegistryConfig`
 - <csr-id-0a86d89a7b57329145e032b3dc2ac999d5f0f812/> rework fetching logic
 - <csr-id-9f9d0e4da2fafb368fa11fd5e692ded6d912d6e5/> be explicit about `async_nats` imports
 - <csr-id-6c42d5c50375cdc2d12c86513a98b45135f0d187/> reduce verbosity on actor logs
 - <csr-id-463a2fbc7887ac7f78d32ccd19266630f5914f2e/> flatten optional annotations to always be set
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers
 - <csr-id-0db5a5ba5b20535e16af46fd92f7040c8174d636/> establish NATS connections concurrently
 - <csr-id-5ce8d6a241f36d76013de1cc5827bf690fc62911/> use `wasmcloud-compat` structs
 - <csr-id-a9f3ba05665d0fe7b36f0df5ed4c202dafadd0bf/> remove unnecessary allocations
 - <csr-id-6b3080a8f655ce36b0cc6ef381ae0bf40e0e2a67/> create bucket explicitly instead of stream
   This also gracefully handles errors where the bucket has already
   been provisioned with custom settings, allowing multiple hosts to
   run in the same pre-provisioned lattice
 - <csr-id-977260cb713f16cb2a42e4881dc4e2b5e03d481b/> exclude self from instruments
 - <csr-id-4e8ef1103a7943a8a6c921b632093e540a7b8a1b/> use `wasmcloud-control-interface`
 - <csr-id-8bfa14f0c25a9c279a12769328c4104b8ca0de74/> expand parameter names
 - <csr-id-805f9609dbc04fd4ed8afd2447896988cbcc4ab5/> remove `wasmbus-rpc` usage

### Style

 - <csr-id-019f63bd9b46f68fc4703242c17cc3e38f0f889c/> address nits
 - <csr-id-782a53ebb8a682197ebb47f4f7651dc075690e22/> use skip_all
 - <csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/> replace needs_chunking function with direct comparison
 - <csr-id-c47ee0cdd3225c25d2ef54bee1bbc42b39375b65/> move longer fields to their own lines
 - <csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/> update imports
 - <csr-id-f2246c07cf38a6f142d7ce58e0623f8da5adbe83/> satisfy clippy
 - <csr-id-594254af85aeaccae50337d3a8514714d11d2720/> stop unnecessarily satisfying clippy
 - <csr-id-ce93e4aad4148a51c2d30b58bdccd17ef38a9954/> remove constants
 - <csr-id-f3f6c21f25632940a6cc1d5290f8e84271609496/> rename most instances of lattice and wasmbus to host
 - <csr-id-c17d7426de06282d8f9d867ef227dc59d4227965/> use context

### Chore (BREAKING)

 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-bcbb402c2efe3dc881b06e666c70e01e94d3ad72/> rename ctl actor to component
 - <csr-id-a1e8d3f09e039723d28d738d98b47bce54e4450d/> update ctl to 0.31.0

### New Features (BREAKING)

 - <csr-id-6b2e1b5915a0e894a567622ffc193230e5654c1f/> Removes old guest config and uses runtime config instead
   Most of the changes are related to wit updates, but this removes the
   guest config from `wasmcloud:bus` and pulls down `wasi:config` in its
   place
 - <csr-id-9e23be23131bbcdad746f7e85d33d5812e5f2ff9/> rename actor_scale* events
 - <csr-id-f34aac419d124aba6b6e252f85627847f67d01f4/> remove capabilities
 - <csr-id-3f2d2f44470d44809fb83de2fa34b29ad1e6cb30/> Adds version to control API
   This should be the final breaking change of the API and it will require
   a two phased rollout. I'll need to cut new core and host versions first
   and then update wash to use the new host for tests.
 - <csr-id-91874e9f4bf2b37b895a4654250203144e12815c/> convert to `wrpc:blobstore`
 - <csr-id-716d251478cf174085f6ff274854ddebd9e0d772/> use `wasmcloud:messaging` in providers
   Also implement statically invoking the `handler` on components in the
   host
 - <csr-id-5c1a0a57e761d405cdbb8ea4cbca0fe13b7e8737/> start providers with named config
 - <csr-id-188f0965e911067b5ffe9c62083fd4fbba2713f4/> refactor componentspec, deliver links to providers
 - <csr-id-df01397bce61344d3429aff081a9f9b23fad0b84/> cache request by unique data
 - <csr-id-1fb6266826f47221ec3f9413f54a4c395622dcbd/> formalize policy service
 - <csr-id-4a4b300515e9984a1befe6aaab1a6298d8ea49b1/> wrap all ctl operations in CtlResponse
 - <csr-id-e16da6614ad9ae28e8c3e6ac3ebb36faf12cb4d1/> remove collection type aliases
 - <csr-id-5275937c2c9b25139f3c208af7909889362df308/> flatten instances on actor/providers
 - <csr-id-48fc893ba2de576511aeea98a3da4cc97024c53e/> fully support interface links, remove aliases
 - <csr-id-49e5943d9a087b5ef5428f73281c36030d77502c/> support wrpc component exports
 - <csr-id-5af1138da6afa3ca6424d4ff10aa49211952c898/> support interface link put, component spec
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
 - <csr-id-2e8893af27700b86dbeb63e5e7fc4252ec6771e1/> add heartbeat fields to inventory
 - <csr-id-032e50925e2e64c865a82cbb90de7da1f99d995e/> change heartbeat payload to inventory
 - <csr-id-df01bbd89fd2b690c2d1bcfe68455fb827646a10/> remove singular actor events, add actor_scaled
 - <csr-id-5cca9ee0a88d63cb53e8d352c16a5d9d59966bc8/> upgrade max_instances to u32
 - <csr-id-d8eb9f3ee9df65e96d076a6ba11d2600d0513207/> rename max-concurrent to max-instances, simplify scale
 - <csr-id-97ecbb34f81f26a36d26f458c8487e05dafa101e/> use max concurrency instead of count
 - <csr-id-ccec9edac6c91def872ca6a1a56f62ea716e84a2/> validate invocations for antiforgery and claims
 - <csr-id-72b7609076ca3b97faf1c4a14489d1f466cf477a/> implement provider health checks
 - <csr-id-ed64180714873bd9be1f9008d29b09cbf276bba1/> implement structured logging
 - <csr-id-ff024913d3107dc65dd8aad69a1f598390de6d1a/> respect allow_file_load
 - <csr-id-39da3e77462d26c8d8d2a290ce33f29a954e83ba/> enforce rpc_timeout
 - <csr-id-921fa784ba3853b6b0a622c6850bb6d71437a011/> implement rpc,ctl,prov_rpc connections
 - <csr-id-7c389bee17d34db732babde7724286656c268f65/> use allow_latest and allowed_insecure config
 - <csr-id-9897b90e845470faa35e8caf4816c29e6dcefd91/> use js_domain provided by cli
 - <csr-id-7d290aa08b2196a6082972a4d662bf1a93d07dec/> implement graceful provider shutdown delay
 - <csr-id-194f791c16ad6a7106393b4bcf0d0c51a70f638d/> maintain cluster issuers list
 - <csr-id-d9281e2d54ac72e94f9afb61b3167690fe1fd89b/> encrypt link secrets, generate xkeys for providers
 - <csr-id-2378057bbbabbfa5a2159b6621d6009396411dd7/> configure observability with trace_level option
 - <csr-id-98b3986aca562d7f5439d3618d1eaf70f1b7e75a/> add secrets backend topic flag

### Bug Fixes (BREAKING)

 - <csr-id-2798858880004225ebe49aa1d873019a02f29e49/> consistent host operations
 - <csr-id-545c21cedd1475def0648e3e700bcdd15f800c2a/> remove support for prov_rpc NATS connection

### Refactor (BREAKING)

 - <csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/> rename lattice prefix to just lattice
 - <csr-id-642874717b6aab760d4692f9e8b12803548314e2/> make content_length a required field

<csr-unknown>
Add the --max-execution-time flag (alias: –max-time) to wasmcloud binary and wash up command, allowing for configuration of the max execution time for the Host runtime.Set Default to 10min and Time format to Milliseconds.running the docker compose for o11y(re) building dog-fetchermodifying the WADM w/ dog fetcher (done by this commit)build & create PAR for http-clientbuild & create PAR for http-serverset WASMCLOUD_OVERRIDE_TRACES_ENDPOINT before wash upreplacing existing wasmcloud host (in ~/.wash/downloads/v1.0.2) Add support for supplying additional CA certificates to OCI and OpenTelemetry clients fetch secrets for providers and links add secrets handler impl for strings set NATS queue group conflate wasi:blobstore/container interface with blobstore pass original component instance through the context upgrade wrpc, async-nats, wasmtime support ScaleComponentCommand w/ update allow empty payloads to trigger stop_host add link name to wRPC invocationsThis commit adds the “link-name” header to invocations performed bythe host using wRPC. Add support for configuring grpc protocol with opentelemetry Updates host to support new wasm artifact typeThis change is entirely backwards compatible as it still supports theold artifact type. I did test that this can download old and newmanifest types gracefully shutdown epoch interrupt thread pass policy string directly to backend use name instead of key for secret map skip backwards-compat link with secret check provided secrets topic for non-empty setup debug tracesThis commit contains experimental code used to debug/replicate theo11y traces for making a call with http-client & http-provider.Running this requires the following hackery: propagate traces through components<csr-unknown/>
<csr-unknown/>

## 0.20.0 (2024-09-28)

<csr-id-fbd1dd10a7c92a40a69c21b2cbba21c07ae8e893/>
<csr-id-fa01304b62e349be3ac3cf00aa43c2f5ead93dd5/>
<csr-id-d21d2a9e7dffd16315eeb565e2cd0e1f1aeeac6c/>
<csr-id-40e5edfc0ee48fadccd0f0fb8f8d0eb53db026f0/>
<csr-id-51c8ceb895b0069af9671e895b9f1ecb841ea6c3/>
<csr-id-da461edd4e5ede0220cb9923b1d9a62808f560dc/>
<csr-id-f36471d7620fd66ff642518ae96188fef6fde5e0/>
<csr-id-da879d3e50d32fe1c09edcf2b58cb2db9c9e2661/>
<csr-id-81ab5914e7d08740eb9371c9b718f13f0419c23f/>
<csr-id-bd50166619b8810ccdc2bcd80c33ff80d94bc909/>
<csr-id-0f7093660a1ef09ff745daf5e1a96fd72c88984d/>
<csr-id-e7c30405302fcccc612209335179f0bc47d8e996/>
<csr-id-20a72597d17db8fcf0c70a7e9172edadcaad5b22/>
<csr-id-d9a8c62d6fce6e71edadcf7de78cac749cf58126/>
<csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/>
<csr-id-88c07bf3be18da4f4afac3e7e356ddc507a6d85e/>
<csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/>
<csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/>
<csr-id-e6dd0b2809510e785f4ee4c531f5666e6ab21998/>
<csr-id-bdb519f91125c3f32f60ad9e9d1ce7bc1f147dc4/>
<csr-id-9f1b2787255cb106d98481019d26e3208c11fc9f/>
<csr-id-863296d7db28ca4815820f8b9a96a63dfe626904/>
<csr-id-e1ab91d678d8191f28e2496a68e52c7b93ad90c3/>
<csr-id-5957fce86a928c7398370547d0f43c9498185441/>
<csr-id-346753ab823f911b12de763225dfd154272f1d3a/>
<csr-id-e8aac21cbc094f87fb486a903eaab9a132a7ee07/>
<csr-id-955a6893792e86292883e76de57434616c28d380/>
<csr-id-f2aed15288300989aca03f899b095d3a71f8e5cd/>
<csr-id-adb08b70ecc37ec14bb9b7eea41c8110696d9b98/>
<csr-id-6b369d49cd37a87dca1f92f31c4d4d3e33dec501/>
<csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/>
<csr-id-95a9d7d3b8c6367df93b65a2e218315cc3ec42eb/>
<csr-id-67847106d968a515ff89427454b7b14dfb486a3d/>
<csr-id-49d86501487f6811bb8b65641c40ab353f6e110d/>
<csr-id-e12ec1d6655a9aa319236a8d62a53fd6521bd683/>
<csr-id-9957ca7f8b21444b2d4e32f20a50b09f92a5b6ee/>
<csr-id-4f55396a0340d65dbebdf6d4f0ca070d6f990fc4/>
<csr-id-5990b00ea49b1bfeac3ee913dc0a9188729abeff/>
<csr-id-1bda5cd0da34dcf2d2613fca13430fac2484b5d9/>
<csr-id-a90b0eaccdeb095ef147bed58e262440fb5f8486/>
<csr-id-50c82440b34932ed5c03cb24a45fbacfe0c3e4d3/>
<csr-id-aa03d411b571e446a842fa0e6b506436e5a04e4c/>
<csr-id-08b8a3c72902e6d8ff4f9dcaa95b9649f3716e75/>
<csr-id-c038aa74a257664780719103c7362a747fc5a539/>
<csr-id-9a086ec818dcb0292d332f606f49e04c503866b4/>
<csr-id-9f9ca40e7a4b1d2d553fabee8a8bfc3f49e85a3f/>
<csr-id-c8240e200c5fab84cfc558efc6445ecc91a9fa24/>
<csr-id-5301084bde0db0c65811aa30c48de2a63e091fcf/>
<csr-id-2389f27f0b570164a895a37abd462be2d68f20be/>
<csr-id-2c778413dd347ade2ade472365545fc954da20d0/>
<csr-id-d377cb4553519413e420f9a547fef7ecf2421591/>
<csr-id-75c200da45e383d02b2557df0bc9db5edb5f9979/>
<csr-id-02ae07006c9b2bb7b58b79b9e581ba255027fc7d/>
<csr-id-93c0981a4d69bc8f8fe06e6139e78e7f700a3115/>
<csr-id-a4b284c182278542b25056f32c86480c490a67b4/>
<csr-id-cd8f69e8d155f3e2aa5169344ff827e1f7d965cf/>
<csr-id-8ffa1317b1f106d6dcd2ec01c41fa14e6e41966e/>
<csr-id-0023f7e86d5a40a534f623b7220743f27871549e/>
<csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/>
<csr-id-5923e34245c498bd9e7206bbe4ac6690192c7c60/>
<csr-id-90918988a075ea7c0a110cf5301ce917f5822c3b/>
<csr-id-11c932b6838aa987eb0122bc50067cee3417025b/>
<csr-id-4fb8206e1d5fb21892a01b9e4f009e48c8bea2df/>
<csr-id-b77767e6d3c32ceba0b4e5b421b532ac0788dc15/>
<csr-id-5cd8afe68e4c481dcf09c9bebb125a9e4667ed1e/>
<csr-id-478f775eb79bc955af691a7b5c7911cc36e8c98f/>
<csr-id-9ac2e29babcaa3e9789c42d05d9d3ad4ccd5fcc7/>
<csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/>
<csr-id-c71f153b84e4ac4f84bdb934c9f7ca735eddd482/>
<csr-id-5225b1961038b815fe98c5250278d1aa483bdded/>
<csr-id-c4b82f28947f06253aa997ae65ab11ebcc507f49/>
<csr-id-45a3d3f477b48e8a79e77880950bb785175a990d/>
<csr-id-95081cacfc3fc04911c91c32f462d643be2e12ed/>
<csr-id-173bfa623328bd1790642ddd6d56c6f9e5b38831/>
<csr-id-c7a7ed73f5497f83a9dcfb509df580cdec3a4635/>
<csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/>
<csr-id-a96b1f370392063f403e9f25e0ef21c30fdcdfa9/>
<csr-id-49f3883c586c098d4b0be44793057b97566ec2e1/>
<csr-id-0f967b065f30a0b5418f7ed519fdef3dc75a6205/>
<csr-id-d16324054a454347044f7cc052da1bbd4324a284/>
<csr-id-578c72d3333f1b9c343437946114c3cd6a0eead4/>
<csr-id-22276ff61bcb4992b557f7af6624c9715f72c32b/>
<csr-id-801377a4445cfb4c1c61a8b8f9ecbe956996272b/>
<csr-id-cb86378831e48368d31947b0a44ef39080fe6d70/>
<csr-id-b2c6676987c6879fb4fcf17066dca6c9129f63b1/>
<csr-id-ed4282c9ea1bb95e346c9a981acdc264b0fc9d3f/>
<csr-id-9ee32d6fa889db105608e6df3d7533a33b26f540/>
<csr-id-b18cd737a830590d232287a0ca0218357cb35813/>
<csr-id-d511d74c21ab96f5913f5546e8253f34c73642a1/>
<csr-id-ac188921856c9b5fe669531e309f3f416d1bb757/>
<csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/>
<csr-id-47e80cf949a2cb287be479653336def31c130ba2/>
<csr-id-c666ef50fecc1ee248bf78d486a915ee077e3b4a/>
<csr-id-2ea22a28ca9fd1838fc03451f33d75690fc28f2a/>
<csr-id-b56982f437209ecaff4fa6946f8fe4c3068a62cd/>
<csr-id-388662a482442df3f74dfe8f9559fc4c07cedbe5/>
<csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/>
<csr-id-d8ad4376cb4db282047de8c4f62f6b8b907c9356/>
<csr-id-f354008c318f49565eb023a91cd3a3781d73c36a/>
<csr-id-4e1d6da189ff49790d876cd244aed89114efba98/>
<csr-id-7f4cd4cf5da514bb1d10c9d064bb905de8621d8e/>
<csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/>
<csr-id-1610702ad0f8cd3ba221c1b6b8ba2ce8fe57c6ae/>
<csr-id-ef1d3af1ccddf33cdb37763101e3fb7577bf1433/>
<csr-id-c654448653db224c6a676ecf43150d880a9daf8c/>
<csr-id-fe7592b1a5501f3faa8bcf3bf45edf4032e92f0b/>
<csr-id-7de31820034c4b70ab6edc772713e64aafe294a9/>
<csr-id-bdb72eed8778a5d8c59d0b8939f147c374cb671f/>
<csr-id-a8e1c0d6f9aa461bf8e26b68092135f90f523573/>
<csr-id-f4611f31e12227ed1257bb95809f9542d1de6353/>
<csr-id-017e6d40841f14b2158cf2ff70ca2ac8940e4b84/>
<csr-id-ec2d0c134cd02dcaf3981d94826935c17b512d4e/>
<csr-id-0261297230f1be083af15e257c967635654c2b71/>
<csr-id-21a7e3f4728a8163a6916b5d1817bac238b6fd46/>
<csr-id-7799e38ecc91c13add5213b72f5e56a5b9e01c6e/>
<csr-id-0a86d89a7b57329145e032b3dc2ac999d5f0f812/>
<csr-id-9f9d0e4da2fafb368fa11fd5e692ded6d912d6e5/>
<csr-id-6c42d5c50375cdc2d12c86513a98b45135f0d187/>
<csr-id-463a2fbc7887ac7f78d32ccd19266630f5914f2e/>
<csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/>
<csr-id-0db5a5ba5b20535e16af46fd92f7040c8174d636/>
<csr-id-5ce8d6a241f36d76013de1cc5827bf690fc62911/>
<csr-id-a9f3ba05665d0fe7b36f0df5ed4c202dafadd0bf/>
<csr-id-6b3080a8f655ce36b0cc6ef381ae0bf40e0e2a67/>
<csr-id-977260cb713f16cb2a42e4881dc4e2b5e03d481b/>
<csr-id-4e8ef1103a7943a8a6c921b632093e540a7b8a1b/>
<csr-id-8bfa14f0c25a9c279a12769328c4104b8ca0de74/>
<csr-id-805f9609dbc04fd4ed8afd2447896988cbcc4ab5/>
<csr-id-ec3bae5c03c77a0b77884b84754e33e1a8361a89/>
<csr-id-019f63bd9b46f68fc4703242c17cc3e38f0f889c/>
<csr-id-782a53ebb8a682197ebb47f4f7651dc075690e22/>
<csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/>
<csr-id-c47ee0cdd3225c25d2ef54bee1bbc42b39375b65/>
<csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/>
<csr-id-f2246c07cf38a6f142d7ce58e0623f8da5adbe83/>
<csr-id-594254af85aeaccae50337d3a8514714d11d2720/>
<csr-id-ce93e4aad4148a51c2d30b58bdccd17ef38a9954/>
<csr-id-f3f6c21f25632940a6cc1d5290f8e84271609496/>
<csr-id-c17d7426de06282d8f9d867ef227dc59d4227965/>
<csr-id-f418ad9c826e6ed6661175cf883882a37d5af1eb/>
<csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/>
<csr-id-bcbb402c2efe3dc881b06e666c70e01e94d3ad72/>
<csr-id-a1e8d3f09e039723d28d738d98b47bce54e4450d/>
<csr-id-47775f0da33b36f9b2707df63c416a4edc51caf6/>
<csr-id-1931aba6d2bf46967eb6f7b66fdffde96a10ae4d/>
<csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/>
<csr-id-642874717b6aab760d4692f9e8b12803548314e2/>

### Chore

 - <csr-id-fbd1dd10a7c92a40a69c21b2cbba21c07ae8e893/> Switch to using oci feature
 - <csr-id-fa01304b62e349be3ac3cf00aa43c2f5ead93dd5/> fix clippy lints
 - <csr-id-d21d2a9e7dffd16315eeb565e2cd0e1f1aeeac6c/> set missing link log to warn
 - <csr-id-40e5edfc0ee48fadccd0f0fb8f8d0eb53db026f0/> Make wasmcloud host heartbeat interval configurable
 - <csr-id-51c8ceb895b0069af9671e895b9f1ecb841ea6c3/> update component/runtime/host crate READMEs
 - <csr-id-da461edd4e5ede0220cb9923b1d9a62808f560dc/> clarify missing secret config error
 - <csr-id-f36471d7620fd66ff642518ae96188fef6fde5e0/> fix clippy lint
 - <csr-id-da879d3e50d32fe1c09edcf2b58cb2db9c9e2661/> update secrets integration to use the update config structure
   Update the secrets integration in a wasmCloud host to include
   information about the policy that determines which backend to
   communicate with. This is a change that comes in from wadm where the
   policy block now contains the information about which backend to use.
   
   This also passes any propertes defined on the policy to the correct
   backend, which are stored as a versioned string-encoded JSON object.
 - <csr-id-81ab5914e7d08740eb9371c9b718f13f0419c23f/> enable `ring` feature for `async-nats`
 - <csr-id-bd50166619b8810ccdc2bcd80c33ff80d94bc909/> address clippy warnings
 - <csr-id-0f7093660a1ef09ff745daf5e1a96fd72c88984d/> update to stream-based serving
 - <csr-id-e7c30405302fcccc612209335179f0bc47d8e996/> improve error messages for missing links
   When known interfaces are accessed, we show a message that notes that
   the target is unknown, but we can improve on that by alerting the user
   to a possibly missing link.
 - <csr-id-20a72597d17db8fcf0c70a7e9172edadcaad5b22/> improve error messages for missing links
   When known interfaces are accessed, we show a message that notes that
   the target is unknown, but we can improve on that by alerting the user
   to a possibly missing link.
 - <csr-id-d9a8c62d6fce6e71edadcf7de78cac749cf58126/> downgrade link/claims log/trace
 - <csr-id-20c72ce0ed423561ae6dbd5a91959bec24ff7cf3/> Replace actor references by component in crates
   Rename wash-cli wash-build tests name and references
   
   Fix nix flake path to Cargo.lock file
   
   Fix format
   
   Rename in wash-cli tests
 - <csr-id-88c07bf3be18da4f4afac3e7e356ddc507a6d85e/> Bump oci-distribution to 0.11.0
 - <csr-id-4e0313ae4cfb5cbb2d3fa0320c662466a7082c0e/> generate changelogs after 1.0.1 release
 - <csr-id-0f03f1f91210a4ed3fa64a4b07aebe8e56627ea6/> updated with newest features
 - <csr-id-e6dd0b2809510e785f4ee4c531f5666e6ab21998/> replace references to 'actor' with 'component'
 - <csr-id-bdb519f91125c3f32f60ad9e9d1ce7bc1f147dc4/> remove unnecessary todo comments
 - <csr-id-9f1b2787255cb106d98481019d26e3208c11fc9f/> show provider ID on healthcheck failure messages
 - <csr-id-863296d7db28ca4815820f8b9a96a63dfe626904/> improve error message for forceful provider shutdown
 - <csr-id-e1ab91d678d8191f28e2496a68e52c7b93ad90c3/> update URLs to `wrpc` org
 - <csr-id-5957fce86a928c7398370547d0f43c9498185441/> address clippy warnings
 - <csr-id-346753ab823f911b12de763225dfd154272f1d3a/> Bumps host version to rc.2
   While I was here, I fixed the issue where we were using the host crate
   version instead of the top level binary host version in our events and
   ctl API responses
 - <csr-id-e8aac21cbc094f87fb486a903eaab9a132a7ee07/> imrpove wording for spec/provider ref mismatch
   This commit slightly improves the wording when a provider ID and
   component specification URL mismatch occurs, along with specifying a
   possible solution.
   
   This error is thrown by `wash` and it's a bit difficult to figure out
   what to resolve it otherwise.
 - <csr-id-955a6893792e86292883e76de57434616c28d380/> update `messaging` to `0.2.0`
 - <csr-id-f2aed15288300989aca03f899b095d3a71f8e5cd/> remove compat crate
 - <csr-id-adb08b70ecc37ec14bb9b7eea41c8110696d9b98/> address clippy warnings
 - <csr-id-6b369d49cd37a87dca1f92f31c4d4d3e33dec501/> use `&str` directly
 - <csr-id-d65512b5e86eb4d13e64cffa220a5a842c7bb72b/> Use traces instead of tracing user-facing language to align with OTEL signal names
 - <csr-id-95a9d7d3b8c6367df93b65a2e218315cc3ec42eb/> refactor component invocation tracking
 - <csr-id-67847106d968a515ff89427454b7b14dfb486a3d/> remove functionality related to wasmbus invocations
 - <csr-id-49d86501487f6811bb8b65641c40ab353f6e110d/> update wRPC
 - <csr-id-e12ec1d6655a9aa319236a8d62a53fd6521bd683/> revert incorrectly handled config conficts
 - <csr-id-9957ca7f8b21444b2d4e32f20a50b09f92a5b6ee/> remove plural actor events
 - <csr-id-4f55396a0340d65dbebdf6d4f0ca070d6f990fc4/> integrate set-link-name and wrpc
 - <csr-id-5990b00ea49b1bfeac3ee913dc0a9188729abeff/> remove unused imports/functions
 - <csr-id-1bda5cd0da34dcf2d2613fca13430fac2484b5d9/> remove unused function
 - <csr-id-a90b0eaccdeb095ef147bed58e262440fb5f8486/> reintroduce wasmbus over wrpc
 - <csr-id-50c82440b34932ed5c03cb24a45fbacfe0c3e4d3/> fix `wasmcloud-host` clippy warning
 - <csr-id-aa03d411b571e446a842fa0e6b506436e5a04e4c/> update version to 0.82.0
 - <csr-id-08b8a3c72902e6d8ff4f9dcaa95b9649f3716e75/> implement preview 2 interfaces
 - <csr-id-c038aa74a257664780719103c7362a747fc5a539/> bump wasmcloud to 0.81
 - <csr-id-9a086ec818dcb0292d332f606f49e04c503866b4/> use consistent message prefix
 - <csr-id-9f9ca40e7a4b1d2d553fabee8a8bfc3f49e85a3f/> address clippy issue
   This is caused by Rust update
 - <csr-id-c8240e200c5fab84cfc558efc6445ecc91a9fa24/> remove `local` host
 - <csr-id-5301084bde0db0c65811aa30c48de2a63e091fcf/> remove support for bindle references
 - <csr-id-2389f27f0b570164a895a37abd462be2d68f20be/> polish tracing and logging levels
 - <csr-id-2c778413dd347ade2ade472365545fc954da20d0/> disambiguate traced function names
 - <csr-id-d377cb4553519413e420f9a547fef7ecf2421591/> improve reference parsing
 - <csr-id-75c200da45e383d02b2557df0bc9db5edb5f9979/> add logs related to registry config
 - <csr-id-02ae07006c9b2bb7b58b79b9e581ba255027fc7d/> add some control interface logs
 - <csr-id-93c0981a4d69bc8f8fe06e6139e78e7f700a3115/> resolve 1.73.0 warnings
 - <csr-id-a4b284c182278542b25056f32c86480c490a67b4/> give NATS 2 secs to start in test
 - <csr-id-cd8f69e8d155f3e2aa5169344ff827e1f7d965cf/> rename SUCCESS to ACCEPTED, None concurrent max
 - <csr-id-8ffa1317b1f106d6dcd2ec01c41fa14e6e41966e/> drop logging level to trace
 - <csr-id-0023f7e86d5a40a534f623b7220743f27871549e/> reduce verbosity of instrumented functions
 - <csr-id-1a80eeaa1f1ba333891092f8a27e924511c0bd68/> satisfy clippy linting
 - <csr-id-5923e34245c498bd9e7206bbe4ac6690192c7c60/> emit more clear start message
 - <csr-id-90918988a075ea7c0a110cf5301ce917f5822c3b/> reduce noise from REFMAP entries
 - <csr-id-11c932b6838aa987eb0122bc50067cee3417025b/> reduce noise on instruments
 - <csr-id-4fb8206e1d5fb21892a01b9e4f009e48c8bea2df/> remove noisy fields from instruments
 - <csr-id-b77767e6d3c32ceba0b4e5b421b532ac0788dc15/> rename friendly noun
 - <csr-id-5cd8afe68e4c481dcf09c9bebb125a9e4667ed1e/> refactor connection opts
 - <csr-id-478f775eb79bc955af691a7b5c7911cc36e8c98f/> made fetch arg ordering consistent

### Documentation

 - <csr-id-7bf02ede2e92aed19bbf7ef5162e2a87dc8f5cb8/> add README for the host crate

### New Features

<csr-id-24e77b7f1f29580ca348a758302cdc6e75cc3afd/>
<csr-id-e0324d66e49be015b7b231626bc3b619d9374c91/>
<csr-id-773780c59dc9af93b51abdf90a4f948ff2efb326/>
<csr-id-c2bb9cb5e2ba1c6b055f6726e86ffc95dab90d2c/>
<csr-id-659cb2eace33962e3ed05d69402607233b33a951/>
<csr-id-070751231e5bb4891b995e992e5206b3050ecc30/>
<csr-id-9cb1b784fe7a8892d73bdb40d1172b1879fcd932/>
<csr-id-ed4b84661c08e43eadfce426474a49ad813ea6ec/>
<csr-id-e17fe933ffdc9b4e6938c4a0f2943c4813b658b1/>
<csr-id-a0a1b8c0c3d82feb19f42c4faa6de96b99bac13f/>
<csr-id-378b7c89c8b00a5dcee76c06bc8de615dc58f8aa/>
<csr-id-0aa01a92925dc12203bf9f06e13d21b7812b77eb/>
<csr-id-077a28a6567a436c99368c7eb1bd5dd2a6bc6103/>
<csr-id-f986e39450676dc598b92f13cb6e52b9c3200c0b/>
<csr-id-3eb453405aa144599f43bbaf56197566c9f0cf0a/>
<csr-id-b8c34346137edf5492fe70abeb22336a33e85bc0/>
<csr-id-a66921edd9be3202d1296a165c34faf597b1dec1/>
<csr-id-e928020fd774abcc213fec560d89f128464da319/>
<csr-id-33b50c2d258ca9744ed65b153a6580f893172e0c/>
<csr-id-9cd2b4034f8d5688ce250429dc14120eaf61b483/>
<csr-id-a1754195fca5a13c8cdde713dad3e1a9765adaf5/>
<csr-id-322f471f9a8154224a50ec33517c9f5b1716d2d5/>
<csr-id-dd0d449e5bfc3826675f3f744db44b3000c67197/>
<csr-id-07b5e70a7f1321d184962d7197a8d98d1ecaaf71/>
<csr-id-5c3dc963783c71fc91ec916be64a6f67917d9740/>
<csr-id-383b3f3067dddc913d5a0c052f7bbb9c47fe8663/>
<csr-id-614af7e3ed734c56b27cd1d2aacb0789a85e8b81/>
<csr-id-e0dac9de4d3a74424e3138971753db9da143db5a/>
<csr-id-e14d0405e9f746041001e101fc24320c9e6b4f9c/>
<csr-id-6fe14b89d4c26e5c01e54773268c6d0f04236e71/>
<csr-id-868570be8d94a6d73608c7cde5d2422e15f9eb0c/>
<csr-id-76c1ed7b5c49152aabd83d27f0b8955d7f874864/>
<csr-id-abb81ebbf99ec3007b1d1d48a43cfe52d86bf3e7/>
<csr-id-be1e03c5281c9cf4b02fe5349a8cf5d0d7cd0892/>
<csr-id-8afb61fb6592db6a24c53f248e4f445f9b2db580/>
<csr-id-82c249b15dba4dbe4c14a6afd2b52c7d3dc99985/>
<csr-id-1dc15a127ac9830f3ebd21e61a1cf3db404eed6d/>
<csr-id-fd50dcfa07b759b01e32d7f974105615c8c47db4/>
<csr-id-f2223a3f5378c3cebfec96b5322df619fcecc556/>
<csr-id-fedfd92dbba773af048fe19d956f4c3625cc17de/>
<csr-id-0c0c004bafb60323018fc1c86cb13493f72d29cd/>
<csr-id-5ede01b1fe0bc62234d2b7d6c72775d9e248a130/>
<csr-id-246384524cfe65ce6742558425b885247b461c5c/>
<csr-id-5173aa5e679ffe446f10aa549f1120f1bd1ab033/>
<csr-id-5d19ba16a98dca9439628e8449309ccaa763ab10/>
<csr-id-fec6f5f1372a1de5737f5ec585ad735e14c20480/>
<csr-id-7d51408440509c687b01e00b77a3672a8e8c30c9/>
<csr-id-17648fedc2a1907b2f0c6d053b9747e72999addb/>
<csr-id-7b2d635949e2ebdb367eefb0b4ea69bf31590a7d/>
<csr-id-6994a2202f856da93d0fe50e40c8e72dd3b7d9e6/>
<csr-id-85cb573d29c75eae4fdaca14be808131383ca3cd/>
<csr-id-64d21b1f3d413e4c5da78d8045c1366c3782a190/>
<csr-id-1a048a71320dbbf58f331e7e958f4b1cd5ed4537/>
<csr-id-cfb66f81180a3b47d6e7df1a444a1ec945115b15/>
<csr-id-2e8982c962f1cbb15a7a0e34c5a7756e02bb56a3/>
<csr-id-44019a895bdb9780abea73a4dc740febf44dff6f/>
<csr-id-977feaa1bca1ae4df625c8061f2f5330029739b4/>
<csr-id-ba675c868d6c76f4e717f64d0d6e93affea9398d/>
<csr-id-68c41586cbff172897c9ef3ed6358a66cd9cbb94/>
<csr-id-05f452a6ec1644db0fd9416f755fe0cad9cce6d3/>
<csr-id-9e61a113c750e885316144681946187e5c113b49/>
<csr-id-2ebdab7551f6da93967d921316cae5d04a409a43/>
<csr-id-123cb2f9b8981c37bc333fece71c009ce875e30f/>
<csr-id-813ce52a9c11270814eec051dfaa8817bf9f567d/>
<csr-id-bef159ab4d5ce6ba73e7c3465110c2990da64eac/>
<csr-id-675d364d2f53f9dbf7ebb6c655d5fbbbba6c62b6/>
<csr-id-c334d84d01b8b92ab9db105f8e6f0c4a6bcef8b1/>
<csr-id-002c9931e7fa309c39df26b313f16976e3a36001/>
<csr-id-48d4557c8ee895278055261bccb1293806b308b0/>
<csr-id-d434e148620d394856246ac34bb0a64c37181970/>
<csr-id-50d0ed1086c5f417ed64dcce139cc3c2b50ca14c/>
<csr-id-31b76fd2754e1962df36340275ad5179576c8d07/>
<csr-id-958aad5ce94120322a920be71626c1aa6a349d0c/>
<csr-id-2e3bd2bd7611e5de9fe123f53778f282613eb0de/>
<csr-id-6fd00493232a2c860e94f6263a3a0876ad7a6acb/>
<csr-id-3588b5f9ce2f0c0a4718d9bd576904ef77682304/>
<csr-id-d367812a666acced17f1c0f795c53ac8cf416cc6/>
<csr-id-2b07909e484f13d64ad54b649a5b8e9c36b48227/>
<csr-id-556da3fb0666f61f140eefef509913f1d34384a3/>
<csr-id-a5db5e5c0d13d66bf8fbf0da7c4f3c10021d0f90/>
<csr-id-c9fecb99793649a6f9321b9224f85b9472889dec/>
<csr-id-77d663d3e1fd5590177ac8003a313a3edf29ab1f/>
<csr-id-02c1ddc0d62b40f63afe4d270643ebc3bf39c081/>
<csr-id-cf3c76a96c7fb411d0c286a687ccf1633cb5feeb/>
<csr-id-4de853a1d3e28126faf9efa51aaa97714af7b493/>
<csr-id-c486dbf6116884da916da700b77559a8dbef9389/>
<csr-id-e943eca7512a0d96a617451e2e2af78718d0f685/>
<csr-id-d5beecd3d756a50f7b07e13afd688b2518039ee3/>
<csr-id-32cead5ec7c1559ad0c161568712140b7d89d196/>
<csr-id-0d88c2858ef950975bb0309bfb906881d6e8e7a6/>
<csr-id-ec5675d11768ed9741a8d3e7c42cc1e5a823d41d/>
<csr-id-239f8065b63dc5ea2460ae378840874ac660856b/>
<csr-id-e26a5b65e445d694acf3d8283cd9e80e850f8fa5/>
<csr-id-ef20466a04d475159088b127b46111b80a5e1eb2/>
<csr-id-7364dd8afae5c8884ca923b39c5680c60d8d0e3d/>
<csr-id-caa965ac17eeda67c35f41b38a236f1b682cf462/>

 - <csr-id-8575f732df33ca973ff340fc3e4bc7fbfeaf89f3/> Adds support for batch support to the host
   This enables keyvalue batch support inside of the host, along with a test
   to make sure it works. Not all of our providers implement batch yet, so
   this uses the Redis provider, which did have implementions. I did have to
   fix the redis provider to get the right type of data back and transform
   it. I also had to update our wRPC versions so we could pick up on some
   bug fixes for the types we are encoding in the batch interface.
 - <csr-id-26d7f64659dbf3263f36da92df89003c579077cc/> fallback to `wrpc:blobstore@0.1.0`
 - <csr-id-61641322dec02dd835e81b51de72cbd1007d13cf/> support for sending out config updates to providers
 - <csr-id-a570a3565e129fc13b437327eb1ba18835c69f57/> add Host level configurability for max_execution_time by flag and env variables
   - Introduce humantime::Duration for capturing human readable input time.
- Add the `--max-execution-time` flag (alias: --max-time) to wasmcloud binary and wash up command, allowing for configuration of the max execution time for the Host runtime.
- Set Default to 10min and Time format to Milliseconds.
* the count of the number of invocations (`wasmcloud_host.actor.invocations`)
* the count of errors (`wasmcloud_host.actor.invocation.errors`)
* the lattice ID
* the host ID
* provider information if a provider invoked the actor: ** the contract ID
   ** the provider ID
   ** the name of the linkdef
1. We have been needing something like this for a while, at the very least for
      being able to configure link names in an actor at runtime
2. There aren't currently any active (yes there were some in the past) efforts
      to add a generic `wasi:cloud/guest-config` interface that can allow any host
      to provide config values to a component. I want to use this as a springboard
      for the conversation in wasi-cloud as we will start to use it and can give
      active feedback as to how the interface should be shaped
- make claims optional (at least for now)
- add streaming support to `wasmcloud:bus`
- rename `wasmcloud_host` -> `wasmcloud_runtime`
- remove all `wasmcloud-interface-*` usages
- add support for `command` executables (I/O actors)
- add local lattice proving the concept, which is used for testing of the feature
- implement an actor instance pool

### Bug Fixes

<csr-id-fa1fde185b47b055e511f6f2dee095e269db1651/>
<csr-id-3cabf109f5b986079cceb7f125f75bf53348712e/>
<csr-id-c87f3fe2654d5c874708974915bdd65f69f4afe1/>
<csr-id-9542e16b80c71dc7cc2f9e7175ebb25be050a242/>
<csr-id-dcbbc843c5a571e1c33775c66bbd3cd528b02c26/>
<csr-id-804d67665fac39c08a536b0902a65a85035e685e/>
<csr-id-91c57b238c6e3aec5bd86f5c2103aaec21932725/>
<csr-id-ef50f046ade176cabbf690de59caad5d4f99c78f/>
<csr-id-2b500d7a38cb338620f9c7834ca7fef882e42c92/>
<csr-id-4ed38913f19fcd4dd44dfdcc9007e80e80cdc960/>
<csr-id-ccb3c73dc1351b11233896abc068a200374df079/>
<csr-id-5b4f75b7b843483566983c72c3a25e91c3de3adc/>
<csr-id-fd85e254ee56abb65bee648ba0ea93b9a227a96f/>
<csr-id-cab6fd2cae47f0a866f17dfdb593a48a9210bab8/>
<csr-id-9fe1fe8ce8d4434fb05635d7d1ae6ee07bc188c3/>
<csr-id-149f98b60c1e70d0e68153add3e30b8fb4483e11/>
<csr-id-ec84fadfd819f203fe2e4906f5338f48f6ddec78/>
<csr-id-152186f9940f6c9352ee5d9f91ddefe5673bdac1/>
<csr-id-a6ec7c3476daf63dc6f53afb7eb512cfc3d2b9d8/>
<csr-id-4aa31f74bf84784af0207d2886f62d833dfe1b63/>
<csr-id-f3bc96128ed7033d08bc7da1ea7ba89c40880ede/>
<csr-id-9e304cd7d19a2f7eef099703f168e8f155d4f8bc/>
<csr-id-e9bea42ed6189d903ea7fc6b7d4dc54a6fe88a12/>
<csr-id-637810b996b59bb4d576b6c1321e0363b1396fe5/>
<csr-id-c6fa704f001a394c10f8769d670941aff62d6414/>
<csr-id-7db1183dbe84aeeb1967eb28d71876f6f175c2c2/>
<csr-id-1d3fd96f2fe23c71b2ef70bb5199db8009c56154/>
<csr-id-38faeace04d4a43ee87eafdfa129555370cddecb/>
<csr-id-39849b5f2fde4d80ccfd48c3c765c258800645ea/>
<csr-id-9d1f67f37082597c25ae8a7239321d8d2e752b4d/>
<csr-id-37618a316baf573cc31311ad3ae78cd054e0e2b5/>
<csr-id-7e53ed56244bf4c3232b390dd1c3984dbc00be74/>
<csr-id-1a86faa9af31af3836da95c4c312ebedaa90c6bc/>
<csr-id-774bb0401d141c59cdd8c73e716f5d8c00002ea0/>
<csr-id-8fdddccf5931cd10266a13f02681fdbfb34aba37/>
<csr-id-e9a391726ad1b7a2e01bab5be09cd090f35fe661/>
<csr-id-3fb60eeca9e122f245b60885bdf13082c3697f04/>
<csr-id-ac935a8028d2ba6a3a356c6e28c3681492bc09a1/>
<csr-id-214c5c4cce254b641d93882795b6f48d61dcc4f9/>
<csr-id-88b2f2f5b2424413f80d71f855185304fb003de5/>
<csr-id-ebe70f3e8a2ae095a56a16b954d4ac20f4806364/>
<csr-id-691c3719b8030e437f565156ad5b9cff12fd4cf3/>
<csr-id-2314f5f4d49c5b98949fe5d4a1eb692f1fad92b7/>
<csr-id-3cef088e82a9c35b2cef76ba34440213361563e4/>
<csr-id-28d2d6fc5e68ab8de12771fb3b0fb00617b32b30/>
<csr-id-bdd0964cf6262c262ee167993f5d6d48994c941d/>
<csr-id-f4ef770dda0af0c1e7df607abbe45888d819260a/>
<csr-id-b2d2415a0370ff8cae65b530953f33a07bb7393a/>
<csr-id-1829b27213e836cb347a542e9cdc771c74427892/>
<csr-id-7502bcb569420e2d402bf66d8a5eff2e6481a80b/>
<csr-id-43a75f3b222d99259c773f990ef8ae4754d3b6fc/>
<csr-id-4e4d5856ae622650d1b74f2c595213ef12559d9d/>
<csr-id-d1042261b6b96658af4032f5f10e5144b9a14717/>
<csr-id-74142c4cff683565fb321b7b65fbb158b5a9c990/>
<csr-id-99aa2fe060f1e1fe7820d7f0cc41cc2584c1e533/>
<csr-id-59e98a997a4b6cc371e4983c42fb6609b73f7b53/>
<csr-id-680def637270c23541d9263db47e9834a9081809/>
<csr-id-c63b6500264128904e9021cea2e3490a74d04107/>
<csr-id-45b0fb0960921a4eebd335977fd8bc747def97a4/>
<csr-id-f2bf50dc6c2cda49c4d82a877aaf554f153f494a/>
<csr-id-11ea950ee26e4b7b7909d04c3505c80b4939efbb/>
<csr-id-64592ede426193873de52fde8cf98611b6a872a8/>
<csr-id-47f45487b46891cfbab5611ee41f52c6582a1dd8/>
<csr-id-02bc0c4f348da19f058787da9a314dd9b634c6ae/>
<csr-id-75a1fb075357ac2566fef1b45c930e6c400a4041/>
<csr-id-d9775af7c953749f37978802c690ee29838f0da6/>
<csr-id-33ef4f34a5748e445f01148ec7d00bb0f01c1606/>
<csr-id-7a84469dae07cd31185dbb0ad6cfd0af02d0e3a3/>

 - <csr-id-4da0105ac7bf463eeb79bc3047cb5e92664f8a7c/> rework `wasi:http` error handling
 - <csr-id-726dd689e6d64eb44930834425d69f21cefc61cd/> log handling errors
 - <csr-id-fc131edff75a7240fe519d8bbc4b08ac31d9bf1c/> Sets a higher value for the incoming events channel
   If you were running a high number of concurrent component invocations, it
   would result in a warning (and possible hang/dropped message) due to a
   full channel. This change attempts to set the channel size to the
   `max_instances` value with a minimum and a maximum possible value (i.e
   we don't want something with 20k instances to have a channel that large).
 - <csr-id-991cb21d5bceee681c613b314a9d2dfaeee890ee/> remove provider caching for local file references
   This commit removes provider caching for local file references -- when
   a file is loaded via a container registry, caching is enabled but if
   it is loaded via a local file on disk, caching is never employed.
 - <csr-id-77b1af98c1cdfdb5425a590856c0e27f2a7e805f/> prevent Provider ConfigBundle early drop, do cleanup
 - <csr-id-76265cdcbf2959f87961340576e71e085f1f4942/> always publish component_scale event
 - <csr-id-2695ad38f3338567de06f6a7ebc719a9421db7eb/> pass policy string directly to backend
 - <csr-id-1914c34317b673f3b7208863ba107c579700a133/> use name instead of key for secret map
 - <csr-id-5506c8b6eb78d8e4b793748072c4f026a4ed1863/> skip backwards-compat link with secret
 - <csr-id-5c68c898f8bd8351f5d16226480fbbe726efc163/> check provided secrets topic for non-empty
 - <csr-id-b014263cf3614995f597336bb40e51ab72bfa1c9/> setup debug traces
   This commit contains experimental code used to debug/replicate the
   o11y traces for making a call with http-client & http-provider.
   
   Running this requires the following hackery:
   
   - running the docker compose for o11y
- (re) building dog-fetcher
- modifying the WADM w/ dog fetcher (done by this commit)
- build & create PAR for http-client
- build & create PAR for http-server
- set WASMCLOUD_OVERRIDE_TRACES_ENDPOINT before `wash up`
- replacing existing wasmcloud host (in `~/.wash/downloads/v1.0.2`)
- Always include a timeout for graceful shutdown (e.g. if NATS
     connection dies, it will never finish)
- Stop if one of the core wasmbus tasks dies
- Flush NATS queues concurrently on shutdown
- Handle `stopped` method errors

### Other

 - <csr-id-9ac2e29babcaa3e9789c42d05d9d3ad4ccd5fcc7/> add links integration test
 - <csr-id-7cd2e71cb82c1e1b75d0c89bd5bda343016e75f4/> bump for test-util release
   Bump wasmcloud-core v0.8.0, opentelemetry-nats v0.1.1, provider-archive v0.12.0, wasmcloud-runtime v0.3.0, wasmcloud-secrets-types v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, safety bump 8 crates
   
   SAFETY BUMP: wasmcloud-runtime v0.3.0, wasmcloud-secrets-client v0.3.0, wasmcloud-tracing v0.6.0, wasmcloud-host v0.82.0, wasmcloud-test-util v0.12.0, wasmcloud-provider-sdk v0.7.0, wash-cli v0.30.0, wash-lib v0.23.0
 - <csr-id-c71f153b84e4ac4f84bdb934c9f7ca735eddd482/> add secrecy
 - <csr-id-5225b1961038b815fe98c5250278d1aa483bdded/> fix outdated `ctl_seed` reference
 - <csr-id-c4b82f28947f06253aa997ae65ab11ebcc507f49/> document invocation handling failures
 - <csr-id-45a3d3f477b48e8a79e77880950bb785175a990d/> check component update ref and respond with a message before task
 - <csr-id-95081cacfc3fc04911c91c32f462d643be2e12ed/> check component image reference on component update
 - <csr-id-173bfa623328bd1790642ddd6d56c6f9e5b38831/> expect stop_actor function parameter host_id to be unused
 - <csr-id-c7a7ed73f5497f83a9dcfb509df580cdec3a4635/> update `wrpc-interface-http`
 - <csr-id-95cfb6d99f0e54243b2fb2618de39210d8f3694f/> update wRPC
 - <csr-id-a96b1f370392063f403e9f25e0ef21c30fdcdfa9/> update wRPC
 - <csr-id-49f3883c586c098d4b0be44793057b97566ec2e1/> update to wasmtime 17
 - <csr-id-0f967b065f30a0b5418f7ed519fdef3dc75a6205/> 'upstream/main' into `merge/wash`
 - <csr-id-d16324054a454347044f7cc052da1bbd4324a284/> bump crate versions
 - <csr-id-578c72d3333f1b9c343437946114c3cd6a0eead4/> bump to `0.79.0`
 - <csr-id-22276ff61bcb4992b557f7af6624c9715f72c32b/> update dependencies
 - <csr-id-801377a4445cfb4c1c61a8b8f9ecbe956996272b/> bump version to `0.78.0`
 - <csr-id-cb86378831e48368d31947b0a44ef39080fe6d70/> update dependencies
 - <csr-id-b2c6676987c6879fb4fcf17066dca6c9129f63b1/> remove `wit-deps` build scripts
 - <csr-id-ed4282c9ea1bb95e346c9a981acdc264b0fc9d3f/> update WIT dependencies
 - <csr-id-9ee32d6fa889db105608e6df3d7533a33b26f540/> update dependencies
 - <csr-id-b18cd737a830590d232287a0ca0218357cb35813/> update `preview2-prototyping`

### Refactor

 - <csr-id-d511d74c21ab96f5913f5546e8253f34c73642a1/> remove missing caching code
 - <csr-id-ac188921856c9b5fe669531e309f3f416d1bb757/> remove unused deps
 - <csr-id-0547e3a429059b15ec969a0fa36d7823a6b7331f/> move functionality into core
   This commit moves functionality that was previously located in the
   unreleased `wasmcloud-host` crate into core.
 - <csr-id-47e80cf949a2cb287be479653336def31c130ba2/> abort health check tasks on provider drop
 - <csr-id-c666ef50fecc1ee248bf78d486a915ee077e3b4a/> include name with secret config
 - <csr-id-2ea22a28ca9fd1838fc03451f33d75690fc28f2a/> move SecretConfig into crate
 - <csr-id-b56982f437209ecaff4fa6946f8fe4c3068a62cd/> address feedback, application name optional
 - <csr-id-388662a482442df3f74dfe8f9559fc4c07cedbe5/> collapse application field
 - <csr-id-cfbf23226f34f3e7245a5d36cd7bb15e1796850c/> efficiency, pass optional vec secrets
 - <csr-id-d8ad4376cb4db282047de8c4f62f6b8b907c9356/> improve error representations, cleanup
 - <csr-id-f354008c318f49565eb023a91cd3a3781d73c36a/> light refactor from followup
 - <csr-id-4e1d6da189ff49790d876cd244aed89114efba98/> remove extra trace_level field
 - <csr-id-7f4cd4cf5da514bb1d10c9d064bb905de8621d8e/> improve error handling
 - <csr-id-c30bf33f754c15122ead7f041b7d3e063dd1db33/> improve error usage of bail
 - <csr-id-1610702ad0f8cd3ba221c1b6b8ba2ce8fe57c6ae/> remove redundant handler clone
 - <csr-id-ef1d3af1ccddf33cdb37763101e3fb7577bf1433/> Actor -> Component
 - <csr-id-c654448653db224c6a676ecf43150d880a9daf8c/> move wasmcloud wrpc transport client to core
   This commit moves the wasmcloud-specific wrpc transport client to the
   `wasmcloud-core` crate. From here, it can be used by both
   the host (`wasmbus`) and other places like tests.
 - <csr-id-fe7592b1a5501f3faa8bcf3bf45edf4032e92f0b/> move label parsing out of host library
 - <csr-id-7de31820034c4b70ab6edc772713e64aafe294a9/> remove deprecated code related to start actor cmd
 - <csr-id-bdb72eed8778a5d8c59d0b8939f147c374cb671f/> rename label to key
 - <csr-id-a8e1c0d6f9aa461bf8e26b68092135f90f523573/> drop write locks immediately
 - <csr-id-f4611f31e12227ed1257bb95809f9542d1de6353/> remove unnecessary mut
 - <csr-id-017e6d40841f14b2158cf2ff70ca2ac8940e4b84/> remove instance pooling
 - <csr-id-ec2d0c134cd02dcaf3981d94826935c17b512d4e/> implement `ResourceRef::authority`
 - <csr-id-0261297230f1be083af15e257c967635654c2b71/> introduce artifact fetchers
 - <csr-id-21a7e3f4728a8163a6916b5d1817bac238b6fd46/> derive `Default` for `Auth`
 - <csr-id-7799e38ecc91c13add5213b72f5e56a5b9e01c6e/> rename `RegistrySettings` -> `RegistryConfig`
 - <csr-id-0a86d89a7b57329145e032b3dc2ac999d5f0f812/> rework fetching logic
 - <csr-id-9f9d0e4da2fafb368fa11fd5e692ded6d912d6e5/> be explicit about `async_nats` imports
 - <csr-id-6c42d5c50375cdc2d12c86513a98b45135f0d187/> reduce verbosity on actor logs
 - <csr-id-463a2fbc7887ac7f78d32ccd19266630f5914f2e/> flatten optional annotations to always be set
 - <csr-id-23f1759e818117f007df8d9b1bdfdfa7710c98c5/> construct a strongly typed HostData to send to providers
 - <csr-id-0db5a5ba5b20535e16af46fd92f7040c8174d636/> establish NATS connections concurrently
 - <csr-id-5ce8d6a241f36d76013de1cc5827bf690fc62911/> use `wasmcloud-compat` structs
 - <csr-id-a9f3ba05665d0fe7b36f0df5ed4c202dafadd0bf/> remove unnecessary allocations
 - <csr-id-6b3080a8f655ce36b0cc6ef381ae0bf40e0e2a67/> create bucket explicitly instead of stream
   This also gracefully handles errors where the bucket has already
   been provisioned with custom settings, allowing multiple hosts to
   run in the same pre-provisioned lattice
 - <csr-id-977260cb713f16cb2a42e4881dc4e2b5e03d481b/> exclude self from instruments
 - <csr-id-4e8ef1103a7943a8a6c921b632093e540a7b8a1b/> use `wasmcloud-control-interface`
 - <csr-id-8bfa14f0c25a9c279a12769328c4104b8ca0de74/> expand parameter names
 - <csr-id-805f9609dbc04fd4ed8afd2447896988cbcc4ab5/> remove `wasmbus-rpc` usage

### Style

 - <csr-id-ec3bae5c03c77a0b77884b84754e33e1a8361a89/> comment
 - <csr-id-019f63bd9b46f68fc4703242c17cc3e38f0f889c/> address nits
 - <csr-id-782a53ebb8a682197ebb47f4f7651dc075690e22/> use skip_all
 - <csr-id-6de67aa1ddab22ec99fe70f2c2fdc92dc5760b06/> replace needs_chunking function with direct comparison
 - <csr-id-c47ee0cdd3225c25d2ef54bee1bbc42b39375b65/> move longer fields to their own lines
 - <csr-id-a8538fb7926b190a180bdd2b46ad00757d98759a/> update imports
 - <csr-id-f2246c07cf38a6f142d7ce58e0623f8da5adbe83/> satisfy clippy
 - <csr-id-594254af85aeaccae50337d3a8514714d11d2720/> stop unnecessarily satisfying clippy
 - <csr-id-ce93e4aad4148a51c2d30b58bdccd17ef38a9954/> remove constants
 - <csr-id-f3f6c21f25632940a6cc1d5290f8e84271609496/> rename most instances of lattice and wasmbus to host
 - <csr-id-c17d7426de06282d8f9d867ef227dc59d4227965/> use context

### Chore (BREAKING)

 - <csr-id-f418ad9c826e6ed6661175cf883882a37d5af1eb/> update host w/ new ctrl iface
 - <csr-id-bc5d296f3a58bc5e8df0da7e0bf2624d03335d9f/> remove cluster_seed/cluster_issuers
 - <csr-id-bcbb402c2efe3dc881b06e666c70e01e94d3ad72/> rename ctl actor to component
 - <csr-id-a1e8d3f09e039723d28d738d98b47bce54e4450d/> update ctl to 0.31.0

### New Features (BREAKING)

 - <csr-id-be7233e730dce14578651a17d16410d7a7dbe91c/> introduce linkdef_set_failed event
 - <csr-id-f4b4eeb64a6eab4f6dfb540eacd7e2256d80aa71/> allow tuning runtime parameters
 - <csr-id-d9281e2d54ac72e94f9afb61b3167690fe1fd89b/> encrypt link secrets, generate xkeys for providers
 - <csr-id-2378057bbbabbfa5a2159b6621d6009396411dd7/> configure observability with trace_level option
 - <csr-id-98b3986aca562d7f5439d3618d1eaf70f1b7e75a/> add secrets backend topic flag
 - <csr-id-6b2e1b5915a0e894a567622ffc193230e5654c1f/> Removes old guest config and uses runtime config instead
   Most of the changes are related to wit updates, but this removes the
   guest config from `wasmcloud:bus` and pulls down `wasi:config` in its
   place
 - <csr-id-9e23be23131bbcdad746f7e85d33d5812e5f2ff9/> rename actor_scale* events
 - <csr-id-f34aac419d124aba6b6e252f85627847f67d01f4/> remove capabilities
 - <csr-id-3f2d2f44470d44809fb83de2fa34b29ad1e6cb30/> Adds version to control API
   This should be the final breaking change of the API and it will require
   a two phased rollout. I'll need to cut new core and host versions first
   and then update wash to use the new host for tests.
 - <csr-id-91874e9f4bf2b37b895a4654250203144e12815c/> convert to `wrpc:blobstore`
 - <csr-id-716d251478cf174085f6ff274854ddebd9e0d772/> use `wasmcloud:messaging` in providers
   Also implement statically invoking the `handler` on components in the
   host
 - <csr-id-5c1a0a57e761d405cdbb8ea4cbca0fe13b7e8737/> start providers with named config
 - <csr-id-188f0965e911067b5ffe9c62083fd4fbba2713f4/> refactor componentspec, deliver links to providers
 - <csr-id-df01397bce61344d3429aff081a9f9b23fad0b84/> cache request by unique data
 - <csr-id-1fb6266826f47221ec3f9413f54a4c395622dcbd/> formalize policy service
 - <csr-id-4a4b300515e9984a1befe6aaab1a6298d8ea49b1/> wrap all ctl operations in CtlResponse
 - <csr-id-e16da6614ad9ae28e8c3e6ac3ebb36faf12cb4d1/> remove collection type aliases
 - <csr-id-5275937c2c9b25139f3c208af7909889362df308/> flatten instances on actor/providers
 - <csr-id-48fc893ba2de576511aeea98a3da4cc97024c53e/> fully support interface links, remove aliases
 - <csr-id-49e5943d9a087b5ef5428f73281c36030d77502c/> support wrpc component exports
 - <csr-id-5af1138da6afa3ca6424d4ff10aa49211952c898/> support interface link put, component spec
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
 - <csr-id-2e8893af27700b86dbeb63e5e7fc4252ec6771e1/> add heartbeat fields to inventory
 - <csr-id-032e50925e2e64c865a82cbb90de7da1f99d995e/> change heartbeat payload to inventory
 - <csr-id-df01bbd89fd2b690c2d1bcfe68455fb827646a10/> remove singular actor events, add actor_scaled
 - <csr-id-5cca9ee0a88d63cb53e8d352c16a5d9d59966bc8/> upgrade max_instances to u32
 - <csr-id-d8eb9f3ee9df65e96d076a6ba11d2600d0513207/> rename max-concurrent to max-instances, simplify scale
 - <csr-id-97ecbb34f81f26a36d26f458c8487e05dafa101e/> use max concurrency instead of count
 - <csr-id-ccec9edac6c91def872ca6a1a56f62ea716e84a2/> validate invocations for antiforgery and claims
 - <csr-id-72b7609076ca3b97faf1c4a14489d1f466cf477a/> implement provider health checks
 - <csr-id-ed64180714873bd9be1f9008d29b09cbf276bba1/> implement structured logging
 - <csr-id-ff024913d3107dc65dd8aad69a1f598390de6d1a/> respect allow_file_load
 - <csr-id-39da3e77462d26c8d8d2a290ce33f29a954e83ba/> enforce rpc_timeout
 - <csr-id-921fa784ba3853b6b0a622c6850bb6d71437a011/> implement rpc,ctl,prov_rpc connections
 - <csr-id-7c389bee17d34db732babde7724286656c268f65/> use allow_latest and allowed_insecure config
 - <csr-id-9897b90e845470faa35e8caf4816c29e6dcefd91/> use js_domain provided by cli
 - <csr-id-7d290aa08b2196a6082972a4d662bf1a93d07dec/> implement graceful provider shutdown delay
 - <csr-id-194f791c16ad6a7106393b4bcf0d0c51a70f638d/> maintain cluster issuers list

### Bug Fixes (BREAKING)

 - <csr-id-8db14d3bb320e6732c62c3abfe936d72e45fe734/> ensure links are unique on source+interface+name
 - <csr-id-2798858880004225ebe49aa1d873019a02f29e49/> consistent host operations
 - <csr-id-545c21cedd1475def0648e3e700bcdd15f800c2a/> remove support for prov_rpc NATS connection

### Refactor (BREAKING)

 - <csr-id-47775f0da33b36f9b2707df63c416a4edc51caf6/> remove functionality from host (moved to core)
 - <csr-id-1931aba6d2bf46967eb6f7b66fdffde96a10ae4d/> use result for changed()
 - <csr-id-6e8faab6a6e9f9bb7327ffb71ded2a83718920f7/> rename lattice prefix to just lattice
 - <csr-id-642874717b6aab760d4692f9e8b12803548314e2/> make content_length a required field

<csr-unknown>
 Add support for supplying additional CA certificates to OCI and OpenTelemetry clients fetch secrets for providers and links add secrets handler impl for strings set NATS queue group conflate wasi:blobstore/container interface with blobstore pass original component instance through the context upgrade wrpc, async-nats, wasmtime support ScaleComponentCommand w/ update allow empty payloads to trigger stop_host add link name to wRPC invocationsThis commit adds the “link-name” header to invocations performed bythe host using wRPC. Add support for configuring grpc protocol with opentelemetry Updates host to support new wasm artifact typeThis change is entirely backwards compatible as it still supports theold artifact type. I did test that this can download old and newmanifest types gracefully shutdown epoch interrupt thread generate crate changelogs count epoch in a separate OS thread handle invocations in tasks propagate max_execution_time to the runtime limit max execution time to 10 minutes update to Wasmtime 20 update wrpc:keyvalue in providerspart of this process is adopting wit-bindgen-wrpc in the host update wasi:keyvalue switch to wit-bindgen-wrpc add label_changed event for label update/deleteThis commit adds a label_changed event that can be listened to inorder to be notified of label changes on a host.The single event handles both updates and deletes. use native TLS roots along webpki fetch configuration direct from bucket implement wrpc:blobstore/blobstore for FS implement Redis wrpc:keyvalue/{atomic,eventual} implement wasi:http/outgoing-handler provider deliver full config with link Add flags for overriding the default OpenTelemetry endpoint Switch to using –enable-observability and –enable-<signal> flags support pubsub on wRPC subjectsUp until now, publishing and subscribing for RPC communcations on theNATS cluster happened on subjects that were related to the wasmbusprotocol (i.e. ‘wasmbus.rpc.*’).To support the WIT-native invocations, i.e. wRPC (#1389), we mustchange the publication and subscription subjects to include also thesubjects that are expected to be used by wprc.This commit updates the provider-sdk to listen additionally tosubjects that are required/used by wrpc, though we do not yet have animplementation for encode/deocde. include actor_id on scaled events downgrade provider claims to optional metadata downgrade actor claims to optional metadata Glues in named config to actorsThis introduces a new config bundle that can watch for config changes. Thereis probably a way to reduce the number of allocations here, but it is goodenough for now.Also, sorry for the new file. I renamed config.rs to host_config.rs soI could reuse the config.rs file, but I forgot to git mv. So that filehasn’t changed implement AcceptorWithHeaders implement wasmcloud_transport wrappers implement wrpc:http/incoming-handler begin incoming wRPC invocation implementation switch to wrpc for wasmcloud:messaging switch to wrpc:{keyvalue,blobstore} implement wrpc:http/outgoing-handler.handle support component invoking polyfilled functions change set-target to set-link-nameUp until the relatively low-level wasmcloud:bus/lattice WITinterface has used a function called set-target to aim invocationsthat occurred in compliant actors and providers.Since wRPC (#1389)enabled  wasmCloud 1.0 is going to be WIT-first going forward, allWIT-driven function executions have access to the relevantinterface (WIT interfaces, rather than Smithy-derived ones) that theycall, at call time.Given that actor & provider side function executions have access totheir WIT interfaces (ex. wasi:keyvalue/readwrite.get), what we needto do is differentiate between the case where multiple targetsmight be responding to the same WIT interface-backed invocations.Unlike before, set-target only needs to really differentiate between linknames.This commit updates set-target to perform differentiate between linknames, building on the work already done to introduce more opaquetargeting via Component IDs. remove module support add invocation and error counts for actor invocationsAdd two new metrics for actors:This also adds a bunch of new attributes to the existing actor metrics so that they make sense in an environment with multiple hosts. Specifically this adds:For actor to actor calls, instead of having the provider metadata it instead has the public key of the invoking actor.An example of what this looks like as an exported Prometheus metric:wasmcloud_host_actor_invocations_total{actor_ref="wasmcloud.azurecr.io/echo:0.3.8", caller_provider_contract_id="wasmcloud:httpserver", caller_provider_id="VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M", caller_provider_link_name="default", host="ND7L3RZ6NLYJGN25E6DKYS665ITWXAPXZXGZXLCUQEDDU65RB5TVUHEN", job="wasmcloud-host", lattice="default", operation="HttpServer.HandleRequest"}
Provider metrics will likely need to wait until the wRPC work is finished. Add initial support for metrics remove requirement for actors to have capabilities in claims add event name as suffix on event topic enable updating host labels via the control interface Adds some additional context around test failures I was seeing Adds support for actor configThis is a fairly large PR because it is adding several new control interfacetopics as well as actually adding the actor config feature.This feature was motivated by 2 major reasons:With that said, note that this is only going to be added for actors built againstthe component model. Since this is net new functionality, I didn’t think it wasworth it to try to backport.As for testing, I have tested that an actor can import the functions and get the valuesvia the various e2e tests and also manually validated that all of the new topicswork. implement wasifills for simple types implement outgoing HTTP handle ctl requests concurrently parse labels from args support annotation filters for stop/scale publish periodic provider health status implement wasi:logging for actors ignore stop_provider annotations support policy service add support for call aliases support chunking and dechunking of requests implement wasi:blobstore support OTEL traces end-to-end send OTEL config via HostData add support for putting registry credentials via control interface support registry settings via config service and command-line flags partially implement wasi:keyvalue/atomic implement wasmcloud:http/incoming-handler support delete claims when actors or providers are stopped remove actor links on deletion implement link names and a2a calls fill in missing data in host pings and heartbeat messages implement ctl_topic_prefix add claims and link query functionality introduce wasmcloud-compat crate generate host name based on a random number add support for non-default link names add support for custom lattice prefix implement wasmcloud:messaging/consumer support implement wasi:keyvalue/readwrite support handle launch commands concurrently implement actor -> provider linking implement update actor implement linkdef add/delete implement start and stop provider commands implement actor operations implement inventory implement host stop implement host ping apply labels from environment introduce wasmbus lattice implement data streaming implement builtin capabilities via WIT propagate traces through components handle invocation handling errors remove publish_event from stop_actor differentiate no config and config error allow overwriting provider reference warn scaling with different imageref rename scaled ID from actor to component Don’t clone targets with handlersThis is a fix that ensures each component has its own set of link nametargets. Before this, it was sharing the whole set of link names betweenall running component instances (of each individual component). deliver target links to started provider fix link_name functionality, reorganize tests correct name and data streaming, update WIT Recreates polyfill imports on updateThis fixes an issue where if you add a new custom interface to an actorwhen updating it, it would fail to have the imports in place fix deadlock and slow ack of update flatten claims response payload race condition with initial config get improve target lookup error handling update wrpc_client re-tag request type enum policy instrument handle_invocation and call Fixes write lock issue on policy serviceOur policy decision logic was taking a write lock even when reading the queue.This basically treated it like a mutex and slowed down the number of requestswe could handle. encode custom parameters as tuples correctly invoke custom functions bindgen issues preventing buildsThis commit fixes the provider bindgen issues for non http-serverbuilds (ex. kv-redis) set log_level for providers fix clippy warning, added ; for consistency, return directly the instance instead of wrapping the instance’s components in a future Add comments, remove useless future::ready fmt add subject to control interface logs publish claims with actor_scaled override previous call alias on clash update format for serialized claims disable handle_links trace until wadm sends fewer requests queue subscribe to linkdefs and get topics drop problematic write lock publish correct number of actor events stop sending linkdef events on startup change expected host label prefix to remove collision with WASMCLOUD_HOST_SEED fixes #746 return an InvocationResponse when failing to decode an invocation deprecate HOST_ label prefix in favor of WASMCLOUD_HOST_ download actor to scale in task proxy RUST_LOG to providers rework host shutdown enforce unique image references for actors properly format actors_started claims Flushes clients when responding to ctl requestsIn cases where wadm was fairly busy, we started getting errors that thehost wasn’t acking our scale actor commands (even though it was actuallyscaling). So I added in some flushing when we send responses so we can besure that the response actually got sent proxy SYSTEMROOT to providers on Windows use named fields when publishing link definitions to providers allow namespaces with slashes look for invocation responses from providers store claims on fetch clean-up imports expose registry as a public module attach traces on inbound and outbound messagesParse headers from CTL interface and RPC messages, and publish tracing headerson CTL and RPC responses Flushes NATS clients on host stopWithout this, sending responses to things like a host stop command orpublishing the host stop event can fail as we don’t ensure all messagesin the NATS client queue have been sent unwrap expired handle stored claims without config_schema return invocation responses for host failures pub the context mod only with the otel feature enabled use cached links for queries remove redundant claim clone always include cluster key as a valid issuer pass OTEL settings to providers via deprecated env vars ignore empty responses store typed keys, not strings properly handle empty responses do not proxy env vars from host to providers Matches up base64 encoding to what providers expected<csr-unknown/>

## v0.18.2 (2021-05-13)

## v0.18.1 (2021-04-29)

## v0.18.0 (2021-04-16)

## v0.17.0 (2021-04-13)

## v0.16.1 (2021-04-09)

## v0.16.0 (2021-03-26)

## v0.15.5 (2021-03-23)

## v0.15.4 (2021-03-22)

## v0.15.3 (2021-03-05)

## v0.15.1 (2021-03-01)

## v0.15.0 (2021-02-16)


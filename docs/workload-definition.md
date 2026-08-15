# The workload definition — a proposal

**Status: proposal.** Nothing in this document changes how wash reads config
today. The wizard already writes the compatible subset (explicit
`workload.allowedHosts`); the rest lands in reviewed slices once the open
decisions at the bottom are made.

## The problem

What a workload *is* — its ingress, its components, what each imports, its
policies — is currently smeared across three files and several conventions:

- `.wash/config.yaml`, whose key casing is split down the middle: `build`,
  `dev`, `dev.volumes` are snake_case; `workload`, `dev.components[]`,
  `hostPlugins`, environment layers are camelCase. `dev.host_plugins` (snake)
  sits beside `host.hostPlugins` (camel).
- Load-bearing runtime semantics hide inside opaque string maps. Messaging
  subject routing is `dev.components[].config.subscriptions` — a
  comma-separated string in a free-form KV map. The per-interface
  `dev.host_interfaces[].config` vocabulary (`backend`, `root`, `url`,
  `prefix`, `database` — *required* for postgres — `buckets`, `read_only`,
  `subscriptions`, `consumer_group`) is enumerable only by reading nine plugin
  modules.
- `workload.allowedHosts` has a three-way default: omitted → allow-all, `[]` →
  deny-all, and Rust's `default()` → deny-all. `allowedIpNameLookups` is
  deny-by-default and separate.
- Backend selection is a five-field precedence chain
  (`wasi_keyvalue_redis_url` > `wasi_keyvalue_nats_url` > `wasi_keyvalue_path`
  > `data_nats_url` > in-memory) stated nowhere as a rule.
- The ingress is inferred (whichever component exports a handler), never
  declared. A service is four fields (`dev.service`, `service_file`,
  `service_image`, `service_pull_policy`) rather than a role.

## The proposal, in one sentence

The `workload:` section of `.wash/config.yaml` grows into the canonical,
camelCase, fully documented definition of what the workload is; `dev:` keeps
only how one machine serves it; no new file is introduced.

Why no new file: `WorkloadConfig` already deliberately mirrors the k8s CRD's
`localResources`, the figment loading/merging (global config, project config,
`WASH_*` env, CLI) already exists for this file, and a second file would orphan
both. There is no topology file at all — a workload's shape is derived from
source on demand and persisted only as the `dev.wasm.topology` OCI annotation
at push — and the wizard writes no replay file: `wash wizard --from <dir>`
recovers the recipe from the project itself.

Two principles:

- **WIT stays authoritative for linking.** A declared `imports:` list
  annotates and enables drift warnings; it never overrides what the host
  derives from the built component.
- **Feature gates stay visible.** Host component plugins keep their hard error
  without `host-component-plugins`; multiplexed named imports document their
  gate.

## The schema, by example

### An HTTP chain with keyvalue on a mid node

```yaml
build:
  command: cargo build --workspace --target wasm32-wasip2 --release
  componentPath: target/wasm32-wasip2/release/ingress.wasm

workload:
  name: order-api
  components:
    - name: ingress
      build: true                  # the component wash dev rebuilds & reloads
      trigger:
        http: {}                   # declared ingress — no longer inferred
    - name: step1
      file: target/wasm32-wasip2/release/step1.wasm
      imports:
        - wasi:keyvalue/store      # mid-chain placement, first-class
    - name: step2
      file: target/wasm32-wasip2/release/step2.wasm
  allowedHosts: []                 # explicit deny-all; never silently defaulted

dev:
  backends:
    keyvalue:
      filesystem: { path: ./data/keyvalue }
```

### A messaging fan-out with per-worker subscriptions

```yaml
workload:
  name: task-fanout
  components:
    - name: api
      build: true
      trigger:
        http: {}
    - name: task-leet
      file: target/wasm32-wasip2/release/task_leet.wasm
      trigger:
        messaging:
          subscriptions: [tasks.leet]     # a real list, not "a,b" in a KV map
          consumerGroup: leet-workers     # documented; was `consumer_group`
      config:
        leet.mode: aggressive             # app config stays app config
    - name: task-reverse
      file: target/wasm32-wasip2/release/task_reverse.wasm
      trigger:
        messaging:
          subscriptions: [tasks.reverse]

dev:
  backends:
    messaging:
      nats: { url: nats://127.0.0.1:4222 }   # omit the block for in-memory
```

The reader lowers `trigger.messaging` to the exact wire contract the runtime
already reads (`subscriptions` comma-joined, `consumer_group`); `wash-runtime`
is untouched.

### A service beside a component, over a socket

```yaml
workload:
  name: tcp-pipeline
  components:
    - name: http-api
      build: true
      trigger:
        http: {}
      config:
        leet.addr: "127.0.0.1:3000"   # a socket edge is app config; the
                                      # manifest honestly lists it under
                                      # `unresolved`, and that stays true
      allowedIpNameLookups: [localhost]
    - name: leet-service
      file: target/wasm32-wasip2/release/service_leet.wasm
      trigger:
        service:
          maxRestarts: 3              # Service.max_restarts, reachable at last
```

This replaces all four `dev.service*` fields: "the built component is the
service" is `build: true` + `trigger.service`; a separate service wasm is just
another component entry. (Services are wasip3; the wizard already enforces it.)

### A workload consuming a host component plugin

```yaml
workload:
  name: widgets
  components:
    - name: ingress
      build: true
      trigger:
        http: {}
      imports:
        - acme:widgets/store        # served by the plugin below

dev:
  hostPlugins:                      # requires a wash built with
    - id: acme-widgets              #   `host-component-plugins`
      image: ghcr.io/acme/widgets:1.2.0
      pullPolicy: ifNotPresent
      maxRestarts: 3
      secretFrom: [acme-creds]
      allowedHosts: [https://api.acme.dev]   # plugins stay deny-by-default

secrets:
  acme-creds:
    fromEnv: [ACME_TOKEN]
```

### Per-component policy (all of it, finally documented)

`allowedHosts`, `allowedIpNameLookups`, `allowedHostLoopbackPorts`,
`poolSize`, `maxInvocations`, `maxConcurrency` — every field the runtime
already honours per component, named in one place, with the replace-not-merge
rule stated: a per-component list replaces the workload default when present.

`imports:` entry forms: a bare string (`wasi:keyvalue/store`), or an object
`{ interface: ..., name: cache, config: { url: ..., prefix: ... } }` for
multiplexed named imports.

## Compatibility

**Additive evolution plus serde aliases. No schema-version key. A migrate
command instead of a flag day.**

- No version key because figment merges global config, project config, and
  `WASH_*` env vars into *one* document — two parse paths cannot merge.
- `BuildConfig`/`DevConfig`/`DevVolume` gain `rename_all = "camelCase"` with a
  snake_case `alias` per multi-word field. Reading accepts both; writers emit
  camelCase. Pinned-YAML tests parse every shipped template/example config
  verbatim.
- Old fields keep working. **Mixing old and new spellings of the same concern
  is a named error** (`workload.components` + `dev.components`;
  `dev.backends.keyvalue` + `wasi_keyvalue_redis_url`) — silent precedence
  between old and new is how config formats rot.
- `wash config migrate` (a later slice) rewrites a file canonically and prints
  the diff first.
- An old wash reading a new file silently ignores unknown keys; the support
  statement should say so out loud.

## Delivery slices

1. **Casing + this document.** Aliases, pinned-YAML compat tests, templates
   rewritten canonically. Nothing semantic moves.
2. **`workload.components` + `trigger`.** New structs and validation in
   `config.rs`; `wash dev`'s `create_workload` honours them; the wizard emits
   them; `--from` reads them. `dev.components`/`dev.service*` untouched.
3. **`dev.backends` + `wash config migrate`.** Typed one-of backend blocks;
   `postgres.database` gets a named field; deprecation notes.

## Decisions the team owns

1. **Where does per-workload interface config live long-term?**
   `postgres.database` and blobstore `buckets`/`readOnly` read locally as
   backend settings (`dev.backends`), but the operator delivers them as
   workload interface config (CRD territory). Placing them under `workload:`
   instead shapes the CRD mirror — decide with the operator maintainers.
2. **The egress default.** Keep omitted→allow-all in dev (ergonomic), warn
   loudly when a component imports `outgoing-handler` with the key omitted
   (this proposal), or flip to deny-all in the new shape only (secure, but the
   same key would mean different things in two places).
3. **Is `trigger.http` informational or enforced?** Informational — validation
   and docs only, host keeps deriving from exports (this proposal, runtime
   untouched) — or enforced routing, which is a runtime behaviour change with
   k8s implications.

# Benches: reuse before you write

Check this table before adding bench code. Everything in it has been rewritten
by hand inside a bench before, and that copy had to be deleted in review.

| Need | Use |
| --- | --- |
| In-process host with HTTP | `HostBuilder` + `Ingress::new(DynamicRouter::default(), addr)`; see `benches/common/mod.rs` (`start_host_and_workload`, `BenchHost`, `bench_client`) |
| Fixture wasm | `cargo xtask build-fixtures <name>` → `tests/wasm/<name>.wasm`; sources in `tests/fixtures/`; `common::Flavor::{P2,P3}.wasm()` |
| Host NATS API | `wash_runtime::washlet::types::v2::*` (generated, serde as proto-JSON) and `washlet::{rpc_subject, heartbeat_subject}` |
| NATS / OCI in tests | testcontainers `nats` (`tests/integration_washlet_api.rs::nats_endpoint`); `tests/common/oci_registry.rs`; `wash_runtime::oci::push_component` |
| Byte sizes | `engine::host_memory::{parse_bytes, render_bytes}` |
| Memory limit (cgroup ∧ RAM) | `engine::host_memory::detected_memory_limit()` |
| Footprint (self / another pid) | `engine::process_memory::{process_footprint, pid_footprint}`: the guard's own accounting |
| Pool and guest memory | `Engine::pool_usage()`, `pool_usage::pool_refusals()`, `Engine::guest_memory()` |
| Density ramps | `bench_tools::density`: `spread`, `percentile`, `ServingEvidence`, `memory_estimate`, `Report`/`Step`, `print_step`, `print_comparison` |
| Publishing | criterion → `target/criterion`, gungraun → `target/gungraun`, density → `<target>/density/<bench>.json`; `bench-tools jsonl`/`summary` turn these into trend-site rows. Add the bench to the `bench.yml` options; run it locally with `scripts/bench/run-bench.sh <bench>` |

Rules:

- Benches do not provision anything: no downloading binaries, spawning NATS or
  hosts, serving registries, or killing process trees. Benches in this crate
  build the host in-process. Out-of-process benches (`crates/bench-scale`)
  attach to infrastructure the caller started.
- If a helper exists but is private, make it `pub` in the runtime. Do not
  copy it.
- Each bench family has one record type and one renderer. To report a new
  figure, add a field to `density::Report`/`Step`; do not write a second reporter.
- `harness = false` benches must exit 0 when run without `--bench`, as
  `cargo test --benches` and nextest `--list` do; see `scale_density.rs` `main`.
- Tests of bench logic use no fixed sleeps. Wait on the event, or use
  `#[tokio::test(start_paused = true)]`.

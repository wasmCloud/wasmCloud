# Runtime parser fuzzing

The release train requests a run in the private
[`wasmCloud/security-fuzz`](https://github.com/wasmCloud/security-fuzz) repository.
It runs all 11 Go targets and the seven Rust targets below for five minutes each
on the release PR's exact merge commit. Sunday
runs in the private repository spend 30 minutes on each target. Manual runs use
the five-minute budget.

The release train dispatches after its PR merges and does not wait for results.
Dispatch failures produce a warning and do not fail the train. Promotion has
no dependency on fuzzing. The dispatch checks the private repository's ID,
visibility, and workflow before sending the full SHA and a unique request ID.
Public logs contain no crash details; maintainers review campaign results and
evidence in the private repository.

The private workflow checks out harnesses at an immutable revision and stages them
into a separate checkout of the selected release source. Recovery SHAs do not
need to contain the fuzz crate, Go fuzz tests, seeds, or the HTTP matcher shim.
The shim accesses the selected source's private router; it supplies no parser
implementation. Harnesses must remain compatible with the release's APIs;
incompatible releases fail the gate rather than skipping targets.

Rust parser dependency declarations come from the selected release's workspace.
`cargo xtask fuzz prepare` stages the harnesses and seeds their generated lockfile
from that release's `Cargo.lock`. After Cargo resolves the generated workspace,
`cargo xtask fuzz verify-lock` rejects any runtime dependency absent from the
release lockfile or with a different version, source, or checksum. This also
applies to automated dependency updates; there is no separate committed fuzz
lockfile to synchronize.

`Cargo.toml.in` is a template; preparation creates the runnable `Cargo.toml` in
the source checkout. The audit workflow stages this workspace and checks its
dependency licenses with the root `deny.toml` policy.

Each target has its own private corpus cache, refreshed after every run,
including failures. Rust seeds are committed under `seeds/`; Go seeds live in
the existing `*_fuzz_test.go` files. Failing inputs are retained as private
artifacts for 30 days; successful release evidence is retained for 90 days.
Triage crashes under [the security policy](../SECURITY.md). Publish reproducers
as regression seeds after any required coordinated disclosure.

## Private automation

[`private/`](private/) contains the reviewed control files deployed to the private
repository. See its [setup instructions](private/README.md) for authentication,
access controls, harness revisions, and updating the deployment. Public CI tests
the staging code and dispatch protocol without running discovery fuzzing.

| Rust target | Surface |
| --- | --- |
| `workload_spec` | Generated protobuf JSON used by NATS workload commands, nested resource policy conversion |
| `protobuf` | Binary protobuf decoding, encode/decode round trips, nested workload conversion |
| `allowed_hosts` | Allowlist parsing and HTTP, name, and socket matching |
| `address_policy` | Address range checks, including IPv4-mapped IPv6 canonicalization |
| `oci_reference` | OCI reference and digest grammar, component source classification, digest pin checks |
| `http_route` | Local route parsing, path boundaries, host matching, route scope isolation and removal |
| `host_interfaces` | WIT shorthand, generated host interface JSON, wire conversion, version parsing and matching |

Rust targets depend on the production `wash-runtime` crate with only `washlet`
and its required OCI feature enabled. Harnesses do no network I/O; component
source classification checks whether a local path exists.
The HTTP matcher shim is compiled only under `cfg(fuzzing)`.

The byte-slice adapter links Clang's libFuzzer archive directly. It does not use
`libfuzzer-sys` or its bundled NCSA runtime. LLVM's runtime uses the
[Apache-2.0 WITH LLVM-exception license](https://github.com/llvm/llvm-project/blob/main/LICENSE.TXT).
Install `clang` and `libclang-rt-dev` on Ubuntu, or Homebrew `llvm` on macOS.
Set `LLVM_LIBFUZZER_PATH` to that installation's archive before building:

```sh
# Linux x86_64
export LLVM_LIBFUZZER_PATH=$(clang --print-file-name=libclang_rt.fuzzer-x86_64.a)
# macOS with Homebrew LLVM
export LLVM_LIBFUZZER_PATH=$(/opt/homebrew/opt/llvm/bin/clang --print-file-name=libclang_rt.fuzzer_osx.a)
```

The private workflow builds every Rust target once on a dated nightly, set as
`RUSTUP_TOOLCHAIN` in `private/fuzz.yml`, and then fuzzes each binary in its
own job. Advance that date deliberately; an unpinned nightly can fail the build
with new warnings. Public CI's `wash` workflow compiles the harnesses against
each change, so API drift fails there rather than in a private campaign.

Install nightly with `rust-src` and cargo-fuzz, then prepare a disposable source
checkout. Run from the harness repository root, replacing `<release-sha>` with
the commit to test:

```sh
rustup toolchain install nightly --profile minimal --component rust-src
cargo install cargo-fuzz --version 0.13.2 --locked
FUZZ_SOURCE=$(mktemp -d)
git archive <release-sha> | tar -x -C "$FUZZ_SOURCE"
cargo xtask fuzz prepare --source "$FUZZ_SOURCE" --language rust
cargo +nightly metadata --manifest-path "$FUZZ_SOURCE/fuzz/Cargo.toml" --format-version 1 > /dev/null
cargo xtask fuzz verify-lock --source "$FUZZ_SOURCE"
cd "$FUZZ_SOURCE"
mkdir -p fuzz/corpus/http_route
cp fuzz/seeds/http_route/* fuzz/corpus/http_route/
cargo +nightly fuzz run http_route --sanitizer address -- -max_total_time=300
```

Reproduce a Rust failure in the prepared source checkout with:

```sh
cargo +nightly fuzz run http_route fuzz/artifacts/http_route/crash-<hash>
```

From the harness repository root, stage Go harnesses into the source checkout
before running each target separately:

```sh
cargo xtask fuzz prepare --source "$FUZZ_SOURCE" --language go
cd "$FUZZ_SOURCE/runtime-operator"
go test ./api/runtime/v1alpha1 -run='^$' -fuzz='^FuzzWorkloadSpecJSON$' -fuzztime=5m
```

Add new targets to `private/policy.json` as well as the relevant manifest or test
file, and deploy the updated policy to the private repository. Use longer
scheduled runs to expand the private corpus and investigate failures privately.

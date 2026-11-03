# Runtime parser fuzzing

`fuzz.yml` runs all 11 Go targets and the seven Rust targets below for five
minutes each before release promotion. The release gate resolves the commit
once; fuzzing and promotion both check out that SHA. A failed or skipped fuzz
job blocks promotion. Sunday runs spend 30 minutes on each target. Manual runs
use the five-minute budget.

The workflow checks out harnesses at its own immutable revision and stages them
into a separate checkout of the selected release source. Recovery SHAs do not
need to contain the fuzz crate, Go fuzz tests, seeds, or the HTTP matcher shim.
The shim accesses the selected source's private router; it supplies no parser
implementation. Harnesses must remain compatible with the release's APIs;
incompatible releases fail the gate rather than skipping targets.

Rust parser dependency declarations come from the selected release's workspace.
Preparation seeds the generated fuzz lockfile from that release's `Cargo.lock`.
After Cargo resolves the added fuzz tooling, verification rejects any runtime
dependency absent from the release lockfile or with a different version, source,
or checksum. This also applies to automated dependency updates; there is no
separate committed fuzz lockfile to synchronize.

Each target has its own corpus cache, refreshed after every run, including
failures. Rust seeds are committed under `seeds/`; Go seeds live in the existing
`*_fuzz_test.go` files. Failing inputs are uploaded as workflow artifacts. Copy
reproducers into the seed directories (Rust) or `testdata/fuzz/<target>/` (Go)
when fixing a finding.

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
and its required OCI feature enabled. Harnesses do no network or filesystem I/O.
The HTTP matcher shim is compiled only under `cfg(fuzzing)`.

Install nightly with `rust-src` and cargo-fuzz, then prepare a disposable source
checkout. Run from the harness repository root, replacing `<release-sha>` with
the commit to test:

```sh
rustup toolchain install nightly --profile minimal --component rust-src
cargo install cargo-fuzz --version 0.13.2 --locked
FUZZ_SOURCE=$(mktemp -d)
git archive <release-sha> | tar -x -C "$FUZZ_SOURCE"
python3 .github/scripts/release-fuzz.py prepare --source "$FUZZ_SOURCE" --language rust
cargo +nightly metadata --manifest-path "$FUZZ_SOURCE/fuzz/Cargo.toml" --format-version 1 > /dev/null
python3 .github/scripts/release-fuzz.py verify-lock --source "$FUZZ_SOURCE"
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
python3 .github/scripts/release-fuzz.py prepare --source "$FUZZ_SOURCE" --language go
cd "$FUZZ_SOURCE/runtime-operator"
go test ./api/runtime/v1alpha1 -run='^$' -fuzz='^FuzzWorkloadSpecJSON$' -fuzztime=5m
```

Add new targets to the workflow matrix as well as the relevant manifest or test
file. Keep release fuzzing mandatory; use longer scheduled runs to expand the
cached corpus.

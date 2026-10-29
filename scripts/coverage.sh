#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export CARGO_PROFILE_DEV_DEBUG=line-tables-only
export CARGO_PROFILE_TEST_DEBUG=line-tables-only
export CARGO_INCREMENTAL=0

features=wash-runtime/wasi-tls,wash/host-component-plugins
report_dir=target/coverage
# Generated protobuf bindings have no maintained application logic.
report_args=(--ignore-filename-regex '[/\\]washlet[/\\]generated[/\\]')

case "${1:-all}" in
  test)
    rm -rf "$report_dir"
    mkdir -p "$report_dir"
    {
      date -u '+Collected: %Y-%m-%dT%H:%M:%SZ'
      git rev-parse HEAD
      git status --short
      rustc -vV
      cargo llvm-cov --version
      echo "Features: $features"
      echo 'Tests: workspace unit and integration tests, including ignored tests'
      echo 'Scope: native Rust workspace; generated protobuf bindings excluded'
      if [[ -n "${GITHUB_RUN_ID:-}" ]]; then
        echo "Run: $GITHUB_SERVER_URL/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID"
      fi
    } > "$report_dir/provenance.txt"
    cargo llvm-cov clean --workspace
    # Build guests before cargo-llvm-cov enables native instrumentation.
    cargo xtask build-fixtures
    test_status=0
    cargo llvm-cov --locked --workspace --features "$features" --no-report --no-fail-fast -- --include-ignored \
      2>&1 | tee "$report_dir/tests.txt" || test_status=$?
    echo "Test exit status: $test_status" >> "$report_dir/provenance.txt"
    if [[ "$test_status" != 0 ]]; then
      exit "$test_status"
    fi
    touch "$report_dir/tests-passed"
    ;;
  report)
    mkdir -p "$report_dir"
    cargo llvm-cov report "${report_args[@]}" | tee "$report_dir/summary.txt"
    cargo llvm-cov report "${report_args[@]}" --json --summary-only --output-path "$report_dir/coverage.json"
    cargo llvm-cov report "${report_args[@]}" --lcov --output-path "$report_dir/lcov.info"
    cargo llvm-cov report "${report_args[@]}" --html --output-dir "$report_dir"
    ;;
  check)
    if [[ ! -f "$report_dir/tests-passed" ]]; then
      echo 'Coverage evidence requires a successful full test run.' >&2
      exit 1
    fi
    cargo llvm-cov report "${report_args[@]}" --fail-under-lines 80 --fail-under-regions 80 \
      2>&1 | tee "$report_dir/threshold.txt"
    ;;
  all)
    # Keep diagnostics when tests or the threshold check fail.
    test_status=0
    bash scripts/coverage.sh test || test_status=$?
    bash scripts/coverage.sh report
    if [[ "$test_status" != 0 ]]; then
      exit "$test_status"
    fi
    bash scripts/coverage.sh check
    ;;
  *)
    echo 'Usage: bash scripts/coverage.sh [all|test|report|check]' >&2
    exit 2
    ;;
esac

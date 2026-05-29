#!/usr/bin/env bash
# Pull every per-run results.jsonl from s3://${WASMCLOUD_BENCH_S3_BUCKET}/runs/, dedupe,
# sort, and emit history.json — a rebuild-from-scratch path for the public
# aggregate that .github/scripts/bench-push-results.mjs maintains
# incrementally. Use this when the
# incremental file gets out of sync, after schema changes, or to seed a
# fresh deployment of the trend site (wasmCloud/arewefastyet).
#
# Usage:
#   WASMCLOUD_BENCH_S3_BUCKET=<bucket> ./scripts/bench/build-history.sh
#
# Optional:
#   WASMCLOUD_BENCH_HISTORY_OUT  - output path (default ./history.json)
#   WASMCLOUD_BENCH_HISTORY_MAX_AGE_DAYS  - drop rows older than this (default 365)

set -euo pipefail

: "${WASMCLOUD_BENCH_S3_BUCKET:?WASMCLOUD_BENCH_S3_BUCKET not set}"

out="${WASMCLOUD_BENCH_HISTORY_OUT:-./history.json}"
max_age_days="${WASMCLOUD_BENCH_HISTORY_MAX_AGE_DAYS:-365}"

mkdir -p "$(dirname "$out")"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

echo "syncing results.jsonl from s3://${WASMCLOUD_BENCH_S3_BUCKET}/runs/ → ${work}"
aws s3 sync --no-progress \
  --exclude '*' --include '*results.jsonl' \
  "s3://${WASMCLOUD_BENCH_S3_BUCKET}/runs/" "${work}/"

# Concatenate and JSON-encode. unique_by(.sha + .group + .param + .run_attempt)
# in case the same row was uploaded twice; sort_by(.timestamp) so the frontend
# can iterate in chronological order.
#
# We deliberately let `cat` fail loudly if any per-run results.jsonl is
# unreadable — this is a recovery tool, you'd reach for it precisely
# *because* state is wrong, and silently dropping rows would defeat the
# point. Fix the unreadable file and re-run.
{
  find "$work" -type f -name results.jsonl -print0 \
    | xargs -0 cat
} \
  | jq -c --argjson max_age "$max_age_days" '
      select(. != null) |
      select(.timestamp != null) |
      select(((now - (.timestamp | fromdate)) / 86400) <= $max_age)
    ' \
  | jq -s 'unique_by([.sha, .bench, .group, .param, .run_attempt, (.metric // null)]) | sort_by(.timestamp)' \
  > "$out"

count=$(jq 'length' < "$out")
benches=$(jq -r '[.[] | .bench] | unique | join(", ")' < "$out")
echo "wrote ${count} rows to ${out}"
echo "  benches: ${benches:-<none>}"
echo "  bytes:   $(wc -c < "$out")"

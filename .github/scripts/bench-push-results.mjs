#!/usr/bin/env node
// Upload one bench run's artifacts to S3, then update the public
// history.json aggregate and invalidate CloudFront.
//
// Heavy JSON construction + history.json read-merge-write live here in
// JS where they're cleaner than jq-in-bash; the actual S3/CloudFront
// calls shell out to the `aws` CLI (already on the bench host's PATH).
//
// Per-run layout (private; only the bench role can read):
//   s3://${WASMCLOUD_BENCH_S3_BUCKET}/runs/<date>/<short-sha>/<run-id>/<bench>/
//     ├─ criterion.tar.zst   raw criterion data
//     ├─ iai.tar.zst         raw iai-callgrind data (when applicable)
//     ├─ results.jsonl       one JSON row per (group, param, metric)
//     ├─ metadata.json       run-level facts
//     └─ run.log             cargo bench stdout/stderr
//
// Aggregate (publicly readable through CloudFront):
//   s3://${WASMCLOUD_BENCH_S3_BUCKET}/history.json
//     - JSON array of every (group, param, metric) row from every run,
//       deduped on (sha, bench, group, param, run_attempt, metric),
//       sorted by timestamp.
//     - Cache-Control: max-age=60.
//     - CloudFront invalidation issued after each push.
//
// Reads (required):
//   WASMCLOUD_BENCH_NAME                 bench whose output we're uploading
//   WASMCLOUD_BENCH_S3_BUCKET            target bucket
//   WASMCLOUD_BENCH_CF_DISTRIBUTION_ID   CloudFront distribution to invalidate
//
// Reads (optional):
//   CARGO_TARGET_DIR        default /var/lib/bench/target
//   GITHUB_RUN_ID           default "local"
//   GITHUB_RUN_ATTEMPT      default "1"
//   GITHUB_REF_NAME         default `git rev-parse --abbrev-ref HEAD`
//   GITHUB_ACTOR/_EVENT_NAME/_WORKFLOW/_SERVER_URL/_REPOSITORY  — for metadata.json

import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, unlinkSync, writeFileSync } from 'node:fs';
import { hostname, tmpdir } from 'node:os';
import { basename, join } from 'node:path';

// rustup is in $HOME/.cargo/bin (not on the global PATH unless the user
// shell sources $HOME/.cargo/env). Prepend it here so the `cargo run -p
// bench-tools` call below resolves without us having to wrap it in `sh
// -c '. ~/.cargo/env && …'`.
process.env.PATH = `${process.env.HOME}/.cargo/bin:${process.env.PATH ?? ''}`;

const bench = required('WASMCLOUD_BENCH_NAME');
const bucket = required('WASMCLOUD_BENCH_S3_BUCKET');
const distId = required('WASMCLOUD_BENCH_CF_DISTRIBUTION_ID');

const targetDir = process.env.CARGO_TARGET_DIR ?? '/var/lib/bench/target';
const critDir = join(targetDir, 'criterion');
const iaiDir = join(targetDir, 'iai');

const runId = process.env.GITHUB_RUN_ID ?? 'local';
const sha = run('git', ['rev-parse', 'HEAD']);
const shortSha = run('git', ['rev-parse', '--short=12', 'HEAD']);
const date = new Date().toISOString().slice(0, 10); // YYYY-MM-DD
const prefix = `runs/${date}/${shortSha}/${runId}/${bench}`;

const work = mkdtempSync(join(tmpdir(), 'push-bench-'));
process.on('exit', () => {
  try {
    rmSync(work, { recursive: true, force: true });
  } catch {
    // best-effort
  }
});

// 1. Tar+zstd the bench-specific output dirs. `tar -I "zstd -19 -T0"`
//    pipes the archive through external zstd at level 19, matching what
//    the bash version did via `tar | zstd`. tar's bundled `--zstd` flag
//    uses level 3 — meaningful compression-ratio difference on criterion
//    output, so we pin to -19 explicitly.
let archived = 0;
if (existsSync(critDir)) {
  run('tar', ['-cf', join(work, 'criterion.tar.zst'), '-I', 'zstd -19 -T0', '-C', critDir, '.']);
  archived++;
}
if (existsSync(iaiDir)) {
  run('tar', ['-cf', join(work, 'iai.tar.zst'), '-I', 'zstd -19 -T0', '-C', iaiDir, '.']);
  archived++;
}
if (archived === 0) {
  console.log(`::warning::no criterion or iai output at ${targetDir}; uploading metadata only`);
}

// 2. Per-(group, param) JSONL rows for trend ingestion. bench-tools jsonl
//    emits an empty stream for benches whose layout doesn't feed
//    history.json today, which keeps the branch in one place (the Rust
//    binary) instead of being repeated here.
const jsonl = run('cargo', ['run', '-p', 'bench-tools', '--quiet', '--', 'jsonl', '--bench', bench]);
writeFileSync(join(work, 'results.jsonl'), jsonl ? `${jsonl}\n` : '');

// 3. Run-level metadata.
const cpuModel = readFirstModelName('/proc/cpuinfo');
const metadata = {
  bench,
  run_id: runId,
  run_attempt: process.env.GITHUB_RUN_ATTEMPT ?? '1',
  workflow: process.env.GITHUB_WORKFLOW ?? 'bench',
  event: process.env.GITHUB_EVENT_NAME ?? '',
  actor: process.env.GITHUB_ACTOR ?? '',
  ref: process.env.GITHUB_REF_NAME || run('git', ['rev-parse', '--abbrev-ref', 'HEAD']),
  sha,
  short_sha: shortSha,
  timestamp: rfc3339Now(),
  run_url:
    `${process.env.GITHUB_SERVER_URL ?? 'https://github.com'}/${process.env.GITHUB_REPOSITORY ?? ''}` +
    `/actions/runs/${runId}`,
  host: hostname(),
  kernel: run('uname', ['-r']),
  cpu: cpuModel,
  cpus_online: parseInt(run('nproc'), 10),
};
writeFileSync(join(work, 'metadata.json'), `${JSON.stringify(metadata, null, 2)}\n`);

// 4. Pick up the run log written by run-bench.sh, if present.
const logSrc = join(targetDir, `run-${bench}-${runId}.log`);
if (existsSync(logSrc)) {
  run('cp', [logSrc, join(work, 'run.log')]);
}

// 5. Upload per-run artifacts.
console.log(`uploading per-run artifacts to s3://${bucket}/${prefix}/`);
for (const file of [
  'criterion.tar.zst',
  'iai.tar.zst',
  'results.jsonl',
  'metadata.json',
  'run.log',
]) {
  const path = join(work, file);
  if (!existsSync(path)) continue;
  run('aws', ['s3', 'cp', '--no-progress', path, `s3://${bucket}/${prefix}/${basename(path)}`]);
}

// 6. Drop the source log now that it's archived in S3.
if (existsSync(logSrc)) {
  unlinkSync(logSrc);
}

// 7. Read-modify-write the public history.json aggregate. Safe without
//    locking because the workflow is `concurrency: bench-host` — there's
//    only ever one writer.
console.log(`updating s3://${bucket}/history.json`);
let existing = [];
const head = spawnSync('aws', ['s3api', 'head-object', '--bucket', bucket, '--key', 'history.json'], {
  stdio: 'ignore',
});
if (head.status === 0) {
  const histPath = join(work, 'history-existing.json');
  run('aws', ['s3', 'cp', '--no-progress', `s3://${bucket}/history.json`, histPath]);
  existing = JSON.parse(readFileSync(histPath, 'utf8'));
}

const newRows = jsonl
  .split('\n')
  .filter((line) => line.length > 0)
  .map((line) => JSON.parse(line));

// Dedup key matches what build-history.sh uses: (sha, bench, group,
// param, run_attempt, metric). Rows from before the metric-field schema
// bump lack `.metric`; those compare as `null` and still unique correctly
// within the criterion subset.
const dedupKey = (r) =>
  JSON.stringify([r.sha, r.bench, r.group, r.param, r.run_attempt, r.metric ?? null]);
const merged = new Map();
for (const row of existing) merged.set(dedupKey(row), row);
for (const row of newRows) merged.set(dedupKey(row), row); // new rows win on collision
const final = [...merged.values()].sort((a, b) => a.timestamp.localeCompare(b.timestamp));

const histOut = join(work, 'history.json');
writeFileSync(histOut, JSON.stringify(final));

run('aws', [
  's3',
  'cp',
  '--no-progress',
  '--content-type',
  'application/json',
  '--cache-control',
  'public, max-age=60',
  histOut,
  `s3://${bucket}/history.json`,
]);

// 8. Invalidate CloudFront so the next request hits a fresh edge cache.
console.log('invalidating CloudFront /history.json');
const invalidationId = run('aws', [
  'cloudfront',
  'create-invalidation',
  '--distribution-id',
  distId,
  '--paths',
  '/history.json',
  '--query',
  'Invalidation.Id',
  '--output',
  'text',
]);
console.log(`invalidation: ${invalidationId}`);

console.log(
  `::notice title=bench results::s3://${bucket}/${prefix}/  (history now ${final.length} rows)`,
);

// ─── helpers ────────────────────────────────────────────────────────────

function required(name) {
  const v = process.env[name];
  if (!v) {
    console.error(`${name} not set`);
    process.exit(1);
  }
  return v;
}

function run(cmd, args = []) {
  return execFileSync(cmd, args, { encoding: 'utf8' }).trim();
}

function readFirstModelName(path) {
  // /proc/cpuinfo has "model name : <name>" on Intel/AMD. Match the awk
  // pipeline the bash version used: first occurrence, leading-space-stripped.
  for (const line of readFileSync(path, 'utf8').split('\n')) {
    const m = line.match(/^model name\s*:\s*(.+)$/);
    if (m) return m[1].trim();
  }
  return '';
}

function rfc3339Now() {
  // 2026-05-13T12:34:56Z — matches `date -u +%FT%TZ` and what
  // bench-tools::Meta::capture() emits, so all timestamp fields agree.
  return new Date().toISOString().replace(/\.\d{3}Z$/, 'Z');
}

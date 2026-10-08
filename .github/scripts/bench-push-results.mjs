#!/usr/bin/env node
// Prepare bench results on the bench host, then publish them from a separate job.

import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, lstatSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { hostname, tmpdir } from 'node:os';
import { basename, join } from 'node:path';

const [mode, bundle] = process.argv.slice(2);
if (!bundle || !['prepare', 'validate', 'publish'].includes(mode)) {
  throw new Error('usage: bench-push-results.mjs <prepare|validate|publish> <bundle-dir>');
}

if (mode === 'prepare') {
  prepare(bundle);
} else if (mode === 'validate') {
  validate(bundle);
} else {
  publish(bundle);
}

function prepare(out) {
  process.env.PATH = `${process.env.HOME}/.cargo/bin:${process.env.PATH ?? ''}`;
  const bench = required('WASMCLOUD_BENCH_NAME');
  const k6Dir = process.env.WASMCLOUD_BENCH_K6_DIR;
  const targetDir = process.env.CARGO_TARGET_DIR ?? '/var/lib/bench/target';
  const runId = process.env.GITHUB_RUN_ID ?? 'local';

  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });

  let archived = 0;
  if (k6Dir) {
    run('tar', ['-cf', join(out, 'k6.tar.zst'), '-I', 'zstd -19 -T0', '-C', k6Dir, '.']);
    archived++;
  } else if (existsSync(join(targetDir, 'criterion'))) {
    run('tar', ['-cf', join(out, 'criterion.tar.zst'), '-I', 'zstd -19 -T0', '-C', join(targetDir, 'criterion'), '.']);
    archived++;
  }
  if (!k6Dir && existsSync(join(targetDir, 'gungraun'))) {
    run('tar', ['-cf', join(out, 'gungraun.tar.zst'), '-I', 'zstd -19 -T0', '-C', join(targetDir, 'gungraun'), '.']);
    archived++;
  }
  if (archived === 0) {
    console.log(`::warning::no bench output at ${targetDir}; uploading metadata only`);
  }

  const jsonl = k6Dir
    ? run('cargo', ['run', '-p', 'bench-tools', '--quiet', '--', 'k6', 'jsonl', k6Dir])
    : run('cargo', ['run', '-p', 'bench-tools', '--quiet', '--', 'jsonl', '--bench', bench]);
  writeFileSync(join(out, 'results.jsonl'), jsonl ? `${jsonl}\n` : '');

  const sha = run('git', ['rev-parse', 'HEAD']);
  const metadata = {
    bench,
    run_id: runId,
    run_attempt: process.env.GITHUB_RUN_ATTEMPT ?? '1',
    workflow: process.env.GITHUB_WORKFLOW ?? 'bench',
    event: process.env.GITHUB_EVENT_NAME ?? '',
    actor: process.env.GITHUB_ACTOR ?? '',
    ref: process.env.WASMCLOUD_BENCH_REF ?? process.env.GITHUB_REF_NAME ?? run('git', ['rev-parse', '--abbrev-ref', 'HEAD']),
    sha,
    short_sha: run('git', ['rev-parse', '--short=12', 'HEAD']),
    timestamp: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
    run_url:
      `${process.env.GITHUB_SERVER_URL ?? 'https://github.com'}/${process.env.GITHUB_REPOSITORY ?? ''}` +
      `/actions/runs/${runId}`,
    host: hostname(),
    kernel: run('uname', ['-r']),
    cpu: readFirstModelName('/proc/cpuinfo'),
    cpus_online: parseInt(run('nproc'), 10),
    ...(k6Dir ? { k6: JSON.parse(readFileSync(join(k6Dir, 'metadata.json'), 'utf8')) } : {}),
  };
  writeFileSync(join(out, 'metadata.json'), `${JSON.stringify(metadata, null, 2)}\n`);

  const logSrc = k6Dir ? join(k6Dir, 'run.log') : join(targetDir, `run-${bench}-${runId}.log`);
  if (existsSync(logSrc)) {
    writeFileSync(join(out, 'run.log'), readFileSync(logSrc));
  }
}

function publish(bundle) {
  const { metadata, rows } = validate(bundle);
  const bucket = required('WASMCLOUD_BENCH_S3_BUCKET');
  const distId = required('WASMCLOUD_BENCH_CF_DISTRIBUTION_ID');
  const prefix = `runs/${metadata.timestamp.slice(0, 10)}/${metadata.short_sha}/${metadata.run_id}/${metadata.bench}`;
  const work = join(tmpdir(), `bench-publish-${process.pid}`);
  mkdirSync(work, { recursive: true });
  try {
    console.log(`uploading per-run artifacts to s3://${bucket}/${prefix}/`);
    for (const file of ['criterion.tar.zst', 'gungraun.tar.zst', 'k6.tar.zst', 'results.jsonl', 'metadata.json', 'run.log']) {
      const path = join(bundle, file);
      if (existsSync(path)) {
        run('aws', ['s3', 'cp', '--no-progress', path, `s3://${bucket}/${prefix}/${basename(path)}`]);
      }
    }

    const newRows = rows.filter((row) => !row.generator_saturated);
    const dedupKey = (row) =>
      JSON.stringify([row.sha, row.bench, row.group, row.param, row.run_attempt, row.metric ?? null]);
    const histOut = join(work, 'history.json');
    let final;
    for (let attempt = 0; attempt < 20; attempt++) {
      const { rows, etag } = readHistory(bucket, work);
      const merged = new Map();
      for (const row of rows) merged.set(dedupKey(row), row);
      for (const row of newRows) merged.set(dedupKey(row), row);
      final = [...merged.values()].sort((a, b) => a.timestamp.localeCompare(b.timestamp));
      writeFileSync(histOut, JSON.stringify(final));
      const put = spawnSync('aws', [
        's3api', 'put-object', '--bucket', bucket, '--key', 'history.json',
        '--body', histOut, '--content-type', 'application/json',
        '--cache-control', 'public, max-age=60',
        ...(etag ? ['--if-match', etag] : ['--if-none-match', '*']),
      ], { encoding: 'utf8' });
      if (put.status === 0) break;
      if (!/PreconditionFailed|ConditionalRequestConflict|\(412\)|\(409\)/.test(put.stderr ?? '')) {
        throw new Error(`could not update history.json: ${put.stderr ?? put.error}`);
      }
      if (attempt === 19) throw new Error('history.json changed during every retry');
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 100 + Math.random() * 400);
    }
    const invalidationId = run('aws', [
      'cloudfront', 'create-invalidation', '--distribution-id', distId,
      '--paths', '/history.json', '--query', 'Invalidation.Id', '--output', 'text',
    ]);
    console.log(`invalidation: ${invalidationId}`);
    console.log(`::notice title=bench results::s3://${bucket}/${prefix}/  (history now ${final.length} rows)`);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

function validate(bundle) {
  for (const file of ['metadata.json', 'results.jsonl']) {
    if (!lstatSync(join(bundle, file)).isFile()) throw new Error(`invalid artifact file: ${file}`);
  }
  for (const file of ['criterion.tar.zst', 'gungraun.tar.zst', 'k6.tar.zst', 'run.log']) {
    if (existsSync(join(bundle, file)) && !lstatSync(join(bundle, file)).isFile()) {
      throw new Error(`invalid artifact file: ${file}`);
    }
  }
  const metadata = JSON.parse(readFileSync(join(bundle, 'metadata.json'), 'utf8'));
  const expectedBench = required('WASMCLOUD_BENCH_NAME');
  const expectedRef = required('WASMCLOUD_BENCH_REF');
  const expectedSha = required('WASMCLOUD_BENCH_SHA');
  const expectedRowBench = process.env.WASMCLOUD_BENCH_ROW_NAME ?? expectedBench;
  if (metadata.bench !== expectedBench || !/^[a-z0-9][a-z0-9_-]*$/.test(expectedBench)) {
    throw new Error('artifact bench does not match this job');
  }
  if (metadata.ref !== expectedRef) {
    throw new Error('artifact ref does not match this job');
  }
  if (metadata.run_id !== required('GITHUB_RUN_ID') || !/^\d+$/.test(metadata.run_id)) {
    throw new Error('artifact run ID does not match this job');
  }
  if (metadata.run_attempt !== required('GITHUB_RUN_ATTEMPT') || !/^\d+$/.test(metadata.run_attempt)) {
    throw new Error('artifact attempt does not match this job');
  }
  if (metadata.sha !== expectedSha || !/^[a-f0-9]{40}$/.test(metadata.sha) ||
      !/^[a-f0-9]{12,40}$/.test(metadata.short_sha) || !metadata.sha.startsWith(metadata.short_sha)) {
    throw new Error('invalid artifact commit');
  }
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(metadata.timestamp)) {
    throw new Error('invalid artifact timestamp');
  }
  const rows = readFileSync(join(bundle, 'results.jsonl'), 'utf8')
    .split('\n')
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line));
  for (const row of rows) {
    if (!row || typeof row !== 'object' || Array.isArray(row) ||
        row.bench !== expectedRowBench || row.sha !== metadata.sha ||
        row.short_sha !== metadata.short_sha || row.ref !== metadata.ref ||
        row.run_id !== metadata.run_id || row.run_attempt !== metadata.run_attempt) {
      throw new Error('artifact row does not match this job');
    }
  }
  return { metadata, rows };
}

function readHistory(bucket, work) {
  const path = join(work, 'history-existing.json');
  const get = spawnSync('aws', ['s3api', 'get-object', '--bucket', bucket, '--key', 'history.json', path], {
    encoding: 'utf8',
  });
  if (get.status !== 0) {
    if (!/\(404\)|Not Found|NoSuchKey/.test(get.stderr ?? '')) {
      throw new Error(`could not read history.json: ${get.stderr ?? get.error}`);
    }
    return { rows: [], etag: null };
  }
  return { rows: JSON.parse(readFileSync(path, 'utf8')), etag: JSON.parse(get.stdout).ETag };
}

function required(name) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} not set`);
  return value;
}

function run(cmd, args = []) {
  return execFileSync(cmd, args, { encoding: 'utf8' }).trim();
}

function readFirstModelName(path) {
  for (const line of readFileSync(path, 'utf8').split('\n')) {
    const match = line.match(/^model name\s*:\s*(.+)$/);
    if (match) return match[1].trim();
  }
  return '';
}

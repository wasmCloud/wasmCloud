import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const script = fileURLToPath(new URL('./bench-push-results.mjs', import.meta.url));

test('publishes the data artifact and retries a conflicting history write', () => {
  const root = mkdtempSync(join(tmpdir(), 'bench-publish-test-'));
  try {
    const bin = join(root, 'bin');
    const bundle = join(root, 'bundle');
    mkdirSync(bin);
    mkdirSync(bundle);
    const row = {
      sha: 'a'.repeat(40), short_sha: 'a'.repeat(12), ref: 'main',
      run_id: '123', run_attempt: '1', bench: 'http_invoke',
      group: 'g', param: 'p', metric: 'mean_ns', timestamp: '2026-10-06T12:00:00Z',
    };
    const competing = { ...row, sha: 'b'.repeat(40) };
    // Same key as `row` but measured on another host: a different series.
    const otherHost = { ...row, host_label: 'aws-c7i' };
    writeFileSync(join(bundle, 'metadata.json'), JSON.stringify({
      bench: 'http_invoke',
      run_id: '123',
      run_attempt: '1',
      ref: 'main',
      sha: 'a'.repeat(40),
      short_sha: 'a'.repeat(12),
      timestamp: '2026-10-06T12:00:00Z',
      host_label: 'hetzner',
    }));
    writeFileSync(join(bundle, 'results.jsonl'), JSON.stringify(row) + '\n');
    writeFileSync(join(root, 'competing.json'), JSON.stringify([competing, otherHost]));
    writeFileSync(join(root, 'conflict'), '');
    writeFileSync(join(bin, 'aws'), `#!/usr/bin/env node
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
const args = process.argv.slice(2);
const root = process.env.MOCK_S3;
const hist = join(root, 'history.json');
const value = (flag) => args[args.indexOf(flag) + 1];
const etag = () => '"' + createHash('md5').update(readFileSync(hist)).digest('hex') + '"';
if (args[0] === 's3' && args[1] === 'cp') {
  const src = args.at(-2);
  const dst = args.at(-1).replace('s3://', '');
  const path = join(root, dst);
  mkdirSync(path.slice(0, path.lastIndexOf('/')), { recursive: true });
  copyFileSync(src, path);
} else if (args[0] === 's3api' && args[1] === 'get-object') {
  if (!existsSync(hist)) {
    process.stderr.write('NoSuchKey');
    process.exit(1);
  }
  copyFileSync(hist, args.at(-1));
  process.stdout.write(JSON.stringify({ ETag: etag() }));
} else if (args[0] === 's3api' && args[1] === 'put-object') {
  if (existsSync(join(root, 'conflict'))) {
    copyFileSync(join(root, 'competing.json'), hist);
    rmSync(join(root, 'conflict'));
    process.stderr.write('PreconditionFailed');
    process.exit(1);
  }
  if (args.includes('--if-none-match') ? existsSync(hist) : value('--if-match') !== etag()) {
    process.stderr.write('PreconditionFailed');
    process.exit(1);
  }
  copyFileSync(value('--body'), hist);
} else if (args[0] === 'cloudfront') {
  process.stdout.write('I123');
} else {
  throw new Error('unexpected aws call: ' + args.join(' '));
}
`);
    chmodSync(join(bin, 'aws'), 0o755);
    const env = {
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      MOCK_S3: root,
      WASMCLOUD_BENCH_NAME: 'http_invoke',
      WASMCLOUD_BENCH_REF: 'main',
      WASMCLOUD_BENCH_SHA: 'a'.repeat(40),
      WASMCLOUD_BENCH_S3_BUCKET: 'bucket',
      WASMCLOUD_BENCH_CF_DISTRIBUTION_ID: 'distribution',
      GITHUB_RUN_ID: '123',
      GITHUB_RUN_ATTEMPT: '1',
    };
    const validate = spawnSync(process.execPath, [script, 'validate', bundle], { env, encoding: 'utf8' });
    assert.equal(validate.status, 0, validate.stderr);
    const publish = spawnSync(process.execPath, [script, 'publish', bundle], { env, encoding: 'utf8' });
    assert.equal(publish.status, 0, publish.stderr);
    const history = JSON.parse(readFileSync(join(root, 'history.json'), 'utf8'));
    assert.equal(history.length, 3);
    assert.deepEqual(history.filter((r) => r.sha === row.sha).map((r) => r.host_label ?? 'hetzner').sort(), ['aws-c7i', 'hetzner']);
    assert.equal(readFileSync(join(root, 'bucket/runs/2026-10-06/aaaaaaaaaaaa/123/http_invoke/results.jsonl'), 'utf8'), JSON.stringify(row) + '\n');

    writeFileSync(join(bundle, 'results.jsonl'), JSON.stringify({ ...row, sha: 'b'.repeat(40) }) + '\n');
    const forged = spawnSync(process.execPath, [script, 'validate', bundle], { env, encoding: 'utf8' });
    assert.notEqual(forged.status, 0);

    rmSync(join(bundle, 'results.jsonl'));
    symlinkSync(join(root, 'history.json'), join(bundle, 'results.jsonl'));
    const invalid = spawnSync(process.execPath, [script, 'validate', bundle], { env, encoding: 'utf8' });
    assert.notEqual(invalid.status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('validates k6 rows against the resolved run', () => {
  const root = mkdtempSync(join(tmpdir(), 'k6-publish-test-'));
  try {
    const metadata = {
      bench: 'k6-http-hello-constant',
      ref: 'v2.10.3',
      sha: 'c'.repeat(40),
      short_sha: 'c'.repeat(12),
      run_id: '456',
      run_attempt: '2',
      timestamp: '2026-10-06T12:00:00Z',
      host_label: 'hetzner',
    };
    const row = {
      bench: 'k6', group: 'http-hello', param: 'constant-1000',
      sha: metadata.sha, short_sha: metadata.short_sha, ref: metadata.ref,
      run_id: metadata.run_id, run_attempt: metadata.run_attempt,
    };
    writeFileSync(join(root, 'metadata.json'), JSON.stringify(metadata));
    writeFileSync(join(root, 'results.jsonl'), JSON.stringify(row) + '\n');
    const env = {
      ...process.env,
      WASMCLOUD_BENCH_NAME: metadata.bench,
      WASMCLOUD_BENCH_ROW_NAME: 'k6',
      WASMCLOUD_BENCH_REF: metadata.ref,
      WASMCLOUD_BENCH_SHA: metadata.sha,
      GITHUB_RUN_ID: metadata.run_id,
      GITHUB_RUN_ATTEMPT: metadata.run_attempt,
    };
    const valid = spawnSync(process.execPath, [script, 'validate', root], { env, encoding: 'utf8' });
    assert.equal(valid.status, 0, valid.stderr);
    writeFileSync(join(root, 'results.jsonl'), JSON.stringify({ ...row, run_attempt: '1' }) + '\n');
    const stale = spawnSync(process.execPath, [script, 'validate', root], { env, encoding: 'utf8' });
    assert.notEqual(stale.status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('validates the host label of the artifact and every row', () => {
  const root = mkdtempSync(join(tmpdir(), 'bench-host-label-test-'));
  try {
    const metadata = {
      bench: 'http_invoke', ref: 'main', sha: 'd'.repeat(40), short_sha: 'd'.repeat(12),
      run_id: '789', run_attempt: '1', timestamp: '2026-11-05T12:00:00Z', host_label: 'aws-c7i',
    };
    const row = {
      bench: 'http_invoke', group: 'g', param: 'p',
      sha: metadata.sha, short_sha: metadata.short_sha, ref: metadata.ref,
      run_id: metadata.run_id, run_attempt: metadata.run_attempt, host_label: 'aws-c7i',
    };
    const env = {
      ...process.env,
      WASMCLOUD_BENCH_NAME: metadata.bench,
      WASMCLOUD_BENCH_REF: metadata.ref,
      WASMCLOUD_BENCH_SHA: metadata.sha,
      WASMCLOUD_BENCH_HOST_LABEL: 'aws-c7i',
      GITHUB_RUN_ID: metadata.run_id,
      GITHUB_RUN_ATTEMPT: metadata.run_attempt,
    };
    const validate = (meta, rows, environment = env) => {
      writeFileSync(join(root, 'metadata.json'), JSON.stringify(meta));
      writeFileSync(join(root, 'results.jsonl'), rows.map((r) => JSON.stringify(r) + '\n').join(''));
      return spawnSync(process.execPath, [script, 'validate', root], { env: environment, encoding: 'utf8' });
    };

    const valid = validate(metadata, [row]);
    assert.equal(valid.status, 0, valid.stderr);

    const wrongMeta = validate({ ...metadata, host_label: 'hetzner' }, [row]);
    assert.match(wrongMeta.stderr, /artifact host label does not match/);
    const noMeta = validate({ ...metadata, host_label: undefined }, [row]);
    assert.match(noMeta.stderr, /artifact host label does not match/);

    const wrongRow = validate(metadata, [{ ...row, host_label: 'hetzner' }]);
    assert.match(wrongRow.stderr, /artifact row does not match/);
    const { host_label: _, ...unlabeled } = row;
    const unlabeledRow = validate(metadata, [unlabeled]);
    assert.match(unlabeledRow.stderr, /artifact row does not match/);

    const { WASMCLOUD_BENCH_HOST_LABEL: __, ...hetznerEnv } = env;
    const hetzner = validate({ ...metadata, host_label: 'hetzner' }, [unlabeled], hetznerEnv);
    assert.equal(hetzner.status, 0, hetzner.stderr);

    const badLabel = validate(metadata, [row], { ...env, WASMCLOUD_BENCH_HOST_LABEL: 'AWS_C7I' });
    assert.match(badLabel.stderr, /invalid bench host label/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

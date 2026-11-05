import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import test from 'node:test';

import { dispatch, repository, repositoryId, requestTimeoutMs } from './release-fuzz.mjs';

const sha = 'a'.repeat(40);
const requestId = '123.2.12345678-1234-1234-1234-123456789abc';
const policy = JSON.parse(readFileSync(new URL('../../fuzz/private/policy.json', import.meta.url), 'utf8'));

function fixture() {
  const state = {
    repo: { private: true, id: repositoryId, full_name: repository, default_branch: 'main' },
    workflow: { id: 456, path: '.github/workflows/fuzz.yml', state: 'active' },
    requests: [], httpError: false,
  };
  const fetcher = async (url, options) => {
    assert.equal(options.headers.Authorization, 'Bearer app-token');
    state.requests.push({ url: String(url), method: options.method, body: options.body && JSON.parse(options.body) });
    if (state.httpError) return new Response('SECRET CRASH DETAILS', { status: 500 });
    switch (String(url)) {
      case `https://api.github.com/repos/${repository}`: return Response.json(state.repo);
      case `https://api.github.com/repos/${repository}/actions/workflows/fuzz.yml`: return Response.json(state.workflow);
      case `https://api.github.com/repos/${repository}/actions/workflows/fuzz.yml/dispatches`: return new Response(null, { status: 204 });
      default: assert.fail(`Unexpected API request: ${url}`);
    }
  };
  return { state, options: { sha, requestId, token: 'app-token', fetcher } };
}

test('dispatches the exact merged commit and returns without polling or reading results', async () => {
  const { state, options } = fixture();
  assert.deepEqual(await dispatch(options), { sha, requestId });
  assert.equal(state.requests.length, 3);
  assert.deepEqual(state.requests.at(-1).body, { ref: 'main', inputs: { sha, request_id: requestId } });
  assert.equal(state.requests.at(-1).method, 'POST');
});

for (const [name, mutate, message] of [
  ['public repository', (s) => { s.repo.private = false; }, /configured private repository/],
  ['replacement repository', (s) => { s.repo.id++; }, /configured private repository/],
  ['renamed repository', (s) => { s.repo.full_name = 'other/repository'; }, /configured private repository/],
  ['another default branch', (s) => { s.repo.default_branch = 'other'; }, /configured private repository/],
  ['disabled workflow', (s) => { s.workflow.state = 'disabled_manually'; }, /unavailable/],
  ['different workflow', (s) => { s.workflow.path = '.github/workflows/other.yml'; }, /unavailable/],
]) {
  test(`rejects dispatch to ${name}`, async () => {
    const { state, options } = fixture();
    mutate(state);
    await assert.rejects(dispatch(options), message);
    assert.ok(!state.requests.some((r) => r.method === 'POST'));
  });
}

test('rejects unresolved source refs, invalid request IDs and missing tokens before any request', async () => {
  for (const invalid of [{ sha: 'main' }, { sha: sha.slice(1) }, { requestId: 'old' }, { token: '' }, { token: undefined }]) {
    const { state, options } = fixture();
    await assert.rejects(dispatch({ ...options, ...invalid }));
    assert.equal(state.requests.length, 0);
  }
});

test('keeps private API failure bodies out of public errors', async () => {
  const { state, options } = fixture();
  state.httpError = true;
  await assert.rejects(dispatch(options), (error) => {
    assert.match(error.message, /HTTP 500/);
    assert.doesNotMatch(error.message, /SECRET/);
    return true;
  });
});

const train = readFileSync(new URL('../workflows/release-train.yml', import.meta.url), 'utf8');

test('mints the private token with octo-sts and fits every request in the step timeout', () => {
  const step = train.match(/      - name: Get a private fuzzing token from octo-sts\n([\s\S]*?)(?=\n      - name:)/)[1];
  assert.match(step, /uses: octo-sts\/action@[0-9a-f]{40} /);
  assert.match(step, new RegExp(`scope: ${repository}\\n`));
  assert.match(step, /identity: release-fuzz\n/);
  assert.match(step, /continue-on-error: true/);
  const dispatchStep = train.match(/      - name: Request private parser fuzzing\n([\s\S]*?)(?=\n      - name:)/)[1];
  assert.match(dispatchStep, /FUZZ_TOKEN: \$\{\{ steps\.fuzz-token\.outputs\.token \}\}/);
  const minutes = Number(dispatchStep.match(/timeout-minutes: (\d+)/)[1]);
  assert.ok(3 * requestTimeoutMs < minutes * 60_000);
});
const mergeStep = train.match(/      - name: Wait for PR auto-merge to complete\n([\s\S]*?)(?=\n      - name:)/)[1];
const mergeScript = mergeStep.split('        run: |\n')[1].split('\n').map((line) => line.slice(10)).join('\n');

for (const resolution of ['valid', 'missing', 'error']) {
  test(`release train completes after merge with ${resolution} fuzz SHA resolution`, () => {
    const root = mkdtempSync(join(tmpdir(), 'release-fuzz-merge-'));
    try {
      const gh = join(root, 'gh');
      writeFileSync(gh, '#!/bin/sh\ncase "$*" in\n  *mergeCommit*) ' +
        (resolution === 'valid' ? `echo '${sha}'` : resolution === 'missing' ? 'echo null' : 'exit 1') +
        '\n    ;;\n  *) echo MERGED;;\nesac\n');
      chmodSync(gh, 0o755);
      const output = join(root, 'output');
      writeFileSync(output, '');
      const result = spawnSync('bash', ['-c', mergeScript], { encoding: 'utf8',
        env: { ...process.env, PATH: `${root}${delimiter}${process.env.PATH}`, GITHUB_ENV: join(root, 'env'),
          GITHUB_OUTPUT: output, PR_URL: 'https://github.com/wasmCloud/wasmCloud/pull/123' } });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(readFileSync(output, 'utf8'), resolution === 'valid' ? `sha=${sha}\n` : '');
      if (resolution !== 'valid') assert.match(result.stdout, /::warning::/);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
}

test('dispatch failures and private campaign results cannot block promotion', () => {
  const step = train.match(/      - name: Request private parser fuzzing\n([\s\S]*?)(?=\n      - name:)/)[1];
  assert.match(step, /continue-on-error: true/);
  assert.match(step, /timeout-minutes: 2/);
  assert.match(step, /RELEASE_SHA: \$\{\{ steps\.merge\.outputs\.sha \}\}/);
  const promote = readFileSync(new URL('../workflows/release-tag.yml', import.meta.url), 'utf8')
    .match(/\n  promote:\n([\s\S]*?)(?=\n    runs-on:)/)[1];
  assert.match(promote, /needs: gate\n/);
  assert.doesNotMatch(promote, /fuzz/);
});

test('private result writer rejects skipped targets and records successful provenance', () => {
  const directory = mkdtempSync(join(tmpdir(), 'fuzz-result-test-'));
  const script = new URL('../../fuzz/private/write-result.mjs', import.meta.url);
  const results = { resolve: { result: 'success', outputs: { sha, harness: sha, seconds: '300' } },
    go: { result: 'success' }, rust: { result: 'success' } };
  const env = { ...process.env, GITHUB_SHA: sha, GITHUB_RUN_ID: '789',
    GITHUB_RUN_ATTEMPT: '1', REQUEST_ID: requestId, RESULTS: JSON.stringify(results) };
  try {
    execFileSync(process.execPath, [script.pathname], { cwd: directory, env });
    const evidence = JSON.parse(readFileSync(join(directory, 'result.json'), 'utf8'));
    assert.equal(evidence.source_sha, sha);
    assert.equal(evidence.request_id, requestId);
    assert.deepEqual(evidence.policy, policy);
    results.rust.result = 'skipped';
    assert.throws(() => execFileSync(process.execPath, [script.pathname], {
      cwd: directory, env: { ...env, RESULTS: JSON.stringify(results) }, stdio: 'pipe',
    }));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

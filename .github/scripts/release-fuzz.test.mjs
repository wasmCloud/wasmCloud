import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { decodeEvidence, federatedToken, gate, policy, repository, repositoryId } from './release-fuzz.mjs';

const sha = 'a'.repeat(40);
const workflowSha = 'b'.repeat(40);
const requestId = '123.2.12345678-1234-1234-1234-123456789abc';

function fixture() {
  const state = {
    repo: { private: true, id: repositoryId, full_name: repository, default_branch: 'main' },
    workflow: { id: 456, path: '.github/workflows/fuzz.yml', state: 'active' },
    run: {
      id: 789, workflow_id: 456, head_sha: workflowSha, head_branch: 'main',
      event: 'workflow_dispatch', run_attempt: 1, display_title: `release-fuzz ${sha} ${requestId}`,
      repository: { id: repositoryId }, status: 'completed', conclusion: 'success',
    },
    artifacts: [{ id: 987, name: 'fuzz-result-1', expired: false }],
    evidence: {
      policy: structuredClone(policy), source_sha: sha, harness_sha: 'c'.repeat(40),
      workflow_sha: workflowSha, run_id: '789', run_attempt: '1', request_id: requestId, seconds: 300,
    },
    requests: [], lookup: true, extraRuns: [], httpError: false,
  };
  let time = Date.parse('2026-11-04T12:00:00Z');
  const fetcher = async (url, options) => {
    const parsed = new URL(url);
    assert.equal(parsed.origin, 'https://api.github.com');
    assert.equal(options.headers.Authorization, 'Bearer app-token');
    const endpoint = parsed.pathname.slice(`/repos/${repository}`.length).replace(/^\//, '');
    state.requests.push({ endpoint, method: options.method || 'GET', body: options.body && JSON.parse(options.body) });
    if (state.httpError) return new Response('SECRET CRASH DETAILS', { status: 500 });
    let value;
    switch (endpoint) {
      case '': value = state.repo; break;
      case 'branches/main': value = { commit: { sha: workflowSha } }; break;
      case 'actions/workflows/fuzz.yml': value = state.workflow; break;
      case 'actions/workflows/fuzz.yml/dispatches': return new Response(null, { status: 204 });
      case 'actions/workflows/456/runs': {
        assert.equal(parsed.searchParams.get('event'), 'workflow_dispatch');
        assert.equal(parsed.searchParams.get('branch'), 'main');
        const runs = [...(state.lookup ? [state.run] : []), ...state.extraRuns];
        value = { total_count: runs.length, workflow_runs: runs };
        break;
      }
      case 'actions/runs/789': value = state.run; break;
      case 'actions/runs/789/artifacts': value = { total_count: state.artifacts.length, artifacts: state.artifacts }; break;
      case 'actions/artifacts/987/zip': return new Response(JSON.stringify(state.evidence));
      case 'actions/runs/789/cancel': return new Response(null, { status: 202 });
      default: assert.fail(`Unexpected API endpoint: ${endpoint}`);
    }
    return new Response(JSON.stringify(value));
  };
  const options = {
    sha, requestId, token: async () => 'app-token', fetcher,
    decode: (bytes) => JSON.parse(bytes.toString()), now: () => time,
    sleep: async (ms) => { time += ms; },
  };
  return { state, options };
}

test('dispatches the exact candidate and requires complete private evidence', async () => {
  const { state, options } = fixture();
  assert.deepEqual(await gate(options), {
    sha, runId: 789, workflowSha, harnessSha: 'c'.repeat(40), policyVersion: 1,
  });
  assert.deepEqual(state.requests.find((r) => r.method === 'POST').body,
    { ref: 'main', inputs: { sha, request_id: requestId } });
  assert.equal(policy.go.length, 11);
  assert.equal(policy.rust.length, 7);
  assert.ok(!state.requests.some((r) => /\/logs|\/jobs|\/caches/.test(r.endpoint)));
});

for (const [name, mutate, message] of [
  ['public repository', (s) => { s.repo.private = false; }, /configured private repository/],
  ['replacement repository', (s) => { s.repo.id++; }, /configured private repository/],
  ['disabled workflow', (s) => { s.workflow.state = 'disabled_manually'; }, /unavailable/],
  ['different workflow revision', (s) => { s.run.head_sha = 'd'.repeat(40); }, /dispatched workflow/],
  ['different workflow', (s) => { s.run.workflow_id++; }, /dispatched workflow/],
  ['workflow rerun', (s) => { s.run.run_attempt = 2; }, /dispatched workflow/],
  ['duplicate dispatches', (s) => { s.extraRuns = [{ ...s.run, id: 790 }]; }, /ambiguous runs/],
  ['failed run', (s) => { s.run.conclusion = 'failure'; }, /did not pass/],
  ['cancelled run', (s) => { s.run.conclusion = 'cancelled'; }, /did not pass/],
  ['skipped run', (s) => { s.run.conclusion = 'skipped'; }, /did not pass/],
  ['missing evidence', (s) => { s.artifacts = []; }, /missing or ambiguous/],
  ['expired evidence', (s) => { s.artifacts[0].expired = true; }, /missing or ambiguous/],
  ['another candidate', (s) => { s.evidence.source_sha = 'd'.repeat(40); }, /does not match/],
  ['another request', (s) => { s.evidence.request_id = 'old'; }, /does not match/],
  ['another result run', (s) => { s.evidence.run_id = '788'; }, /does not match/],
  ['shorter duration', (s) => { s.evidence.seconds = 1; }, /does not match/],
  ['missing Go target', (s) => { s.evidence.policy.go.pop(); }, /does not match/],
  ['missing Rust target', (s) => { s.evidence.policy.rust.pop(); }, /does not match/],
]) {
  test(`blocks release for ${name}`, async () => {
    const { state, options } = fixture();
    mutate(state);
    await assert.rejects(gate(options), message);
  });
}

test('does not accept a stale success from another request', async () => {
  const { state, options } = fixture();
  state.run.display_title = `release-fuzz ${sha} old-request`;
  await assert.rejects(gate(options), /did not start/);
  assert.ok(!state.requests.some((r) => r.endpoint.endsWith('/artifacts')));
});

test('times out and cancels a private run without opening the gate', async () => {
  const { state, options } = fixture();
  state.run.status = 'in_progress';
  await assert.rejects(gate({ ...options, timeoutMs: 1000 }), /Timed out/);
  assert.ok(state.requests.some((r) => r.endpoint === 'actions/runs/789/cancel'));
});

test('keeps private API failure bodies out of public errors', async () => {
  const { state, options } = fixture();
  state.httpError = true;
  await assert.rejects(gate(options), (error) => {
    assert.match(error.message, /HTTP 500/);
    assert.doesNotMatch(error.message, /SECRET/);
    return true;
  });
});

test('refreshes and revokes app tokens across builds longer than one hour', async () => {
  let time = 0;
  let exchanges = 0;
  const revoked = [];
  const token = federatedToken({ ACTIONS_ID_TOKEN_REQUEST_URL: 'https://oidc.example/token?job=1',
    ACTIONS_ID_TOKEN_REQUEST_TOKEN: 'request-token' }, async (url, options) => {
    const parsed = new URL(url);
    if (parsed.hostname === 'oidc.example') {
      assert.equal(parsed.searchParams.get('audience'), 'octo-sts.dev');
      return new Response(JSON.stringify({ value: 'oidc-token' }));
    }
    if (parsed.hostname === 'octo-sts.dev') {
      assert.equal(parsed.searchParams.get('scope'), repository);
      assert.equal(parsed.searchParams.get('identity'), 'release-fuzz');
      assert.equal(options.headers.Authorization, 'Bearer oidc-token');
      return new Response(JSON.stringify({ token: `app-${++exchanges}` }));
    }
    assert.equal(options.method, 'DELETE');
    revoked.push(options.headers.Authorization);
    return new Response(null, { status: 204 });
  }, () => time);
  assert.equal(await token(), 'app-1');
  time = 39 * 60_000;
  assert.equal(await token(), 'app-1');
  time = 61 * 60_000;
  assert.equal(await token(), 'app-2');
  await token.revoke();
  assert.deepEqual(revoked, ['Bearer app-1', 'Bearer app-2']);
});

test('decodes a real evidence archive and suppresses invalid private content', () => {
  const directory = mkdtempSync(join(tmpdir(), 'fuzz-evidence-test-'));
  try {
    writeFileSync(join(directory, 'result.json'), '{"source_sha":"candidate"}\n');
    execFileSync('zip', ['-q', 'evidence.zip', 'result.json'], { cwd: directory });
    assert.deepEqual(decodeEvidence(readFileSync(join(directory, 'evidence.zip'))), { source_sha: 'candidate' });
    assert.throws(() => decodeEvidence(Buffer.from('SECRET CRASH DETAILS')), /^Error: Private fuzzing evidence could not be decoded$/);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('private result writer rejects skipped targets and records successful provenance', () => {
  const directory = mkdtempSync(join(tmpdir(), 'fuzz-result-test-'));
  const script = new URL('../../fuzz/private/write-result.mjs', import.meta.url);
  const results = { resolve: { result: 'success', outputs: { sha, harness: workflowSha, seconds: '300' } },
    go: { result: 'success' }, rust: { result: 'success' } };
  const env = { ...process.env, GITHUB_SHA: workflowSha, GITHUB_RUN_ID: '789',
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

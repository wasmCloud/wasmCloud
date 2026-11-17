import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { checkSecurity, waitForSecurity } from './release-security.mjs';

const sha = 'a'.repeat(40);
function fixture() {
  const runs = Object.fromEntries(['audit', 'codeql'].map((workflow, index) => [workflow, [{
    id: index + 1, head_sha: sha, head_branch: 'main', path: `.github/workflows/${workflow}.yml`,
    event: 'push', status: 'completed', conclusion: 'success',
  }]]));
  const jobs = {
    1: ['cargo-audit', 'cargo-deny-licenses', 'osv-scanner', 'gate-audit'].map((name) => ({ name, conclusion: 'success' })),
    2: ['CodeQL (Rust)', 'CodeQL (Go - runtime-gateway)', 'CodeQL (Go - runtime-operator)'].map((name) => ({ name, conclusion: 'success' })),
  };
  const alerts = [];
  const requests = [];
  const options = { repo: 'wasmCloud/wasmCloud', sha, api: async (endpoint) => {
    requests.push(endpoint);
    if (endpoint.includes('/workflows/')) {
      const workflow = endpoint.split('/workflows/')[1].split('.yml')[0];
      return [{ workflow_runs: runs[workflow] }];
    }
    if (endpoint.includes('/jobs?')) return [{ jobs: jobs[endpoint.split('/runs/')[1].split('/')[0]] }];
    if (endpoint.includes('/code-scanning/alerts?')) return [alerts];
    throw new Error(`unexpected endpoint ${endpoint}`);
  } };
  return { runs, jobs, alerts, requests, options };
}

test('requires audit and every CodeQL language on the release SHA', async () => {
  const f = fixture();
  assert.deepEqual(await checkSecurity(f.options), { ready: true, pending: [] });
  assert.equal(f.requests.length, 5);
  assert.ok(f.requests[0].includes(`head_sha=${sha}`));
});

for (const workflow of ['audit', 'codeql']) {
  for (const state of ['missing', 'queued', 'in_progress', 'pull_request', 'different-sha', 'different-branch', 'different-workflow']) {
    test(`waits for ${workflow} when ${state}`, async () => {
      const f = fixture();
      const run = f.runs[workflow][0];
      if (state === 'missing') f.runs[workflow] = [];
      else if (state === 'pull_request') run.event = state;
      else if (state === 'different-sha') run.head_sha = 'b'.repeat(40);
      else if (state === 'different-branch') run.head_branch = 'other';
      else if (state === 'different-workflow') run.path = '.github/workflows/untrusted.yml';
      else run.status = state;
      const result = await checkSecurity(f.options);
      assert.equal(result.ready, false);
      assert.deepEqual(result.pending, [workflow]);
    });
  }
}

for (const conclusion of ['failure', 'cancelled', 'skipped', 'neutral', 'timed_out']) {
  test(`a newer ${conclusion} run cannot inherit an older success`, async () => {
    const f = fixture();
    f.runs.audit.push({ ...f.runs.audit[0], id: 3, conclusion });
    await assert.rejects(checkSecurity(f.options), /audit run 3 must succeed/);
  });
}

for (const [id, name] of [[1, 'cargo-audit'], [1, 'osv-scanner'], [1, 'cargo-deny-licenses'], [2, 'CodeQL (Rust)'], [2, 'CodeQL (Go - runtime-gateway)'], [2, 'CodeQL (Go - runtime-operator)']]) {
  test(`a successful run with skipped ${name} blocks promotion`, async () => {
    const f = fixture();
    f.jobs[id].find((job) => job.name === name).conclusion = 'skipped';
    await assert.rejects(checkSecurity(f.options), /must complete/);
  });
}

test('the newest pending scan waits despite an older successful run', async () => {
  const f = fixture();
  f.runs.audit.push({ ...f.runs.audit[0], id: 3, status: 'in_progress', conclusion: null });
  assert.equal((await checkSecurity(f.options)).ready, false);
});

test('API errors and invalid release SHAs fail closed', async () => {
  const f = fixture();
  await assert.rejects(checkSecurity({ ...f.options, sha: 'main' }), /full release SHA/);
  assert.equal(f.requests.length, 0);
  await assert.rejects(checkSecurity({ ...f.options, api: async () => { throw new Error('403 Forbidden'); } }), /403 Forbidden/);
});

for (const rule of [{ security_severity_level: 'high' }, { security_severity_level: 'critical' }, { severity: 'error' }]) {
  test(`open ${JSON.stringify(rule)} alerts block a successful scan`, async () => {
    const f = fixture();
    f.alerts.push({ number: 99, rule });
    await assert.rejects(checkSecurity(f.options), /open blocking CodeQL alerts.*99/);
  });
}

test('medium and low alerts follow the existing release policy', async () => {
  const f = fixture();
  f.alerts.push({ number: 1, rule: { severity: 'warning', security_severity_level: 'medium' } });
  assert.equal((await checkSecurity(f.options)).ready, true);
});

test('missing scans time out instead of allowing promotion', async () => {
  const f = fixture();
  f.runs.codeql = [];
  let elapsed = 0;
  await assert.rejects(waitForSecurity(f.options, { now: () => elapsed, sleep: async (duration) => { elapsed += duration; }, timeoutMs: 35_000 }), /Timed out/);
  assert.equal(elapsed, 35_000);
});

test('pending scans may finish before the deadline', async () => {
  const f = fixture();
  f.runs.codeql[0].status = 'queued';
  let elapsed = 0;
  await waitForSecurity(f.options, { now: () => elapsed, sleep: async (duration) => {
    elapsed += duration;
    f.runs.codeql[0].status = 'completed';
  } });
  assert.equal(elapsed, 30_000);
});

test('automatic and manual promotion both gate security before registry writes', () => {
  const workflow = readFileSync(new URL('../workflows/release-tag.yml', import.meta.url), 'utf8');
  const step = workflow.match(/      - name: Require security scans for the release commit\n([\s\S]*?)(?=\n      - name:)/)[1];
  assert.match(step, /MERGE_SHA: \$\{\{ steps.ver.outputs.sha \}\}/);
  assert.doesNotMatch(step, /continue-on-error:|\n        if:/);
  assert.ok(workflow.indexOf('node .github/scripts/release-security.mjs') < workflow.indexOf('- name: Login to ghcr.io'));
  assert.match(workflow, /security-events: read/);
});


test('a newly failing scheduled advisory scan overrides an earlier success', async () => {
  const f = fixture();
  f.runs.audit.push({ ...f.runs.audit[0], id: 3, event: 'schedule', conclusion: 'failure' });
  await assert.rejects(checkSecurity(f.options), /audit run 3 must succeed/);
});

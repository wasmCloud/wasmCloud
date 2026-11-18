import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { checkSecurity, waitForSecurity } from './release-security.mjs';

const sha = 'a'.repeat(40);
const started = '2026-11-17T10:00:00Z';
const completed = '2026-11-17T10:30:00Z';
function fixture() {
  const runs = Object.fromEntries(['audit', 'codeql'].map((workflow, index) => [workflow, [{
    id: index + 1, head_sha: sha, head_branch: 'main', path: `.github/workflows/${workflow}.yml`,
    event: 'push', status: 'completed', conclusion: 'success',
    created_at: started, run_started_at: started, updated_at: completed,
  }]]));
  const jobs = {
    1: ['cargo-audit', 'cargo-deny-licenses', 'osv-scanner', 'gate-audit'].map((name) => ({ name, run_attempt: 1, conclusion: 'success' })),
    2: ['CodeQL (Rust)', 'CodeQL (Go - runtime-gateway)', 'CodeQL (Go - runtime-operator)'].map((name) => ({ name, run_attempt: 1, conclusion: 'success', started_at: started, completed_at: completed })),
  };
  const analyses = ['rust', 'go-runtime-gateway', 'go-runtime-operator'].map((language, i) => ({
    id: 10 + i, commit_sha: sha, ref: 'refs/heads/main', tool: { name: 'CodeQL' },
    category: `/language:${language}`, created_at: '2026-11-17T10:20:00Z', error: '',
    analysis_key: `.github/workflows/codeql.yml:analyze-${i === 0 ? 'rust' : 'go'}`,
  }));
  const sarif = Object.fromEntries(analyses.map(({ id }) => [id, { runs: [{ results: [] }] }]));
  const releaseAlerts = {};
  const alerts = [];
  const requests = [];
  const options = { repo: 'wasmCloud/wasmCloud', sha, api: async (endpoint, format) => {
    requests.push(endpoint);
    if (endpoint.includes('/workflows/')) {
      const workflow = endpoint.split('/workflows/')[1].split('.yml')[0];
      return [{ workflow_runs: runs[workflow] }];
    }
    if (endpoint.includes('/jobs?')) {
      const all = jobs[endpoint.split('/runs/')[1].split('/')[0]];
      const latest = Math.max(...all.map((job) => job.run_attempt));
      return [{ jobs: endpoint.includes('filter=all') ? all : all.filter((job) => job.run_attempt === latest) }];
    }
    if (endpoint.includes('/code-scanning/analyses?')) return [analyses];
    if (endpoint.includes('/code-scanning/analyses/')) {
      assert.equal(format.accept, 'application/sarif+json');
      return [sarif[endpoint.split('/').at(-1)]];
    }
    if (endpoint.includes('/code-scanning/alerts/')) return [releaseAlerts[endpoint.split('/').at(-1)]];
    if (endpoint.includes('/code-scanning/alerts?')) return [alerts];
    throw new Error(`unexpected endpoint ${endpoint}`);
  } };
  return { runs, jobs, analyses, sarif, releaseAlerts, alerts, requests, options };
}

test('requires audit and every CodeQL language on the release SHA', async () => {
  const f = fixture();
  assert.deepEqual(await checkSecurity(f.options), { ready: true, pending: [] });
  assert.equal(f.requests.length, 9);
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

for (const [workflow, id, name, category] of [
  ['audit', 1, 'cargo-audit'],
  ['codeql', 2, 'CodeQL (Rust)', '/language:rust'],
  ['codeql', 2, 'CodeQL (Go - runtime-gateway)', '/language:go-runtime-gateway'],
  ['codeql', 2, 'CodeQL (Go - runtime-operator)', '/language:go-runtime-operator'],
]) {
  test(`a successful selective ${name} retry preserves the other successful jobs`, async () => {
    const f = fixture();
    f.runs[workflow][0].run_attempt = 2;
    const original = f.jobs[id].find((job) => job.name === name);
    original.conclusion = 'failure';
    f.jobs[id].push({ ...original, run_attempt: 2, conclusion: 'success',
      started_at: '2026-11-17T11:00:00Z', completed_at: '2026-11-17T11:30:00Z' });
    if (category) {
      const analysis = f.analyses.find((analysis) => analysis.category === category);
      // Failed attempts can still upload findings. Only the retried job's
      // analysis should be checked; untouched jobs retain their original times.
      f.sarif[analysis.id].runs[0].results = [{ properties: { 'github/alertNumber': 99 } }];
      f.releaseAlerts[99] = { state: 'fixed', rule: { severity: 'error' } };
      f.analyses.push({ ...analysis, id: 20, created_at: '2026-11-17T11:20:00Z' });
      f.sarif[20] = { runs: [{ results: [] }] };
    }
    assert.equal((await checkSecurity(f.options)).ready, true);
    assert.ok(f.requests.some((request) => request.includes(`/runs/${id}/jobs?filter=all`)));
  });
}

for (const [id, name] of [[1, 'cargo-audit'], [2, 'CodeQL (Rust)']]) {
  for (const conclusion of ['failure', 'skipped', null]) {
    for (const reversed of [false, true]) {
      test(`latest ${name} ${conclusion} cannot inherit an earlier success (reversed=${reversed})`, async () => {
        const f = fixture();
        const original = f.jobs[id].find((job) => job.name === name);
        f.jobs[id].push({ ...original, run_attempt: 2, conclusion });
        if (reversed) f.jobs[id].reverse();
        await assert.rejects(checkSecurity(f.options), /must complete/);
      });
    }
  }
}

test('selective retries are resolved across job-history pages', async () => {
  const f = fixture();
  const api = f.options.api;
  const original = f.jobs[1].find((job) => job.name === 'cargo-audit');
  original.conclusion = 'failure';
  f.options.api = async (endpoint, format) => {
    if (endpoint.includes('/runs/1/jobs?')) {
      assert.match(endpoint, /filter=all/);
      return [{ jobs: [{ ...original, run_attempt: 2, conclusion: 'success' }] }, { jobs: f.jobs[1] }];
    }
    return api(endpoint, format);
  };
  assert.equal((await checkSecurity(f.options)).ready, true);
});

for (const attempt of [undefined, 0]) {
  test(`job history with an invalid attempt (${attempt}) fails closed`, async () => {
    const f = fixture();
    f.jobs[1][0].run_attempt = attempt;
    await assert.rejects(checkSecurity(f.options), /missing or invalid attempt/);
  });
}

test('a retried CodeQL job cannot use its earlier clean analysis', async () => {
  const f = fixture();
  f.jobs[2].push({ ...f.jobs[2][0], run_attempt: 2,
    started_at: '2026-11-17T11:00:00Z', completed_at: '2026-11-17T11:30:00Z' });
  f.analyses.push({ ...f.analyses[0], id: 20, created_at: '2026-11-17T11:20:00Z' });
  f.sarif[20] = { runs: [{ results: [{ properties: { 'github/alertNumber': 99 } }] }] };
  f.releaseAlerts[99] = { state: 'fixed', rule: { severity: 'error' } };
  await assert.rejects(checkSecurity(f.options), /blocking CodeQL alert 99/);
});

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
  assert.ok(workflow.indexOf('node "${RUNNER_TEMP}/release-security.mjs"') < workflow.indexOf('- name: Login to ghcr.io'));
  assert.match(workflow, /security-events: read/);
});


test('a newly failing scheduled advisory scan overrides an earlier success', async () => {
  const f = fixture();
  f.runs.audit.push({ ...f.runs.audit[0], id: 3, event: 'schedule', conclusion: 'failure' });
  await assert.rejects(checkSecurity(f.options), /audit run 3 must succeed/);
});

for (const workflow of ['audit', 'codeql']) {
  for (const state of ['failure', 'in_progress', 'success']) {
    test(`a later retry of an older ${workflow} run takes precedence when ${state}`, async () => {
      const f = fixture();
      const older = f.runs[workflow][0];
      const newer = { ...older, id: older.id + 100, event: 'schedule',
        updated_at: '2026-11-17T11:00:00Z' };
      f.runs[workflow].push(newer);
      older.run_attempt = 2;
      older.updated_at = '2026-11-17T12:00:00Z';
      older.status = state === 'in_progress' ? state : 'completed';
      older.conclusion = state === 'in_progress' ? null : state;
      if (state === 'failure') await assert.rejects(checkSecurity(f.options), /must succeed/);
      else if (state === 'in_progress') assert.equal((await checkSecurity(f.options)).ready, false);
      else {
        assert.equal((await checkSecurity(f.options)).ready, true);
        assert.ok(f.requests.some((request) => request.includes(`/runs/${older.id}/jobs?`)));
      }
    });
  }
}

for (const language of ['rust', 'go-runtime-gateway', 'go-runtime-operator']) {
  for (const rule of [{ security_severity_level: 'high' }, { security_severity_level: 'critical' }, { severity: 'error' }]) {
    test(`${language} release findings still block after a fix on main: ${JSON.stringify(rule)}`, async () => {
      const f = fixture();
      const analysis = f.analyses.find((analysis) => analysis.category === `/language:${language}`);
      f.sarif[analysis.id].runs[0].results.push({ properties: { 'github/alertNumber': 99 } });
      f.releaseAlerts[99] = { number: 99, state: 'fixed', rule };
      assert.equal(f.alerts.length, 0); // main has already fixed the release's finding.
      await assert.rejects(checkSecurity(f.options), /blocking CodeQL alert 99 in release/);
    });
  }
}

test('explicitly dismissed and nonblocking release findings follow the alert policy', async () => {
  const f = fixture();
  f.sarif[10].runs[0].results = [1, 2].map((number) => ({ properties: { 'github/alertNumber': number } }));
  f.releaseAlerts[1] = { state: 'dismissed', rule: { severity: 'error', security_severity_level: 'critical' } };
  f.releaseAlerts[2] = { state: 'open', rule: { severity: 'warning', security_severity_level: 'medium' } };
  assert.equal((await checkSecurity(f.options)).ready, true);
});

for (const state of ['missing', 'wrong-sha', 'wrong-ref', 'wrong-tool', 'wrong-workflow', 'wrong-category', 'earlier-attempt', 'later-run']) {
  test(`waits for the release's analysis when ${state}`, async () => {
    const f = fixture();
    const analysis = f.analyses[0];
    if (state === 'missing') f.analyses.shift();
    else if (state === 'wrong-sha') analysis.commit_sha = 'b'.repeat(40);
    else if (state === 'wrong-ref') analysis.ref = 'refs/pull/123/merge';
    else if (state === 'wrong-tool') analysis.tool.name = 'other';
    else if (state === 'wrong-workflow') analysis.analysis_key = '.github/workflows/other.yml:analyze-rust';
    else if (state === 'wrong-category') analysis.category = '/language:other';
    else analysis.created_at = state === 'earlier-attempt' ? '2026-11-17T09:00:00Z' : '2026-11-17T11:00:00Z';
    assert.deepEqual(await checkSecurity(f.options), { ready: false, pending: ['CodeQL analysis /language:rust'] });
  });
}

test('missing analysis results time out and may become available before the deadline', async () => {
  const f = fixture();
  const analysis = f.analyses.shift();
  let elapsed = 0;
  await assert.rejects(waitForSecurity(f.options, { now: () => elapsed,
    sleep: async (ms) => { elapsed += ms; }, timeoutMs: 35_000 }), /Timed out/);
  await waitForSecurity(f.options, { now: () => elapsed, sleep: async (ms) => {
    elapsed += ms;
    f.analyses.push(analysis);
  } });
  assert.equal(elapsed, 65_000);
});

test('an older clean analysis cannot mask a newer finding for the same release', async () => {
  const f = fixture();
  f.analyses.push({ ...f.analyses[0], id: 20, created_at: '2026-11-17T10:25:00Z' });
  f.sarif[20] = { runs: [{ results: [{ properties: { 'github/alertNumber': 99 } }] }] };
  f.releaseAlerts[99] = { state: 'fixed', rule: { severity: 'error' } };
  await assert.rejects(checkSecurity(f.options), /blocking CodeQL alert 99/);
});

test('analysis lookup paginates past newer commits without reading older history', async () => {
  const f = fixture();
  const api = f.options.api;
  const pages = [];
  f.options.api = async (endpoint, format) => {
    if (!endpoint.includes('/analyses?')) return api(endpoint, format);
    assert.equal(format.paginate, false);
    const page = Number(new URL(`https://api.github.com/${endpoint}`).searchParams.get('page'));
    pages.push(page);
    if (page === 1) return [Array.from({ length: 100 }, (_, i) => ({ ...f.analyses[0],
      id: 1000 + i, commit_sha: 'b'.repeat(40), created_at: '2026-11-17T11:00:00Z' }))];
    if (page === 2) return [[...f.analyses, ...Array.from({ length: 97 }, () => ({
      ...f.analyses[0], created_at: '2026-11-17T09:00:00Z',
    }))]];
    throw new Error('read older, ineligible scan history');
  };
  assert.equal((await checkSecurity(f.options)).ready, true);
  assert.deepEqual(pages, [1, 2]);
});

for (const state of ['analysis-error', 'missing-results', 'missing-runs', 'missing-alert-number', 'invalid-alert', 'API-error']) {
  test(`release analysis fails closed on ${state}`, async () => {
    const f = fixture();
    if (state === 'analysis-error') f.analyses[0].error = 'failed to process';
    else if (state === 'missing-results') delete f.sarif[10].runs[0].results;
    else if (state === 'missing-runs') f.sarif[10].runs = [];
    else {
      f.sarif[10].runs[0].results = [{ properties: state === 'missing-alert-number' ? {} : { 'github/alertNumber': 99 } }];
      f.releaseAlerts[99] = {};
      if (state === 'API-error') {
        const api = f.options.api;
        f.options.api = async (endpoint, format) => {
          if (endpoint.includes('/alerts/')) throw new Error('403 Forbidden');
          return api(endpoint, format);
        };
      }
    }
    await assert.rejects(checkSecurity(f.options));
  });
}

test('manual recovery runs the workflow revision gate while the release checkout lacks it', () => {
  const workflow = readFileSync(new URL('../workflows/release-tag.yml', import.meta.url), 'utf8');
  const step = workflow.match(/      - name: Require security scans for the release commit\n([\s\S]*?)(?=\n      - name:)/)[1];
  assert.match(step, /WORKFLOW_SHA: \$\{\{ github.workflow_sha \}\}/);
  const command = step.split('        run: |\n')[1].split('\n').map((line) => line.slice(10)).join('\n');
  const dir = mkdtempSync(join(tmpdir(), 'release-security-recovery-'));
  const git = (...args) => execFileSync('git', args, { cwd: dir, encoding: 'utf8' }).trim();
  try {
    git('init', '--quiet');
    git('config', 'user.name', 'Test');
    git('config', 'user.email', 'test@example.com');
    git('-c', 'commit.gpgsign=false', 'commit', '--quiet', '--allow-empty', '-m', 'old release');
    const release = git('rev-parse', 'HEAD');
    mkdirSync(join(dir, '.github/scripts'), { recursive: true });
    writeFileSync(join(dir, '.github/scripts/release-security.mjs'),
      `import assert from 'node:assert/strict'; assert.equal(process.env.MERGE_SHA, '${release}'); console.log('workflow revision gate ran');`);
    git('add', '.github');
    git('-c', 'commit.gpgsign=false', 'commit', '--quiet', '-m', 'introduce gate');
    const revision = git('rev-parse', 'HEAD');
    git('checkout', '--quiet', '--detach', release);
    const output = execFileSync('bash', ['-e', '-o', 'pipefail', '-c', command], { cwd: dir, encoding: 'utf8', env: {
      ...process.env, WORKFLOW_SHA: revision, MERGE_SHA: release, RUNNER_TEMP: dir, GITHUB_ENV: join(dir, 'github-env'),
    } });
    assert.match(output, /workflow revision gate ran/);
    assert.equal(git('rev-parse', 'HEAD'), release);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

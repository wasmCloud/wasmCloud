// Require successful security scans on the commit being promoted, including
// manual recovery releases. A successful build alone does not satisfy this gate.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { setTimeout } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';

const required = {
  audit: ['cargo-audit', 'cargo-deny-licenses', 'osv-scanner', 'gate-audit'],
  codeql: ['CodeQL (Rust)', 'CodeQL (Go - runtime-gateway)', 'CodeQL (Go - runtime-operator)'],
};

const analyses = {
  '/language:rust': ['CodeQL (Rust)', 'analyze-rust'],
  '/language:go-runtime-gateway': ['CodeQL (Go - runtime-gateway)', 'analyze-go'],
  '/language:go-runtime-operator': ['CodeQL (Go - runtime-operator)', 'analyze-go'],
};
const blocksRelease = (rule) => rule?.severity === 'error' ||
  ['high', 'critical'].includes(rule?.security_severity_level);
// A retry keeps its run ID. Its updated time must outrank an intervening run.
const runTime = (run) => {
  const time = Math.max(...[run.created_at, run.run_started_at, run.updated_at].filter(Boolean).map(Date.parse));
  assert.ok(Number.isFinite(time), 'missing or invalid security run timestamp');
  return time;
};

async function checkReleaseAnalyses({ repo, sha, api }, jobs) {
  const earliestJob = Math.min(...jobs.filter((job) => required.codeql.includes(job.name)).map((job) => Date.parse(job.started_at)));
  assert.ok(Number.isFinite(earliestJob), 'missing CodeQL job start time');
  // The analyses API cannot filter by SHA. Stop paging once results predate
  // these jobs rather than downloading the entire repository scan history.
  const available = [];
  for (let page = 1; ; page++) {
    const [entries] = await api(`repos/${repo}/code-scanning/analyses?ref=refs%2Fheads%2Fmain&tool_name=CodeQL&sort=created&direction=desc&per_page=100&page=${page}`, { paginate: false });
    assert.ok(Array.isArray(entries), 'missing CodeQL analyses');
    available.push(...entries);
    if (entries.length < 100 || Date.parse(entries.at(-1).created_at) < earliestJob) break;
  }
  const pending = [];
  const alerts = new Map();
  for (const [category, [name, jobKey]] of Object.entries(analyses)) {
    const job = jobs.find((job) => job.name === name);
    assert.ok(Number.isFinite(Date.parse(job.started_at)) && Number.isFinite(Date.parse(job.completed_at)),
      `missing CodeQL job timestamps for ${name}`);
    // Match the exact commit and this job's execution window, including retries.
    const analysis = available.filter((analysis) => analysis.commit_sha === sha &&
      analysis.ref === 'refs/heads/main' && analysis.tool?.name === 'CodeQL' &&
      analysis.category === category && analysis.analysis_key === `.github/workflows/codeql.yml:${jobKey}` &&
      Date.parse(analysis.created_at) >= Date.parse(job.started_at) &&
      Date.parse(analysis.created_at) <= Date.parse(job.completed_at))
      .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at) || b.id - a.id)[0];
    if (!analysis) {
      pending.push(`CodeQL analysis ${category}`);
      continue;
    }
    assert.equal(analysis.error, '', `CodeQL analysis ${analysis.id} failed`);
    const [sarif] = await api(`repos/${repo}/code-scanning/analyses/${analysis.id}`, { accept: 'application/sarif+json' });
    assert.ok(Array.isArray(sarif.runs) && sarif.runs.length > 0, 'missing CodeQL SARIF runs');
    for (const run of sarif.runs) {
      assert.ok(Array.isArray(run.results), 'missing CodeQL SARIF results');
      for (const result of run.results) {
        const number = result.properties?.['github/alertNumber'];
        assert.ok(Number.isSafeInteger(number) && number > 0, 'missing CodeQL alert number in SARIF');
        if (!alerts.has(number)) {
          const [alert] = await api(`repos/${repo}/code-scanning/alerts/${number}`);
          assert.ok(alert.rule && ['open', 'fixed', 'dismissed'].includes(alert.state), 'invalid CodeQL alert');
          alerts.set(number, alert);
        }
        const alert = alerts.get(number);
        // A fix on newer main does not fix the pinned release. Explicit
        // dismissals still apply, as they do to the open-alert policy below.
        assert.ok(alert.state === 'dismissed' || !blocksRelease(alert.rule),
          `blocking CodeQL alert ${number} in release ${sha}`);
      }
    }
  }
  return pending;
}

export async function checkSecurity({ repo, sha, api }) {
  assert.match(sha, /^[0-9a-f]{40}$/, 'a full release SHA is required');
  const pending = [];
  let codeqlJobs;
  for (const [workflow, jobs] of Object.entries(required)) {
    const pages = await api(`repos/${repo}/actions/workflows/${workflow}.yml/runs?head_sha=${sha}&branch=main&per_page=100`);
    const run = pages.flatMap((page) => page.workflow_runs).filter((run) =>
      run.head_sha === sha && run.head_branch === 'main' &&
      run.path === `.github/workflows/${workflow}.yml` &&
      ['push', 'workflow_dispatch', 'schedule'].includes(run.event),
    ).sort((a, b) => runTime(b) - runTime(a) || b.id - a.id)[0];
    if (!run || run.status !== 'completed') {
      pending.push(workflow);
      continue;
    }
    assert.equal(run.conclusion, 'success', `${workflow} run ${run.id} must succeed for ${sha}`);
    // filter=latest excludes jobs left over from earlier, failed attempts.
    const jobPages = await api(`repos/${repo}/actions/runs/${run.id}/jobs?filter=latest&per_page=100`);
    const actual = jobPages.flatMap((page) => page.jobs);
    if (workflow === 'codeql') codeqlJobs = actual;
    for (const name of jobs) {
      const job = actual.find((job) => job.name === name);
      assert.equal(job?.conclusion, 'success', `${workflow} must complete ${name} successfully for ${sha}`);
    }
  }
  if (pending.length) return { ready: false, pending };
  pending.push(...await checkReleaseAnalyses({ repo, sha, api }, codeqlJobs));
  if (pending.length) return { ready: false, pending };
  // Scan execution succeeds even when it produces alerts. Honor the project's
  // existing high/critical and error thresholds, including existing alerts.
  const pages = await api(`repos/${repo}/code-scanning/alerts?ref=refs%2Fheads%2Fmain&state=open&tool_name=CodeQL&per_page=100`);
  const blocking = pages.flat().filter((alert) => blocksRelease(alert.rule));
  assert.equal(blocking.length, 0, `open blocking CodeQL alerts on main: ${blocking.map((alert) => alert.number).join(', ')}`);
  return { ready: true, pending: [] };
}

export async function waitForSecurity(options, { now = Date.now, sleep = setTimeout, timeoutMs = 30 * 60_000 } = {}) {
  const deadline = now() + timeoutMs;
  while (now() < deadline) {
    const result = await checkSecurity(options);
    if (result.ready) return;
    console.log(`Waiting for ${result.pending.join(', ')} on ${options.sha}`);
    await sleep(Math.min(30_000, Math.max(0, deadline - now())));
  }
  throw new Error(`Timed out waiting for security scans on ${options.sha}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    await waitForSecurity({ repo: process.env.GH_REPO, sha: process.env.MERGE_SHA,
      api: async (endpoint, { accept = 'application/vnd.github+json', paginate = true } = {}) => {
        const result = JSON.parse(execFileSync('gh', ['api', ...(paginate ? ['--paginate', '--slurp'] : []), '-H', `Accept: ${accept}`, endpoint],
          { encoding: 'utf8', maxBuffer: 32 * 1024 * 1024, timeout: 60_000 }));
        return paginate ? result : [result];
      },
    });
    console.log(`Security scans passed for ${process.env.MERGE_SHA}`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

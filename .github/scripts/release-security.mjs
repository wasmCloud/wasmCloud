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

export async function checkSecurity({ repo, sha, api }) {
  assert.match(sha, /^[0-9a-f]{40}$/, 'a full release SHA is required');
  const pending = [];
  for (const [workflow, jobs] of Object.entries(required)) {
    const pages = await api(`repos/${repo}/actions/workflows/${workflow}.yml/runs?head_sha=${sha}&branch=main&per_page=100`);
    const run = pages.flatMap((page) => page.workflow_runs).filter((run) =>
      run.head_sha === sha && run.head_branch === 'main' &&
      run.path === `.github/workflows/${workflow}.yml` &&
      ['push', 'workflow_dispatch', 'schedule'].includes(run.event),
    ).sort((a, b) => b.id - a.id)[0];
    if (!run || run.status !== 'completed') {
      pending.push(workflow);
      continue;
    }
    assert.equal(run.conclusion, 'success', `${workflow} run ${run.id} must succeed for ${sha}`);
    // filter=latest excludes jobs left over from earlier, failed attempts.
    const jobPages = await api(`repos/${repo}/actions/runs/${run.id}/jobs?filter=latest&per_page=100`);
    const actual = jobPages.flatMap((page) => page.jobs);
    for (const name of jobs) {
      const job = actual.find((job) => job.name === name);
      assert.equal(job?.conclusion, 'success', `${workflow} must complete ${name} successfully for ${sha}`);
    }
  }
  if (pending.length) return { ready: false, pending };
  // Scan execution succeeds even when it produces alerts. Honor the project's
  // existing high/critical and error thresholds, including existing alerts.
  const pages = await api(`repos/${repo}/code-scanning/alerts?ref=refs%2Fheads%2Fmain&state=open&tool_name=CodeQL&per_page=100`);
  const blocking = pages.flat().filter((alert) => alert.rule?.severity === 'error' ||
    ['high', 'critical'].includes(alert.rule?.security_severity_level));
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
      api: async (endpoint) => JSON.parse(execFileSync('gh', ['api', '--paginate', '--slurp', endpoint], { encoding: 'utf8' })),
    });
    console.log(`Security scans passed for ${process.env.MERGE_SHA}`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { appendFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { isDeepStrictEqual } from 'node:util';

export const repository = 'wasmCloud/security-fuzz';
export const repositoryId = 1445861135;
export const policy = JSON.parse(readFileSync(new URL('../../fuzz/private/policy.json', import.meta.url), 'utf8'));
const shaPattern = /^[0-9a-f]{40}$/;
const apiRoot = 'https://api.github.com';

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

async function request(fetcher, url, options = {}) {
  let response;
  try {
    response = await fetcher(url, { ...options, signal: AbortSignal.timeout(30_000) });
  } catch {
    throw new Error('Private fuzzing request could not be completed');
  }
  requireValue(response.ok, `Private fuzzing request failed (HTTP ${response.status})`);
  return response;
}

async function json(response) {
  try {
    return await response.json();
  } catch {
    throw new Error('Private fuzzing returned an invalid response');
  }
}

// Refresh the app token during long builds; no private response bodies reach public logs.
export function federatedToken(env, fetcher = fetch, now = Date.now) {
  let cached;
  let refreshAt = 0;
  const revoke = async () => {
    if (!cached) return;
    try {
      await fetcher(`${apiRoot}/installation/token`, {
        method: 'DELETE',
        headers: { Authorization: `Bearer ${cached}` },
        signal: AbortSignal.timeout(10_000),
      });
    } catch {
      // Tokens also expire at GitHub.
    }
    cached = undefined;
  };
  const token = async () => {
    if (cached && now() < refreshAt) return cached;
    requireValue(env.ACTIONS_ID_TOKEN_REQUEST_URL && env.ACTIONS_ID_TOKEN_REQUEST_TOKEN, 'OIDC is required for private fuzzing');
    const oidcUrl = new URL(env.ACTIONS_ID_TOKEN_REQUEST_URL);
    oidcUrl.searchParams.set('audience', 'octo-sts.dev');
    const oidc = await json(await request(fetcher, oidcUrl, {
      headers: { Authorization: `Bearer ${env.ACTIONS_ID_TOKEN_REQUEST_TOKEN}` },
    }));
    requireValue(typeof oidc.value === 'string' && oidc.value.length > 0, 'Missing workflow identity token');
    const exchange = new URL('https://octo-sts.dev/sts/exchange');
    exchange.searchParams.set('scope', repository);
    exchange.searchParams.set('identity', 'release-fuzz');
    const result = await json(await request(fetcher, exchange, {
      headers: { Authorization: `Bearer ${oidc.value}` },
    }));
    requireValue(typeof result.token === 'string' && result.token.length > 0, 'Missing private fuzzing app token');
    await revoke();
    cached = result.token;
    refreshAt = now() + 40 * 60_000;
    return cached;
  };
  token.revoke = revoke;
  return token;
}

export function decodeEvidence(archive) {
  const directory = mkdtempSync(join(tmpdir(), 'release-fuzz-'));
  try {
    const filename = join(directory, 'evidence.zip');
    writeFileSync(filename, archive);
    const output = execFileSync('unzip', ['-p', filename, 'result.json'], {
      encoding: 'utf8', maxBuffer: 64 * 1024, stdio: ['ignore', 'pipe', 'pipe'],
    });
    return JSON.parse(output);
  } catch {
    throw new Error('Private fuzzing evidence could not be decoded');
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

export function validateEvidence(result, { sha, workflowSha, runId, requestId }) {
  requireValue(
    result?.source_sha === sha && shaPattern.test(result.harness_sha) &&
    result.workflow_sha === workflowSha && result.run_id === String(runId) &&
    result.run_attempt === '1' && result.request_id === requestId &&
    result.seconds === policy.release_seconds && isDeepStrictEqual(result.policy, policy),
    'Private fuzzing evidence does not match this release request',
  );
}

export async function gate({ sha, requestId, token, fetcher = fetch, now = Date.now,
  sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
  decode = decodeEvidence, timeoutMs = 150 * 60_000 }) {
  requireValue(shaPattern.test(sha), 'A full release commit SHA is required');
  requireValue(/^\d+\.\d+\.[0-9a-f-]{36}$/.test(requestId), 'Invalid private fuzzing request ID');
  const started = now();
  const deadline = started + timeoutMs;
  const title = `release-fuzz ${sha} ${requestId}`;
  const api = async (endpoint, method = 'GET', body) => request(fetcher, `${apiRoot}/repos/${repository}${endpoint ? `/${endpoint}` : ''}`, {
    method,
    headers: {
      Accept: 'application/vnd.github+json',
      Authorization: `Bearer ${await token()}`,
      'X-GitHub-Api-Version': '2022-11-28',
      ...(body ? { 'Content-Type': 'application/json' } : {}),
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const repo = await json(await api(''));
  requireValue(repo.private === true && repo.id === repositoryId && repo.full_name === repository && repo.default_branch === 'main',
    'Fuzzing requires the configured private repository');
  const branch = await json(await api('branches/main'));
  const workflowSha = branch.commit?.sha;
  requireValue(shaPattern.test(workflowSha), 'Invalid private workflow revision');
  const workflow = await json(await api('actions/workflows/fuzz.yml'));
  requireValue(Number.isSafeInteger(workflow.id) && workflow.path === '.github/workflows/fuzz.yml' && workflow.state === 'active',
    'The private fuzzing workflow is unavailable');
  await api('actions/workflows/fuzz.yml/dispatches', 'POST', {
    ref: 'main', inputs: { sha, request_id: requestId },
  });

  let runId;
  while (now() < deadline) {
    if (!runId) {
      const query = new URLSearchParams({
        event: 'workflow_dispatch', branch: 'main', per_page: '100',
        created: `>=${new Date(started - 2000).toISOString()}`,
      });
      const runs = await json(await api(`actions/workflows/${workflow.id}/runs?${query}`));
      requireValue(Array.isArray(runs.workflow_runs) && runs.total_count <= 100, 'Private fuzzing run lookup is incomplete');
      const matches = runs.workflow_runs.filter((run) => run.display_title === title);
      requireValue(matches.length <= 1, 'Private fuzzing request has ambiguous runs');
      requireValue(matches.length === 0 || Number.isSafeInteger(matches[0].id), 'Invalid private fuzzing run ID');
      runId = matches[0]?.id;
      if (!runId && now() - started >= 5 * 60_000) throw new Error('Private fuzzing did not start');
    }
    if (runId) {
      const run = await json(await api(`actions/runs/${runId}`));
      requireValue(run.id === runId && run.workflow_id === workflow.id && run.head_sha === workflowSha &&
        run.head_branch === 'main' && run.event === 'workflow_dispatch' && run.run_attempt === 1 &&
        run.display_title === title && run.repository?.id === repositoryId,
      'Private fuzzing run does not match the dispatched workflow');
      if (run.status === 'completed') {
        requireValue(run.conclusion === 'success', 'Private fuzzing did not pass; inspect the private run');
        const artifacts = await json(await api(`actions/runs/${runId}/artifacts?per_page=100`));
        requireValue(Array.isArray(artifacts.artifacts) && artifacts.total_count <= 100, 'Private fuzzing evidence lookup is incomplete');
        const matches = artifacts.artifacts.filter((artifact) => artifact.name === 'fuzz-result-1' && !artifact.expired);
        requireValue(matches.length === 1 && Number.isSafeInteger(matches[0].id), 'Private fuzzing evidence is missing or ambiguous');
        const archive = await request(fetcher, `${apiRoot}/repos/${repository}/actions/artifacts/${matches[0].id}/zip`, {
          headers: { Authorization: `Bearer ${await token()}` }, redirect: 'follow',
        });
        const chunks = [];
        let size = 0;
        for await (const chunk of archive.body) {
          size += chunk.length;
          requireValue(size <= 1024 * 1024, 'Private fuzzing evidence exceeds the size limit');
          chunks.push(chunk);
        }
        let evidence;
        try { evidence = decode(Buffer.concat(chunks)); } catch { throw new Error('Private fuzzing evidence could not be decoded'); }
        validateEvidence(evidence, { sha, workflowSha, runId, requestId });
        return { sha, runId, workflowSha, harnessSha: evidence.harness_sha, policyVersion: policy.version };
      }
      requireValue(['queued', 'in_progress', 'waiting', 'pending', 'requested'].includes(run.status), 'Private fuzzing has an unexpected status');
    }
    await sleep(20_000);
  }
  if (runId) {
    try { await api(`actions/runs/${runId}/cancel`, 'POST'); } catch { /* The gate remains closed. */ }
  }
  throw new Error('Timed out waiting for private fuzzing');
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const token = federatedToken(process.env);
  try {
    const result = await gate({ sha: process.env.RELEASE_SHA,
      requestId: `${process.env.GITHUB_RUN_ID}.${process.env.GITHUB_RUN_ATTEMPT}.${randomUUID()}`, token });
    const summary = `Private parser fuzzing passed for \`${result.sha}\`: ${policy.go.length + policy.rust.length} targets, ${policy.release_seconds} seconds each.\n`;
    console.log(summary.trim());
    if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, summary);
    if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `sha=${result.sha}\nprivate_run_id=${result.runId}\n`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  } finally {
    await token.revoke();
  }
}

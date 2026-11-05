import { randomUUID } from 'node:crypto';
import { appendFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export const repository = 'wasmCloud/security-fuzz';
export const repositoryId = 1445861135;
const apiRoot = 'https://api.github.com';
// Three requests must finish within the step's two-minute timeout.
export const requestTimeoutMs = 20_000;

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

async function request(fetcher, url, options = {}) {
  let response;
  try {
    response = await fetcher(url, { ...options, signal: AbortSignal.timeout(requestTimeoutMs) });
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

export async function dispatch({ sha, requestId, token, fetcher = fetch }) {
  requireValue(/^[0-9a-f]{40}$/.test(sha), 'A full release commit SHA is required');
  requireValue(/^\d+\.\d+\.[0-9a-f-]{36}$/.test(requestId), 'Invalid private fuzzing request ID');
  requireValue(typeof token === 'string' && token.length > 0, 'Missing private fuzzing app token');
  const api = async (endpoint, method = 'GET', body) => request(fetcher, `${apiRoot}/repos/${repository}${endpoint ? `/${endpoint}` : ''}`, {
    method,
    headers: {
      Accept: 'application/vnd.github+json',
      Authorization: `Bearer ${token}`,
      'X-GitHub-Api-Version': '2022-11-28',
      ...(body ? { 'Content-Type': 'application/json' } : {}),
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const repo = await json(await api(''));
  requireValue(repo.private === true && repo.id === repositoryId && repo.full_name === repository && repo.default_branch === 'main',
    'Fuzzing requires the configured private repository');
  const workflow = await json(await api('actions/workflows/fuzz.yml'));
  requireValue(Number.isSafeInteger(workflow.id) && workflow.path === '.github/workflows/fuzz.yml' && workflow.state === 'active',
    'The private fuzzing workflow is unavailable');
  await api('actions/workflows/fuzz.yml/dispatches', 'POST', {
    ref: 'main', inputs: { sha, request_id: requestId },
  });
  return { sha, requestId };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const result = await dispatch({ sha: process.env.RELEASE_SHA, token: process.env.FUZZ_TOKEN,
      requestId: `${process.env.GITHUB_RUN_ID}.${process.env.GITHUB_RUN_ATTEMPT}.${randomUUID()}` });
    const summary = `Private parser fuzzing requested for \`${result.sha}\` (request \`${result.requestId}\`). Promotion does not wait for this campaign.\n`;
    console.log(summary.trim());
    if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, summary);
  } catch (error) {
    console.error(`::warning::${error.message}; release promotion continues`);
    process.exitCode = 1;
  }
}

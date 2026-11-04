import { randomUUID } from 'node:crypto';
import { appendFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export const repository = 'wasmCloud/security-fuzz';
export const repositoryId = 1445861135;
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

export function federatedToken(env, fetcher = fetch) {
  let cached;
  const token = async () => {
    if (cached) return cached;
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
    cached = result.token;
    return cached;
  };
  token.revoke = async () => {
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
  return token;
}

export async function dispatch({ sha, requestId, token, fetcher = fetch }) {
  requireValue(/^[0-9a-f]{40}$/.test(sha), 'A full release commit SHA is required');
  requireValue(/^\d+\.\d+\.[0-9a-f-]{36}$/.test(requestId), 'Invalid private fuzzing request ID');
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
  const workflow = await json(await api('actions/workflows/fuzz.yml'));
  requireValue(Number.isSafeInteger(workflow.id) && workflow.path === '.github/workflows/fuzz.yml' && workflow.state === 'active',
    'The private fuzzing workflow is unavailable');
  await api('actions/workflows/fuzz.yml/dispatches', 'POST', {
    ref: 'main', inputs: { sha, request_id: requestId },
  });
  return { sha, requestId };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const token = federatedToken(process.env);
  try {
    const result = await dispatch({ sha: process.env.RELEASE_SHA,
      requestId: `${process.env.GITHUB_RUN_ID}.${process.env.GITHUB_RUN_ATTEMPT}.${randomUUID()}`, token });
    const summary = `Private parser fuzzing requested for \`${result.sha}\` (request \`${result.requestId}\`). Promotion does not wait for this campaign.\n`;
    console.log(summary.trim());
    if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, summary);
  } catch (error) {
    console.error(`::warning::${error.message}; release promotion continues`);
    process.exitCode = 1;
  } finally {
    await token.revoke();
  }
}

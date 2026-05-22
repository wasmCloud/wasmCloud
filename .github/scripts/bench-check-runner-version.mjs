#!/usr/bin/env node
// Monthly check: is the actions/runner version pinned in
// scripts/bench/install-runner.sh behind the latest upstream release?
//
// If so, opens (or updates) a single tracking issue with the release
// notes inline, so a maintainer can scan the changelog without
// clicking through to the GitHub release page and decide whether the
// bump needs care (e.g. arg/format changes to config.sh, new system
// dependencies). Idempotent: re-runs on the existing issue rather than
// spawning duplicates.
//
// Auto-close behavior: when the pinned version catches up to the
// latest upstream, the tracking issue is closed with a one-liner
// comment. Maintainers can also close via `Closes #N` in the bumping
// PR; either path works.
//
// Invoked by .github/workflows/bench-host-checks.yml on a monthly cron.
//
// Reads:
//   GITHUB_TOKEN      — required; needs issues:write
//   GITHUB_REPOSITORY — owner/repo (set automatically by GH Actions)
//
// Exits 0 on every code path that isn't a hard error (network,
// malformed install-runner.sh) — the workflow should not fail just
// because there's a pending bump.

import { readFileSync } from 'node:fs';

const INSTALL_RUNNER = 'scripts/bench/install-runner.sh';
const ISSUE_MARKER = '[bench-host] actions/runner update available';

const token = required('GITHUB_TOKEN');
const repo = required('GITHUB_REPOSITORY');

const apiHeaders = {
  Authorization: `Bearer ${token}`,
  Accept: 'application/vnd.github+json',
  'X-GitHub-Api-Version': '2022-11-28',
  'User-Agent': 'wasmcloud-bench-host-version-check',
};

// ── 1. Pinned version from install-runner.sh ────────────────────────────
const installSh = readFileSync(INSTALL_RUNNER, 'utf8');
const pinnedMatch = installSh.match(/^RUNNER_VERSION="([^"]+)"/m);
if (!pinnedMatch) {
  console.error(`could not parse RUNNER_VERSION from ${INSTALL_RUNNER}`);
  process.exit(1);
}
const pinned = pinnedMatch[1];

// ── 2. Latest upstream release ──────────────────────────────────────────
const releaseRes = await fetch(
  'https://api.github.com/repos/actions/runner/releases/latest',
  { headers: apiHeaders },
);
if (!releaseRes.ok) {
  console.error(`fetching latest release: ${releaseRes.status} ${await releaseRes.text()}`);
  process.exit(1);
}
const release = await releaseRes.json();
const latest = release.tag_name.replace(/^v/, '');
const releaseUrl = release.html_url;
const releaseBody = release.body || '_(no release notes)_';
const releaseDate = release.published_at;

console.log(`pinned: v${pinned}; latest: v${latest}`);

// ── 3. Find existing tracking issue (open or recently closed) ──────────
const existing = await findExistingIssue();

// ── 4. Branch on whether we're behind ───────────────────────────────────
if (pinned === latest) {
  if (existing && existing.state === 'open') {
    await closeIssue(
      existing.number,
      `Pinned version is now \`v${pinned}\`, matching latest upstream. Auto-closing.`,
    );
    console.log(`closed #${existing.number} — pinned now matches latest`);
  } else {
    console.log('up to date — nothing to do');
  }
  process.exit(0);
}

// Behind upstream: open or update.
const body = composeBody({ pinned, latest, releaseUrl, releaseDate, releaseBody });

if (!existing || existing.state === 'closed') {
  const created = await createIssue(`${ISSUE_MARKER}: v${latest}`, body);
  console.log(`opened #${created.number} for v${latest}`);
  process.exit(0);
}

// Existing open issue. If it already tracks this exact latest version,
// nothing to add. Otherwise, comment with the newer info and retitle.
const trackedMatch = existing.title.match(/v([\d.]+)$/);
const tracked = trackedMatch?.[1];
if (tracked === latest) {
  console.log(`#${existing.number} already tracks v${latest}`);
  process.exit(0);
}

await commentOnIssue(
  existing.number,
  `Bumped tracked latest from \`v${tracked ?? '?'}\` → \`v${latest}\` on the monthly check.\n\n${body}`,
);
await patchIssueTitle(existing.number, `${ISSUE_MARKER}: v${latest}`);
console.log(`updated #${existing.number}: now tracking v${latest}`);

// ── helpers ─────────────────────────────────────────────────────────────

function required(name) {
  const v = process.env[name];
  if (!v) {
    console.error(`${name} is required`);
    process.exit(1);
  }
  return v;
}

async function findExistingIssue() {
  // We look at both open and closed states in case a previous bump's
  // tracking issue was just closed — we'd rather reopen the same issue
  // than spawn a new one for an immediate follow-up release.
  const q = `repo:${repo} is:issue in:title "${ISSUE_MARKER}"`;
  const url = `https://api.github.com/search/issues?q=${encodeURIComponent(q)}&sort=updated&order=desc&per_page=1`;
  const res = await fetch(url, { headers: apiHeaders });
  if (!res.ok) {
    console.error(`searching issues: ${res.status} ${await res.text()}`);
    process.exit(1);
  }
  const json = await res.json();
  return json.items[0];
}

async function createIssue(title, body) {
  const res = await fetch(`https://api.github.com/repos/${repo}/issues`, {
    method: 'POST',
    headers: { ...apiHeaders, 'Content-Type': 'application/json' },
    body: JSON.stringify({ title, body }),
  });
  if (!res.ok) {
    console.error(`creating issue: ${res.status} ${await res.text()}`);
    process.exit(1);
  }
  return res.json();
}

async function commentOnIssue(number, body) {
  const res = await fetch(`https://api.github.com/repos/${repo}/issues/${number}/comments`, {
    method: 'POST',
    headers: { ...apiHeaders, 'Content-Type': 'application/json' },
    body: JSON.stringify({ body }),
  });
  if (!res.ok) {
    console.error(`commenting on #${number}: ${res.status} ${await res.text()}`);
    process.exit(1);
  }
}

async function patchIssueTitle(number, title) {
  const res = await fetch(`https://api.github.com/repos/${repo}/issues/${number}`, {
    method: 'PATCH',
    headers: { ...apiHeaders, 'Content-Type': 'application/json' },
    body: JSON.stringify({ title }),
  });
  if (!res.ok) {
    console.error(`updating title on #${number}: ${res.status} ${await res.text()}`);
    process.exit(1);
  }
}

async function closeIssue(number, comment) {
  await commentOnIssue(number, comment);
  const res = await fetch(`https://api.github.com/repos/${repo}/issues/${number}`, {
    method: 'PATCH',
    headers: { ...apiHeaders, 'Content-Type': 'application/json' },
    body: JSON.stringify({ state: 'closed', state_reason: 'completed' }),
  });
  if (!res.ok) {
    console.error(`closing #${number}: ${res.status} ${await res.text()}`);
    process.exit(1);
  }
}

function composeBody({ pinned, latest, releaseUrl, releaseDate, releaseBody }) {
  return [
    `**Pinned** (in [\`${INSTALL_RUNNER}\`](../blob/main/${INSTALL_RUNNER})): \`v${pinned}\``,
    `**Latest upstream:** \`v${latest}\` — released ${releaseDate}`,
    `**Release notes:** ${releaseUrl}`,
    '',
    '<details><summary>upstream release body</summary>',
    '',
    releaseBody,
    '',
    '</details>',
    '',
    '---',
    '',
    '### To accept the bump',
    '',
    '1. Bump `RUNNER_VERSION` and `RUNNER_SHA256` in `scripts/bench/install-runner.sh`. The SHA-256 is on the release page under "Assets" — use the `actions-runner-linux-x64-<version>.tar.gz` checksum.',
    '2. On the bench host: stop the systemd service, deregister with a fresh removal token, `rm -rf /opt/actions-runner`, then re-run `install-runner.sh` with a fresh registration token. See [scripts/bench/README.md §6](../blob/main/scripts/bench/README.md#6-self-hosted-github-actions-runner).',
    '3. Reference this issue in the PR (`Closes #N`) so it auto-closes — or just leave it: the next monthly check auto-closes this issue once `RUNNER_VERSION` matches upstream.',
    '',
    '### To defer',
    '',
    'Leave this issue open. The next monthly check will update it in place if upstream ships another release before the bump lands; it will not spawn a duplicate.',
    '',
    '_Filed by [`bench-check-runner-version.mjs`](../blob/main/.github/scripts/bench-check-runner-version.mjs)._',
  ].join('\n');
}

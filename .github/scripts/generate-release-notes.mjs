#!/usr/bin/env node
// Generates the body for a GitHub Release.
//
// GitHub's /releases/generate-notes API picks `previous_tag_name` by
// walking every tag in the repo when not given an explicit one — that
// includes the non-wasmcloud tag namespaces (wash-cli-v…,
// runtime-operator/v…, control-interface-v…) and ends up picking the
// wrong predecessor for the wasmCloud release `v…` series. Compute
// the predecessor here from the release namespace only and pin it.
//
// Tag policy (matches semver release-namespace `vX.Y.Z[-prerelease]`):
//   - GA `vX.Y.Z`         → previous = immediate semver predecessor.
//   - RC `vX.Y.Z-rc.N`    → previous = `vX.Y.Z-rc.M` (same X.Y.Z base)
//                           if present; otherwise empty body.
//   - other pre-releases  → empty body (alpha/beta/draft/etc. don't
//     (alpha, beta, …)      get auto-generated notes).
//
// Inputs (env):
//   GH_TOKEN          GitHub PAT (or GITHUB_TOKEN) with `contents: read`.
//   GITHUB_REPOSITORY owner/repo (set by GHA).
//   TAG               The release tag being created, e.g. v2.2.0.
//   MERGE_SHA         The commit being tagged.
//   GITHUB_ENV        Set by GHA — STAGE marker is appended for the
//                     failure-notification step downstream.
//
// Outputs:
//   release-notes.md  The body to pass to action-gh-release as body_path.

import { execFileSync } from 'node:child_process';
import { appendFileSync, writeFileSync } from 'node:fs';

const RC_RE = /^v(\d+\.\d+\.\d+)-rc\.\d+$/;

function requireEnv(name) {
  const v = process.env[name];
  if (!v) {
    console.error(`missing required env var: ${name}`);
    process.exit(1);
  }
  return v;
}

function pickPreviousTag(tag, tags) {
  const idx = tags.indexOf(tag);
  if (idx < 0 || idx + 1 >= tags.length) return '';

  const rcMatch = tag.match(RC_RE);
  if (rcMatch) {
    // Only chain RCs of the same X.Y.Z base. The immediate successor
    // in the descending sort is the last tag cut before this one.
    const base = rcMatch[1];
    const candidate = tags[idx + 1];
    const cMatch = candidate.match(RC_RE);
    return cMatch && cMatch[1] === base ? candidate : '';
  }
  if (tag.includes('-')) {
    // Other pre-release flavors (alpha, beta, draft, etc.) don't chain.
    return '';
  }
  return tags[idx + 1];
}

async function main() {
  const token = requireEnv('GH_TOKEN');
  const repo = requireEnv('GITHUB_REPOSITORY');
  const tag = requireEnv('TAG');
  const mergeSha = requireEnv('MERGE_SHA');

  if (process.env.GITHUB_ENV) {
    appendFileSync(process.env.GITHUB_ENV, 'STAGE=generate-notes\n');
  }

  // Wasmcloud release tags are `vX.Y.Z[-prerelease]`. The glob filters
  // out other tag namespaces (their first segment doesn't start `v<digit>`).
  // `--sort=-version:refname` is git's semver-aware descending sort.
  const tagsOut = execFileSync(
    'git',
    ['tag', '--list', 'v[0-9]*.[0-9]*.[0-9]*', '--sort=-version:refname'],
    { encoding: 'utf8' },
  ).trim();
  const tags = tagsOut.length ? tagsOut.split('\n') : [];
  const previousTag = pickPreviousTag(tag, tags);

  // Pre-release tags only get a body when there's an in-series RC
  // predecessor. Anything else with a `-` (alpha/beta/draft/first RC
  // of a base) ships empty — auto-generated notes from a prior GA
  // would be noise on an in-progress release.
  if (tag.includes('-') && !previousTag) {
    console.log(`pre-release ${tag} has no in-series RC predecessor — empty body`);
    writeFileSync('release-notes.md', '');
    return;
  }

  const body = { tag_name: tag, target_commitish: mergeSha };
  if (previousTag) {
    body.previous_tag_name = previousTag;
    console.log(`previous tag: ${previousTag}`);
  } else {
    console.log('no previous tag found — letting the API auto-detect');
  }

  const res = await fetch(
    `https://api.github.com/repos/${repo}/releases/generate-notes`,
    {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: 'application/vnd.github+json',
        'X-GitHub-Api-Version': '2022-11-28',
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(body),
    },
  );
  if (!res.ok) {
    console.error(`generate-notes API ${res.status}: ${await res.text()}`);
    process.exit(1);
  }
  const data = await res.json();
  const notes = data.body ?? '';
  writeFileSync('release-notes.md', notes);

  console.log('--- generated body (first 20 lines) ---');
  console.log(notes.split('\n').slice(0, 20).join('\n'));
}

await main();

#!/usr/bin/env node
// Bumps the wit pipeline's pinned tool versions in wit.yml to upstream's
// latest stable GitHub releases. Driven by wit-tools-bump.yml.
//
// Each tracked tool has a `<VAR>: '<bare-semver>'` line in wit.yml's
// workflow-level env: block. For each, this resolves the latest
// non-prerelease, non-draft release and rewrites the pin in place.
//
// Tracked tools:
//   WASM_TOOLS_VERSION ← bytecodealliance/wasm-tools
//   WKG_VERSION        ← bytecodealliance/wasm-pkg-tools (wkg)
//
// Usage: node bump-wit-tools.mjs [path-to-wit.yml]
//   Defaults to .github/workflows/wit.yml; the optional arg exists so the
//   script can be exercised against a copy without touching the real file.
//
// Env:
//   GH_TOKEN / GITHUB_TOKEN   optional; raises the GitHub API rate limit.
//   GITHUB_OUTPUT             required; receives the step outputs.
//
// Outputs (to $GITHUB_OUTPUT):
//   changed=true|false
//   body=<markdown bullet list of the bumps, one per changed tool>
//
// Fails loudly if a release can't be resolved or a pin line is missing —
// a silent no-op would let a tool quietly rot at an old version.

import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';

const TOOLS = [
  {
    var: 'WASM_TOOLS_VERSION',
    repo: 'bytecodealliance/wasm-tools',
    name: 'wasm-tools',
  },
  {
    var: 'WKG_VERSION',
    repo: 'bytecodealliance/wasm-pkg-tools',
    name: 'wkg',
  },
];

// `releases/latest` returns the most recent non-prerelease, non-draft
// release. Strip the leading `v` to match wit.yml's bare-semver format.
async function latestVersion(repo, token) {
  const res = await fetch(`https://api.github.com/repos/${repo}/releases/latest`, {
    headers: {
      Accept: 'application/vnd.github+json',
      'User-Agent': 'wasmcloud-wit-tools-bump',
      'X-GitHub-Api-Version': '2022-11-28',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
  });
  if (!res.ok) {
    throw new Error(`GitHub API ${res.status} ${res.statusText} for ${repo}/releases/latest`);
  }
  const { tag_name: tag } = await res.json();
  if (!tag) {
    throw new Error(`no tag_name in latest release for ${repo}`);
  }
  return tag.replace(/^v/, '');
}

// The regex is anchored to the two-space indent that the workflow-level
// env: block uses; if that indent changes, this extractor must change too.
function pinRe(name) {
  return new RegExp(`^(  ${name}: ')([^']+)(')`, 'm');
}

async function main() {
  const file = process.argv[2] ?? '.github/workflows/wit.yml';

  const githubOutput = process.env.GITHUB_OUTPUT;
  if (!githubOutput) {
    console.error('GITHUB_OUTPUT env var is not set');
    process.exit(1);
  }

  const token = process.env.GH_TOKEN ?? process.env.GITHUB_TOKEN;

  let content = readFileSync(file, 'utf8');
  const bumps = [];

  for (const tool of TOOLS) {
    const re = pinRe(tool.var);
    const match = content.match(re);
    if (!match) {
      console.error(`no '${tool.var}:' pin found in ${file}`);
      process.exit(1);
    }
    const current = match[2];
    const latest = await latestVersion(tool.repo, token);
    if (current === latest) {
      console.log(`${tool.name} is already at ${latest} — nothing to do`);
      continue;
    }
    content = content.replace(re, `$1${latest}$3`);
    bumps.push(
      `- ${tool.name} \`${current}\` → \`${latest}\` ` +
        `([release notes](https://github.com/${tool.repo}/releases/tag/v${latest}))`,
    );
    console.log(`${tool.name} ${current} → ${latest}`);
  }

  const changed = bumps.length > 0;
  if (changed) {
    writeFileSync(file, content);
  }

  // Multiline output uses GitHub's heredoc form so the body's newlines
  // survive into the PR title/body. The delimiter cannot appear in the body.
  const body = bumps.join('\n');
  appendFileSync(githubOutput, `changed=${changed}\n`);
  appendFileSync(githubOutput, `body<<WIT_TOOLS_BUMP_EOF\n${body}\nWIT_TOOLS_BUMP_EOF\n`);
}

await main();

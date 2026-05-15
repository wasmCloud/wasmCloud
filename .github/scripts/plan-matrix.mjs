#!/usr/bin/env node
// Computes the build matrix for a paths-filter-gated workflow from a
// static ITEMS list. Shared by examples.yml and templates.yml so both
// workflows pick "what to build for this event" the same way.
//
// Inputs (env):
//   ITEMS         JSON array of build items. Each MUST have:
//                   key:    unique matrix identifier.
//                 May also have:
//                   filter: name of the dorny/paths-filter group that
//                           gates this item on PRs. Defaults to `key`.
//                           Multiple items may share a `filter` (e.g.
//                           grpc-hello-world client + server both gate
//                           on the same examples/grpc-hello-world/**
//                           filter group).
//                 Any other fields are passed through to the matrix
//                 verbatim — that's how per-item config (workdir, image,
//                 needs-protoc, …) reaches the build job.
//   CHANGED       JSON array from dorny/paths-filter's `outputs.changes`
//                 (the filter step is skipped on push, so this is empty
//                 or unset there).
//   EVENT_NAME    github.event_name.
//   CANARY        'true' if the PR carries the workflow's force-everything
//                 label (`examples-canary`, `templates-canary`, …).
//
// Output (to $GITHUB_OUTPUT):
//   matrix        JSON object `{include: [items…]}` shaped for
//                 `strategy.matrix: ${{ fromJSON(...) }}`.
//   has_items     'true' if at least one item will build — callers gate
//                 the matrix job on this so an empty matrix doesn't
//                 produce a runtime error.
//
// Build set:
//   non-PR (push, schedule, …):  every item
//   PR with canary label:        every item
//   PR (default):                items whose `filter` (or `key`) is in CHANGED

import { appendFileSync } from 'node:fs';

function requireEnv(name) {
  const v = process.env[name];
  if (v === undefined || v === '') {
    console.error(`${name} env var is required`);
    process.exit(1);
  }
  return v;
}

async function main() {
  const githubOutput = requireEnv('GITHUB_OUTPUT');
  const items = JSON.parse(requireEnv('ITEMS'));
  const changed = new Set(JSON.parse(process.env.CHANGED || '[]'));
  const isPR = process.env.EVENT_NAME === 'pull_request';
  const canary = process.env.CANARY === 'true';

  for (const it of items) {
    if (!it.key) {
      console.error(`ITEMS entry missing 'key': ${JSON.stringify(it)}`);
      process.exit(1);
    }
  }

  const toBuild = !isPR || canary
    ? items
    : items.filter((it) => changed.has(it.filter || it.key));

  const matrix = { include: toBuild };

  console.error(`event=${process.env.EVENT_NAME} canary=${canary}`);
  console.error(`changed=${JSON.stringify([...changed])}`);
  console.error(`to_build=${JSON.stringify(toBuild.map((it) => it.key))}`);

  appendFileSync(githubOutput, `matrix=${JSON.stringify(matrix)}\n`);
  appendFileSync(githubOutput, `has_items=${toBuild.length > 0}\n`);
}

await main();

#!/usr/bin/env node
// Generates a dorny/paths-filter `filters` YAML doc from a workflow's
// ITEMS list, so adding/removing a template or example doesn't require
// hand-editing per-group blocks. Shared by examples.yml and templates.yml.
//
// Inputs (env):
//   ITEMS          JSON array shaped like plan-matrix.mjs's ITEMS.
//                  Each entry MUST have `key` + `workdir`. Optional
//                  `filter` field names the paths-filter group; multiple
//                  items with the same `filter` coalesce into one group.
//   SHARED_PATHS   JSON array of path patterns appended to every group
//                  (action defs, shared scripts, the workflow file).
//   OUTPUT         Optional path to write the YAML doc to. If unset,
//                  writes to stdout.
//
// Per-group path:
//   Each group's primary path is the longest common ancestor of its
//   members' workdirs, suffixed with `/**`. Single-member groups → that
//   member's `workdir/**`. Multi-member groups → the shared parent dir
//   (e.g. grpc-hello-world client+server collapse to
//   `examples/grpc-hello-world/**`), so a change anywhere in the parent
//   tree rebuilds every member.

import { writeFileSync, writeSync } from 'node:fs';

const items = JSON.parse(process.env.ITEMS);
const sharedPaths = JSON.parse(process.env.SHARED_PATHS);
const output = process.env.OUTPUT;

const groups = new Map();
for (const it of items) {
  if (!it.key || !it.workdir) {
    console.error(`ITEMS entry must have key + workdir: ${JSON.stringify(it)}`);
    process.exit(1);
  }
  const name = it.filter || it.key;
  if (!groups.has(name)) groups.set(name, []);
  groups.get(name).push(it.workdir);
}

function commonPath(paths) {
  if (paths.length === 1) return paths[0];
  const parts = paths.map((p) => p.split('/'));
  const minLen = Math.min(...parts.map((p) => p.length));
  const common = [];
  for (let i = 0; i < minLen; i++) {
    const seg = parts[0][i];
    if (parts.every((p) => p[i] === seg)) common.push(seg);
    else break;
  }
  if (common.length === 0) {
    console.error(`workdirs in one group share no common prefix: ${JSON.stringify(paths)}`);
    process.exit(1);
  }
  return common.join('/');
}

const lines = [];
for (const [name, workdirs] of [...groups].sort(([a], [b]) => a.localeCompare(b))) {
  lines.push(`${name}:`);
  lines.push(`  - '${commonPath(workdirs)}/**'`);
  for (const p of sharedPaths) {
    lines.push(`  - '${p}'`);
  }
}
const yaml = lines.join('\n') + '\n';

if (output) {
  writeFileSync(output, yaml);
  console.log(`wrote ${groups.size} filter group(s) to ${output}`);
} else {
  writeSync(1, yaml);
}

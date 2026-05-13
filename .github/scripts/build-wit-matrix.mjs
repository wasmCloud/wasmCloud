#!/usr/bin/env node
// Builds the wit.yml publish matrix from wit-build/*.wasm.
//
// For each enriched WIT component artifact, wasm-tools re-emits its
// `package <ns>:<name>@<ver>;` header in canonical form. That's the
// source of truth for what to publish and where. Each artifact becomes
// one matrix entry:
//   { name, version, ref: ghcr.io/wasmcloud/interfaces/<name>:<version>,
//     artifact: <basename of wasm file> }
//
// Outputs (to $GITHUB_OUTPUT):
//   matrix=<JSON object with `include` array>
//   has_packages=true|false
//
// Fails loudly if a header is missing, malformed, or declares a
// namespace other than `wasmcloud`. The publish job pushes only to
// ghcr.io/wasmcloud/interfaces, so anything else would silently land
// in the wrong registry path.

import { execFileSync } from 'node:child_process';
import { appendFileSync, readdirSync } from 'node:fs';
import { basename } from 'node:path';

const BUILD_DIR = 'wit-build';
const REQUIRED_NAMESPACE = 'wasmcloud';
const REGISTRY_PATH = 'ghcr.io/wasmcloud/interfaces';
// `package wasmcloud:messaging@0.2.0;` — canonicalized by wasm-tools,
// so source-file comments / whitespace can't affect this.
const HEADER_RE = /^package ([^:]+):([^@]+)@([^;]+);/;

const githubOutput = process.env.GITHUB_OUTPUT;
if (!githubOutput) {
  console.error('GITHUB_OUTPUT env var is not set');
  process.exit(1);
}

const wasms = readdirSync(BUILD_DIR)
  .filter((f) => f.endsWith('.wasm'))
  .sort();

const items = [];
for (const file of wasms) {
  const path = `${BUILD_DIR}/${file}`;
  const wit = execFileSync('wasm-tools', ['component', 'wit', path], {
    encoding: 'utf8',
  });
  const header = wit.split('\n').find((line) => line.startsWith('package '));
  if (!header) {
    console.error(`no package header in ${path} — refusing to publish`);
    process.exit(1);
  }
  const match = header.match(HEADER_RE);
  if (!match) {
    console.error(`could not parse package header in ${path}: ${header}`);
    process.exit(1);
  }
  const [, namespace, name, version] = match;
  if (namespace !== REQUIRED_NAMESPACE) {
    console.error(
      `package ${path} declares namespace '${namespace}' but this workflow only publishes '${REQUIRED_NAMESPACE}:*' packages`,
    );
    process.exit(1);
  }
  items.push({
    name,
    version,
    ref: `${REGISTRY_PATH}/${name}:${version}`,
    artifact: basename(path),
  });
}

const matrix = { include: items };
const hasPackages = items.length > 0;

appendFileSync(githubOutput, `matrix=${JSON.stringify(matrix)}\n`);
appendFileSync(githubOutput, `has_packages=${hasPackages}\n`);

console.log('--- matrix ---');
console.log(JSON.stringify(matrix, null, 2));

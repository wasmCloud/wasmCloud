#!/usr/bin/env node
// Moves each VEX statement from release `--from` to release `--to`, after
// the release train bumps the workspace version. A statement is carried only
// while Cargo.lock still has every cargo subcomponent it names; the release
// PR's approval reviews what was carried.

import assert from 'node:assert/strict';
import { writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

import { VEX_PATH, describeDrift, lockDrift, readArtifacts, readCargo, readVex, releaseProducts } from './lib/vex.mjs';

// Returns a copy of `doc` covering `to` and the advisories it carried.
export function carryVex(doc, from, to, artifacts, lock, now) {
  const drift = lockDrift(doc, from, artifacts, lock).map(describeDrift);
  assert.ok(!drift.length, `reassess before releasing ${to}:\n${drift.join('\n')}`);
  const fromIds = releaseProducts(from, artifacts);
  const toIds = releaseProducts(to, artifacts);
  const next = structuredClone(doc);
  const carried = [];
  for (const statement of next.statements) {
    const name = statement.vulnerability?.name;
    const byId = new Map(statement.products.map((product) => [product['@id'], product]));
    if (toIds.every((id) => byId.has(id))) continue;
    const added = toIds.map((id, i) => {
      const prior = byId.get(fromIds[i]);
      assert.ok(prior, `${name} lacks release product ${fromIds[i]}; assess it by hand`);
      return { ...structuredClone(prior), '@id': id };
    });
    // Each release's assets keep its own assessment, so `from` products go.
    statement.products = [
      ...statement.products.filter((product) => !fromIds.includes(product['@id']) && !toIds.includes(product['@id'])),
      ...added.map((product) => byId.get(product['@id']) ?? product),
    ];
    carried.push({ name, status: statement.status, justification: statement.justification });
  }
  if (carried.length) {
    next.version = doc.version + 1;
    next.last_updated = now;
  }
  return { doc: next, carried };
}

// Markdown for the release PR body, one line per carried advisory.
export function carriedSummary(carried, from, to) {
  if (!carried.length) return '';
  const lines = carried.map(({ name, status, justification }) =>
    `- \`${name}\`: ${status}${justification ? ` (\`${justification}\`)` : ''}`);
  return [
    `**VEX:** these assessments were carried from v${from} to v${to}. Each dependency is unchanged in \`Cargo.lock\`; approving this PR confirms the code paths in each statement still hold.`,
    '',
    ...lines,
    '',
  ].join('\n');
}

function main(argv) {
  const { values } = parseArgs({
    args: argv,
    options: { from: { type: 'string' }, to: { type: 'string' }, summary: { type: 'string' } },
  });
  assert.ok(values.from && values.to, 'usage: release-vex.mjs --from X.Y.Z --to X.Y.Z [--summary FILE]');
  const { version, lock } = readCargo('.');
  assert.equal(version, values.to, `Cargo.toml is at ${version}; bump it to ${values.to} first`);
  const now = new Date().toISOString().replace(/\.\d{3}Z$/, 'Z');
  const { doc, carried } = carryVex(readVex('.'), values.from, values.to, readArtifacts('.'), lock, now);
  writeFileSync(join('.', VEX_PATH), `${JSON.stringify(doc, null, 2)}\n`);
  if (values.summary) writeFileSync(values.summary, carriedSummary(carried, values.from, values.to));
  console.log(`carried ${carried.length} VEX statements from ${values.from} to ${values.to}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

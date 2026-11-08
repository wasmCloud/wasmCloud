#!/usr/bin/env node
// Moves each VEX statement from release `--from` to release `--to`, after
// the release train bumps the workspace version. A statement that still
// suppresses a `.cargo/audit.toml` ignore is carried only while Cargo.lock
// still has every cargo subcomponent it names. A `not_affected` statement
// whose ignore was removed after a dependency update becomes `fixed` for
// `--to`. The release PR's approval reviews what was carried.

import assert from 'node:assert/strict';
import { writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

import { VEX_PATH, cargoPurl, describeDrift, lockDrift, readArtifacts, readCargo, readVex, releaseProducts } from './lib/vex.mjs';

// The cargo package names a statement's products name as subcomponents.
function cargoNames(statement) {
  const names = new Set();
  for (const product of statement.products ?? []) {
    for (const sub of product.subcomponents ?? []) {
      const pkg = cargoPurl(sub['@id']);
      if (pkg) names.add(pkg.name);
    }
  }
  return [...names].sort();
}

// The subcomponents a `fixed` statement records: each named package at the
// versions Cargo.lock now has. A package the lock dropped is left out.
function lockedSubcomponents(names, lock) {
  return names.flatMap((name) => [...(lock.get(name) ?? [])].sort().map((v) => ({ '@id': `pkg:cargo/${name}@${v}` })));
}

// Rewrites a statement for `to` as `fixed`, after its audit ignore was
// removed. Every assessed dependency version must be gone from Cargo.lock;
// otherwise the ignore was dropped without the update that fixes it.
function retire(statement, from, to, toIds, lock) {
  const name = statement.vulnerability?.name;
  const assessed = new Set();
  for (const product of statement.products) {
    for (const sub of product.subcomponents ?? []) assessed.add(sub['@id']);
  }
  const stale = [...assessed].filter((id) => {
    const pkg = cargoPurl(id);
    return pkg && lock.get(pkg.name)?.has(pkg.version);
  });
  assert.ok(!stale.length,
    `${name} is no longer ignored in .cargo/audit.toml, but Cargo.lock still has ${stale.join(', ')}; restore the ignore or update the dependency`);
  const subcomponents = lockedSubcomponents(cargoNames(statement), lock);
  const now = subcomponents.map((sub) => sub['@id'].replace(/^pkg:cargo\//, '')).join(', ') || 'none of the affected packages';
  statement.status = 'fixed';
  delete statement.justification;
  delete statement.impact_statement;
  delete statement.action_statement;
  statement.status_notes = `Fixed in v${to} by a dependency update; Cargo.lock now has ${now}. The v${from} release assets keep the earlier assessment.`;
  statement.products = toIds.map((id) => (subcomponents.length ? { '@id': id, subcomponents } : { '@id': id }));
}

// Returns a copy of `doc` covering `to` and the advisories it carried.
// `ignored` is the `.cargo/audit.toml` ignore list.
export function carryVex(doc, from, to, artifacts, lock, now, ignored) {
  const drift = lockDrift(doc, from, artifacts, lock, ignored).map(describeDrift);
  assert.ok(!drift.length, `reassess before releasing ${to}:\n${drift.join('\n')}`);
  const suppressing = new Set(ignored);
  const fromIds = releaseProducts(from, artifacts);
  const toIds = releaseProducts(to, artifacts);
  const next = structuredClone(doc);
  const carried = [];
  for (const statement of next.statements) {
    const name = statement.vulnerability?.name;
    const byId = new Map(statement.products.map((product) => [product['@id'], product]));
    if (toIds.every((id) => byId.has(id))) continue;
    if (statement.status === 'not_affected' && !suppressing.has(name)) {
      retire(statement, from, to, toIds, lock);
      carried.push({ name, status: statement.status });
      continue;
    }
    if (statement.status === 'fixed') {
      // A fix stays fixed; record the dependency versions this release ships.
      const subcomponents = lockedSubcomponents(cargoNames(statement), lock);
      statement.products = toIds.map((id) => (subcomponents.length ? { '@id': id, subcomponents } : { '@id': id }));
      carried.push({ name, status: statement.status });
      continue;
    }
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
    `**VEX:** these assessments were carried from v${from} to v${to}. Each \`not_affected\` dependency is unchanged in \`Cargo.lock\`; approving this PR confirms the code paths in each statement still hold. Each \`fixed\` statement records an advisory a dependency update resolved.`,
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
  const { ignored, version, lock } = readCargo('.');
  assert.equal(version, values.to, `Cargo.toml is at ${version}; bump it to ${values.to} first`);
  const now = new Date().toISOString().replace(/\.\d{3}Z$/, 'Z');
  const { doc, carried } = carryVex(readVex('.'), values.from, values.to, readArtifacts('.'), lock, now, ignored);
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

#!/usr/bin/env node
// Requires a current release assessment for every Cargo audit ignore, and the
// same ignore in osv-scanner.toml so both scanners suppress the same advisories.

import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describeDrift, lockDrift, readArtifacts, readCargo, readVex, releaseProducts } from './lib/vex.mjs';

const JUSTIFICATIONS = new Set([
  'vulnerable_code_not_in_execute_path',
  'vulnerable_code_cannot_be_controlled_by_adversary',
]);

function text(value, label) {
  assert.equal(typeof value, 'string', `${label} must be a string`);
  assert.ok(value.trim(), `${label} must not be empty`);
}

export function checkVex(doc, ignored, version, artifacts) {
  assert.equal(doc['@context'], 'https://openvex.dev/ns/v0.2.0', 'invalid OpenVEX context');
  for (const field of ['@id', 'author', 'timestamp']) text(doc[field], field);
  assert.ok(Number.isFinite(Date.parse(doc.timestamp)), 'invalid timestamp');
  assert.ok(Number.isInteger(doc.version) && doc.version >= 1, 'invalid document version');
  assert.ok(Array.isArray(doc.statements) && doc.statements.length, 'no VEX statements');
  const requiredProducts = releaseProducts(version, artifacts);
  const covered = new Set();
  const seen = new Set();
  for (const statement of doc.statements) {
    const name = statement.vulnerability?.name;
    text(name, 'advisory name');
    text(statement.vulnerability['@id'], `${name} advisory URL`);
    assert.ok(!seen.has(name), `duplicate statement for ${name}`);
    seen.add(name);
    assert.ok(['not_affected', 'affected', 'fixed', 'under_investigation'].includes(statement.status), `${name} has an invalid status`);
    if (statement.status === 'not_affected') {
      assert.ok(JUSTIFICATIONS.has(statement.justification), `${name} needs a non-exploitability justification`);
      text(statement.impact_statement, `${name} impact_statement`);
      covered.add(name);
    } else if (statement.status === 'affected') {
      text(statement.action_statement, `${name} action_statement`);
    }
    text(statement.status_notes, `${name} status_notes`);
    assert.ok(Array.isArray(statement.products), `${name} needs release products`);
    const products = new Set(statement.products.map((product) => product['@id']));
    for (const product of requiredProducts) {
      assert.ok(products.has(product), `${name} lacks release product ${product}; reassess the release`);
    }
  }
  for (const advisory of ignored) {
    assert.ok(covered.has(advisory), `Cargo audit ignores ${advisory} without a VEX statement`);
  }
}

// OSV-Scanner reads the same Cargo.lock as cargo-audit. An advisory VEX marks
// not_affected there must be ignored by both, or osv-scanner fails gate-audit.
export function checkOsvMirror(ignored, osvIgnored) {
  const osv = new Set(osvIgnored);
  for (const advisory of ignored) {
    assert.ok(osv.has(advisory), `Cargo audit ignores ${advisory} but osv-scanner.toml does not`);
  }
}

// With `--release`, a dependency that moved since its assessment is an error.
// Between releases it is a warning, since main still carries the last
// release's version and products.
function main(args) {
  const release = args.includes('--release');
  const { ignored, osvIgnored, version, lock } = readCargo('.');
  const artifacts = readArtifacts('.');
  const doc = readVex('.');
  checkVex(doc, ignored, version, artifacts);
  checkOsvMirror(ignored, osvIgnored);
  const drift = lockDrift(doc, version, artifacts, lock, ignored).map(describeDrift);
  if (release) {
    assert.ok(!drift.length, `reassess before releasing ${version}:\n${drift.join('\n')}`);
  }
  for (const line of drift) {
    console.log(`::warning title=VEX assessment out of date::${line}; reassess it before the next release`);
  }
  console.log(`VEX covers all ${ignored.length} Cargo audit ignores for release ${version}.`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

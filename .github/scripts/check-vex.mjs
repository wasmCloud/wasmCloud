#!/usr/bin/env node
// Requires a current release assessment for every Cargo audit ignore.

import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

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
  const requiredProducts = [
    ...artifacts.map((name) => `pkg:github/wasmcloud/wasmcloud@v${version}?file_name=${name}`),
    ...['', '-all-features'].map((suffix) =>
      `pkg:oci/wash@${version}${suffix}?repository_url=ghcr.io%2Fwasmcloud%2Fwash`),
  ];
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

function main() {
  // Use the runner's TOML parser for Cargo's quoting, comments, and arrays.
  const { ignored, version } = JSON.parse(execFileSync('python3', ['-c', `
import json, pathlib, tomllib
audit = tomllib.loads(pathlib.Path('.cargo/audit.toml').read_text())
manifest = tomllib.loads(pathlib.Path('Cargo.toml').read_text())
print(json.dumps({'ignored': audit.get('advisories', {}).get('ignore', []),
                  'version': manifest['workspace']['package']['version']}))
  `], { encoding: 'utf8' }));
  const workflow = readFileSync('.github/workflows/wash.yml', 'utf8');
  const artifacts = [...new Set([...workflow.matchAll(/^\s+artifact: (wash-[\w-]+)\s*$/gm)].map((m) => m[1]))];
  assert.ok(artifacts.length, 'no release binary targets found in wash.yml');
  artifacts.push('wash-x86_64-pc-windows-msvc.zip');
  const doc = JSON.parse(readFileSync('vex/wasmcloud.openvex.json', 'utf8'));
  checkVex(doc, ignored, version, artifacts);
  console.log(`VEX covers all ${ignored.length} Cargo audit ignores for release ${version}.`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

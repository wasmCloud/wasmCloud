#!/usr/bin/env node
// Runs `cargo deny check licenses` against every Rust workspace in the repo,
// including standalone examples and templates, using the root deny.toml.
//
// deny.toml's targets are the ones wash ships for. Standalone workspaces build
// components, so they're checked for wasm too. cargo-deny takes one --target
// per run and doesn't need the target installed.
//
// Every workspace is checked before failing, so one run reports them all.
//
// Invoked by .github/workflows/audit.yml. Needs `git` and `cargo-deny` on PATH.

import { execFileSync, spawnSync } from 'node:child_process';
import { relative, resolve } from 'node:path';

const WASM_TARGETS = ['wasm32-unknown-unknown', 'wasm32-wasip1', 'wasm32-wasip2', 'wasm32-wasip3'];

const ROOT_MANIFEST = resolve('Cargo.toml');

function capture(command, args) {
  return execFileSync(command, args, { encoding: 'utf8' }).trim();
}

const manifests = capture('git', ['ls-files', 'Cargo.toml', '**/Cargo.toml']).split('\n');
const workspaces = new Set(
  manifests.map((manifest) =>
    capture('cargo', [
      'locate-project',
      '--workspace',
      '--message-format',
      'plain',
      '--manifest-path',
      manifest,
    ]),
  ),
);

const failures = [];

function check(workspace, target) {
  const args = [
    'deny',
    '--manifest-path',
    workspace,
    '--workspace',
    '--config',
    'deny.toml',
    '--locked',
  ];
  if (target) {
    args.push('--target', target);
  }
  args.push('check', 'licenses');

  const { status, error } = spawnSync('cargo', args, { stdio: 'inherit' });
  if (error) {
    throw error;
  }
  if (status !== 0) {
    failures.push(`${relative('.', workspace)}${target ? ` (${target})` : ''}`);
  }
}

for (const workspace of [...workspaces].sort()) {
  console.log(`Checking licenses: ${relative('.', workspace)}`);
  check(workspace);
  if (workspace !== ROOT_MANIFEST) {
    for (const target of WASM_TARGETS) {
      check(workspace, target);
    }
  }
}

if (failures.length > 0) {
  console.error('\nLicense check failed for:\n');
  for (const failure of failures) {
    console.error(`  - ${failure}`);
  }
  process.exit(1);
}

console.log(`\nLicenses ok across ${workspaces.size} workspaces.`);

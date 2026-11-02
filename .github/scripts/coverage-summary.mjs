#!/usr/bin/env node

import { appendFileSync, readFileSync } from 'node:fs';

const destination = process.env.GITHUB_STEP_SUMMARY;
if (!destination) throw new Error('GITHUB_STEP_SUMMARY is required');

const coverage = readFileSync('target/coverage/summary.txt', 'utf8').trimEnd();
// `cargo xtask coverage test` records the thresholds `check` enforces.
const thresholds = readFileSync('target/coverage/provenance.txt', 'utf8')
  .match(/^Thresholds: (.+)$/m)?.[1];
if (!thresholds) throw new Error('provenance.txt does not record the thresholds');
appendFileSync(destination, [
  '## Native Rust coverage',
  '',
  `Commit: ${process.env.GITHUB_SHA}`,
  `Tests: ${process.env.TEST_RESULT}; ${thresholds}: ${process.env.THRESHOLD_RESULT}`,
  '',
  'Scope: native Rust workspace, excluding xtask and generated protobuf bindings.',
  'Rust compiler regions and lines are reported separately from statement coverage.',
  'See CONTRIBUTING.md for scope and the OpenSSF evidence requirements.',
  '',
  '```text',
  coverage,
  '```',
  '',
  'Download rust-coverage for HTML, JSON, LCOV, and toolchain provenance.',
  '',
].join('\n'));

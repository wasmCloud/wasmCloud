#!/usr/bin/env node

import { appendFileSync, readFileSync } from 'node:fs';

const destination = process.env.GITHUB_STEP_SUMMARY;
if (!destination) throw new Error('GITHUB_STEP_SUMMARY is required');

const coverage = readFileSync('target/coverage/summary.txt', 'utf8').trimEnd();
appendFileSync(destination, [
  '## Native Rust coverage',
  '',
  `Commit: ${process.env.GITHUB_SHA}`,
  `Tests: ${process.env.TEST_RESULT}; 80% lines and regions: ${process.env.THRESHOLD_RESULT}`,
  '',
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

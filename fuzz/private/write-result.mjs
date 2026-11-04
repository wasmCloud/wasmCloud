import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';

const policy = JSON.parse(readFileSync(new URL('./policy.json', import.meta.url), 'utf8'));
const needs = JSON.parse(process.env.RESULTS);
assert.equal(needs.resolve.result, 'success');
assert.equal(needs.go.result, 'success');
assert.equal(needs.rust.result, 'success');

const result = {
  policy,
  source_sha: needs.resolve.outputs.sha,
  harness_sha: needs.resolve.outputs.harness,
  seconds: Number(needs.resolve.outputs.seconds),
  workflow_sha: process.env.GITHUB_SHA,
  run_id: process.env.GITHUB_RUN_ID,
  run_attempt: process.env.GITHUB_RUN_ATTEMPT,
  request_id: process.env.REQUEST_ID || '',
};
assert.match(result.source_sha, /^[0-9a-f]{40}$/);
assert.match(result.harness_sha, /^[0-9a-f]{40}$/);
writeFileSync('result.json', `${JSON.stringify(result)}\n`);

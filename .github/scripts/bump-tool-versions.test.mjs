import assert from 'node:assert/strict';
import test from 'node:test';

import { parseReleaseTag } from './bump-tool-versions.mjs';

test('accepts the tag formats the tracked tools publish', () => {
  // wasm-tools, cargo-llvm-cov and gobco use vMAJOR.MINOR.PATCH.
  assert.equal(parseReleaseTag('bytecodealliance/wasm-tools', 'v1.262.0'), '1.262.0');
  assert.equal(parseReleaseTag('taiki-e/cargo-llvm-cov', 'v0.9.1'), '0.9.1');
  assert.equal(parseReleaseTag('rillig/gobco', 'v1.3.4'), '1.3.4');
  // protobuf uses vMAJOR.MINOR.
  assert.equal(parseReleaseTag('protocolbuffers/protobuf', 'v36.2'), '36.2');
});

test('accepts a bare version without a leading v', () => {
  assert.equal(parseReleaseTag('example/tool', '1.2.3'), '1.2.3');
});

test('rejects tags that could inject shell or env syntax', () => {
  for (const tag of [
    '1.2$(curl https://example.invalid)',
    'v1.2.3`id`',
    '1.2.3;rm -rf /',
    '1.2.3\nEVIL=1',
    '1.2.3 ',
    '"1.2.3"',
    '1.2.3$IFS',
  ]) {
    assert.throws(() => parseReleaseTag('example/tool', tag), /refusing release tag/, tag);
  }
});

test('rejects non-numeric and malformed versions', () => {
  for (const tag of [
    '',
    'v',
    '1',
    'v1',
    '1.2.3.4',
    '1.2.3-rc.1',
    '1.2.3+build',
    'vv1.2.3',
    'release-1.2.3',
    '1..2',
    'latest',
  ]) {
    assert.throws(() => parseReleaseTag('example/tool', tag), /refusing release tag/, tag);
  }
});

test('rejects a missing or non-string tag', () => {
  assert.throws(() => parseReleaseTag('example/tool', undefined), /not a string/);
  assert.throws(() => parseReleaseTag('example/tool', 123), /not a string/);
});

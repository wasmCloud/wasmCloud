import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { fakeTools, platformIndex } from './lib/release-fakes.mjs';

const script = fileURLToPath(new URL('./release-images.mjs', import.meta.url));
const SHA = 'a'.repeat(40);

function run(tools, command, env = {}) {
  return spawnSync(process.execPath, [script, command], {
    cwd: tools.root,
    env: { ...tools.env, IMAGE_TAG: '2.0.0', MERGE_SHA: SHA, ...env },
    encoding: 'utf8',
  });
}

function outputs(tools) {
  return Object.fromEntries(
    readFileSync(tools.env.GITHUB_OUTPUT, 'utf8').trim().split('\n').map((l) => l.split(/=(.*)/s).slice(0, 2)),
  );
}

function canaries() {
  const index = (d) => ({ digest: d, manifests: platformIndex(`${d}-amd64`, `${d}-arm64`) });
  return {
    [`ghcr.io/wasmcloud/wash:sha-${SHA}`]: index('sha256:wash'),
    [`ghcr.io/wasmcloud/wash:sha-${SHA}-all-features`]: index('sha256:all'),
    [`ghcr.io/wasmcloud/runtime-gateway:sha-${SHA}`]: index('sha256:gw'),
    [`ghcr.io/wasmcloud/runtime-operator:sha-${SHA}`]: index('sha256:op'),
  };
}

test('resolve keeps published tags and plans promotion for the rest, then promote retags', () => {
  const tools = fakeTools();
  try {
    tools.writeRegistry({ ...canaries(), 'ghcr.io/wasmcloud/runtime-gateway:2.0.0': { digest: 'sha256:gw-released' } });
    const resolved = run(tools, 'resolve');
    assert.equal(resolved.status, 0, resolved.stderr);
    const plan = JSON.parse(outputs(tools).plan);
    assert.deepEqual(
      plan.map(({ key, tag, digest, promote, name, ecosystem }) => [key, tag, digest, promote, name, ecosystem]),
      [
        ['wash', '2.0.0', 'sha256:wash', true, 'wash', 'cargo'],
        ['gateway', '2.0.0', 'sha256:gw-released', false, 'runtime-gateway', 'golang'],
        ['operator', '2.0.0', 'sha256:op', true, 'runtime-operator', 'golang'],
        ['wash_all_features', '2.0.0-all-features', 'sha256:all', true, 'wash-all-features', 'cargo'],
      ],
    );

    const promoted = run(tools, 'promote', { PLAN: JSON.stringify(plan) });
    assert.equal(promoted.status, 0, promoted.stderr);
    const out = outputs(tools);
    assert.equal(out.wash_digest, 'sha256:wash');
    assert.equal(out.gateway_digest, 'sha256:gw-released');
    assert.equal(out.wash_all_features_digest, 'sha256:all');
    const registry = JSON.parse(readFileSync(tools.env.FAKE_REGISTRY, 'utf8'));
    assert.equal(registry['ghcr.io/wasmcloud/runtime-operator:2.0.0']?.digest, 'sha256:op');
    assert.equal(registry['ghcr.io/wasmcloud/runtime-gateway:2.0.0'].digest, 'sha256:gw-released');
  } finally {
    rmSync(tools.root, { recursive: true, force: true });
  }
});

test('resolve refuses an unexpected registry error', () => {
  const tools = fakeTools();
  try {
    tools.writeRegistry({ ...canaries(), 'ghcr.io/wasmcloud/wash:2.0.0': 'error' });
    const res = run(tools, 'resolve');
    assert.equal(res.status, 1);
    assert.match(res.stderr, /refusing to promote[\s\S]*unauthorized/);
  } finally {
    rmSync(tools.root, { recursive: true, force: true });
  }
});

test('resolve fails when a canary image is missing', () => {
  const tools = fakeTools();
  try {
    const registry = canaries();
    delete registry[`ghcr.io/wasmcloud/wash:sha-${SHA}-all-features`];
    tools.writeRegistry(registry);
    const res = run(tools, 'resolve');
    assert.equal(res.status, 1);
    assert.match(res.stderr, /canary image .*-all-features does not exist/);
  } finally {
    rmSync(tools.root, { recursive: true, force: true });
  }
});

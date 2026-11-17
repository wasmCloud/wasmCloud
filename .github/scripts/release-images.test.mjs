import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { fakeTools, platformIndex } from './lib/release-fakes.mjs';

const script = fileURLToPath(new URL('./release-images.mjs', import.meta.url));
const SHA = 'a'.repeat(40);
const digest = (character) => `sha256:${character.repeat(64)}`;
const images = [
  ['wash', 'wash', digest('1'), 123],
  ['runtime-gateway', 'runtime-gateway', digest('2'), 124],
  ['runtime-operator', 'runtime-operator', digest('3'), 125],
  ['wash-all-features', 'wash', digest('4'), 123],
];

function fixture() {
  const tools = fakeTools();
  const records = Object.fromEntries(images.map(([name, workflow, value, runId]) => [name, {
    image: `ghcr.io/wasmcloud/${workflow}`, digest: value, sha: SHA, runId: String(runId),
  }]));
  const runs = Object.fromEntries(images.map(([, workflow, , id]) => [workflow, [{
    id, head_sha: SHA, head_branch: 'main', path: `.github/workflows/${workflow}.yml`,
    event: 'push', status: 'completed', conclusion: 'success',
  }]]));
  const registry = Object.fromEntries(images.map(([, workflow, value]) => [
    `ghcr.io/wasmcloud/${workflow}@${value}`,
    { digest: value, manifests: platformIndex(`${value}-amd64`, `${value}-arm64`) },
  ]));
  // The fake GitHub API and artifact downloads remain separate from registry tags.
  const gh = join(tools.root, 'bin', 'gh');
  writeFileSync(gh, `#!/usr/bin/env node
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
const { records, runs } = JSON.parse(readFileSync(process.env.FAKE_CANARIES, 'utf8'));
const args = process.argv.slice(2);
if (args[0] === 'api') {
  const workflow = args.at(-1).split('/workflows/')[1].split('.yml')[0];
  console.log(JSON.stringify([{ workflow_runs: runs[workflow] }]));
} else if (args[0] === 'run' && args[1] === 'download') {
  const name = args[args.indexOf('--name') + 1].replace('release-image-', '');
  if (!records[name]) process.exit(1);
  const directory = args[args.indexOf('--dir') + 1];
  mkdirSync(directory, { recursive: true });
  writeFileSync(join(directory, 'image.json'), JSON.stringify(records[name]));
} else process.exit(2);
`);
  chmodSync(gh, 0o755);
  tools.env.FAKE_CANARIES = join(tools.root, 'canaries.json');
  tools.env.GH_REPO = 'wasmCloud/wasmCloud';
  const save = () => {
    tools.writeRegistry(registry);
    writeFileSync(tools.env.FAKE_CANARIES, JSON.stringify({ records, runs }));
  };
  save();
  return { tools, records, runs, registry, save };
}

function run(tools, command, env = {}) {
  return spawnSync(process.execPath, [script, command], {
    cwd: tools.root,
    env: { ...tools.env, IMAGE_TAG: '2.0.0', MERGE_SHA: SHA, ...env },
    encoding: 'utf8',
  });
}

function outputs(tools) {
  return Object.fromEntries(
    readFileSync(tools.env.GITHUB_OUTPUT, 'utf8').trim().split('\n').map((line) => line.split(/=(.*)/s).slice(0, 2)),
  );
}

test('resolve promotes the recorded digests even when canary tags point elsewhere', () => {
  const { tools, registry, save } = fixture();
  try {
    registry[`ghcr.io/wasmcloud/wash:sha-${SHA}`] = { digest: digest('f') };
    registry['ghcr.io/wasmcloud/runtime-gateway:2.0.0'] = registry[`ghcr.io/wasmcloud/runtime-gateway@${digest('2')}`];
    save();
    const resolved = run(tools, 'resolve');
    assert.equal(resolved.status, 0, resolved.stderr);
    const plan = JSON.parse(outputs(tools).plan);
    assert.deepEqual(plan.map(({ digest: value, promote }) => [value, promote]), [
      [digest('1'), true], [digest('2'), false], [digest('3'), true], [digest('4'), true],
    ]);
    const promoted = run(tools, 'promote', { PLAN: JSON.stringify(plan) });
    assert.equal(promoted.status, 0, promoted.stderr);
    const out = outputs(tools);
    assert.equal(out.wash_digest, digest('1'));
    assert.equal(out.gateway_digest, digest('2'));
    assert.equal(out.wash_all_features_digest, digest('4'));
  } finally {
    rmSync(tools.root, { recursive: true, force: true });
  }
});

for (const [name, mutate, message] of [
  ['a conflicting published tag', (f) => { f.registry['ghcr.io/wasmcloud/wash:2.0.0'] = { digest: digest('f') }; }, /differs from successful canary/],
  ['a registry failure', (f) => { f.registry['ghcr.io/wasmcloud/wash:2.0.0'] = 'error'; }, /refusing to promote/],
  ['a missing artifact', (f) => { delete f.records.wash; }, /Command failed/],
  ['a record from another commit', (f) => { f.records.wash.sha = 'b'.repeat(40); }, /invalid wash canary digest/],
  ['a record from another run', (f) => { f.records.wash.runId = '999'; }, /invalid wash canary digest/],
  ['a record for another image', (f) => { f.records.wash.image = 'ghcr.io/attacker/wash'; }, /invalid wash canary digest/],
  ['an invalid digest', (f) => { f.records.wash.digest = 'latest'; }, /invalid wash canary digest/],
  ['an unavailable verified digest', (f) => { delete f.registry[`ghcr.io/wasmcloud/wash@${digest('1')}`]; }, /verified canary .* unavailable/],
  ['a failed canary run', (f) => { f.runs.wash[0].conclusion = 'failure'; }, /no successful wash canary/],
  ['a PR canary run', (f) => { f.runs.wash[0].event = 'pull_request'; }, /no successful wash canary/],
  ['a different branch', (f) => { f.runs.wash[0].head_branch = 'other'; }, /no successful wash canary/],
  ['a different workflow', (f) => { f.runs.wash[0].path = '.github/workflows/untrusted.yml'; }, /no successful wash canary/],
]) {
  test(`resolve rejects ${name}`, () => {
    const f = fixture();
    try {
      mutate(f);
      f.save();
      const result = run(f.tools, 'resolve');
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, message);
      assert.equal(readFileSync(f.tools.env.GITHUB_OUTPUT, 'utf8'), '');
    } finally {
      rmSync(f.tools.root, { recursive: true, force: true });
    }
  });
}

test('promote rejects a release tag changed after resolution', () => {
  const f = fixture();
  try {
    assert.equal(run(f.tools, 'resolve').status, 0);
    const plan = outputs(f.tools).plan;
    f.registry['ghcr.io/wasmcloud/wash:2.0.0'] = { digest: digest('f') };
    f.save();
    const result = run(f.tools, 'promote', { PLAN: plan });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /changed since resolution/);
  } finally {
    rmSync(f.tools.root, { recursive: true, force: true });
  }
});

test('record uses the manifest creation metadata and workflow identity', () => {
  const f = fixture();
  try {
    const metadata = join(f.tools.root, 'metadata.json');
    const record = join(f.tools.root, 'image.json');
    writeFileSync(metadata, JSON.stringify({ 'containerimage.descriptor': { digest: digest('5') } }));
    const env = { IMAGE_NAME: 'wash', IMAGE_METADATA: metadata, IMAGE_RECORD: record, GITHUB_SHA: SHA, GITHUB_RUN_ID: '123' };
    const result = run(f.tools, 'record', env);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(readFileSync(record, 'utf8')), {
      image: 'ghcr.io/wasmcloud/wash', digest: digest('5'), sha: SHA, runId: '123',
    });
    writeFileSync(metadata, '{}');
    assert.notEqual(run(f.tools, 'record', env).status, 0);
  } finally {
    rmSync(f.tools.root, { recursive: true, force: true });
  }
});

test('a different manifest digest after retagging blocks attestation outputs', () => {
  const f = fixture();
  try {
    assert.equal(run(f.tools, 'resolve').status, 0);
    const plan = JSON.parse(outputs(f.tools).plan);
    writeFileSync(f.tools.env.GITHUB_OUTPUT, '');
    const docker = join(f.tools.root, 'bin', 'docker');
    const fake = readFileSync(docker, 'utf8').replace(
      'registry[tag] = lookup(args.at(-1));',
      `registry[tag] = { ...lookup(args.at(-1)), digest: '${digest('f')}' };`,
    );
    writeFileSync(docker, fake);
    const result = run(f.tools, 'promote', { PLAN: JSON.stringify(plan) });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /differs from verified canary/);
    assert.equal(readFileSync(f.tools.env.GITHUB_OUTPUT, 'utf8'), '');
  } finally {
    rmSync(f.tools.root, { recursive: true, force: true });
  }
});

test('all four canary records come from manifest creation metadata and reach promotion', () => {
  const reusable = readFileSync(new URL('../workflows/docker-build-push.yml', import.meta.url), 'utf8');
  const wash = readFileSync(new URL('../workflows/wash.yml', import.meta.url), 'utf8');
  const release = readFileSync(new URL('../workflows/release-tag.yml', import.meta.url), 'utf8');
  assert.match(reusable, /imagetools create --metadata-file "\$RUNNER_TEMP\/canary-image.json"/);
  assert.match(reusable, /name: release-image-\$\{\{ needs.build.outputs.artifact_prefix \}\}/);
  for (const name of ['wash', 'wash-all-features']) {
    assert.ok(wash.includes(`--metadata-file "$RUNNER_TEMP/${name}-image.json"`));
    assert.ok(wash.includes(`name: release-image-${name}\n`));
    assert.ok(wash.includes(`IMAGE_METADATA: \${{ runner.temp }}/${name}-image.json`));
  }
  assert.match(release, /GH_TOKEN: \$\{\{ github.token \}\}/);
  assert.match(release, /GH_REPO: \$\{\{ github.repository \}\}/);
});

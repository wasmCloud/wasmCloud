import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { fakeTools, platformIndex } from './lib/release-fakes.mjs';

const script = fileURLToPath(new URL('./release-sboms.mjs', import.meta.url));
const IMAGE = 'ghcr.io/wasmcloud/runtime-gateway';
const BINARY = 'wash-x86_64-unknown-linux-gnu';
const PLAN = JSON.stringify([{ image: IMAGE, digest: 'sha256:index', name: 'runtime-gateway', ecosystem: 'golang' }]);

function run(tools, args, env = {}) {
  return spawnSync(process.execPath, [script, ...args], {
    cwd: tools.root,
    env: { ...tools.env, PLAN, TAG: 'v2.0.0', MERGE_SHA: 'a'.repeat(40), GITHUB_REPOSITORY: 'wasmCloud/wasmCloud', ...env },
    encoding: 'utf8',
  });
}

// A release with one image and one binary artifact, both with dependencies.
function release({ manifests = platformIndex('sha256:amd64', 'sha256:arm64'), binaryPurls } = {}) {
  const tools = fakeTools();
  tools.writeRegistry({ [`${IMAGE}@sha256:index`]: { digest: 'sha256:index', manifests } });
  const deps = (eco) => [`pkg:${eco}/app@1`, `pkg:${eco}/dep@1`];
  tools.writeSboms({
    [`registry:${IMAGE}@sha256:amd64`]: deps('golang'),
    [`registry:${IMAGE}@sha256:arm64`]: deps('golang'),
    [`file:artifacts/${BINARY}/${BINARY}`]: binaryPurls ?? deps('cargo'),
  });
  for (const dir of [BINARY, `${BINARY.replace('linux-gnu', 'pc-windows-msvc')}.zip`]) {
    mkdirSync(join(tools.root, 'artifacts', dir), { recursive: true });
  }
  writeFileSync(join(tools.root, 'artifacts', BINARY, BINARY), 'binary');
  return tools;
}

function sboms(tools) {
  const dir = join(tools.root, 'artifacts', 'sboms');
  return Object.fromEntries(readdirSync(dir).map((f) => [f, readFileSync(join(dir, f), 'utf8')]));
}

test('generate writes normalized, reproducible SBOMs for every image platform and binary', () => {
  const tools = release();
  try {
    const first = run(tools, ['generate']);
    assert.equal(first.status, 0, first.stderr);
    const docs = sboms(tools);
    assert.deepEqual(Object.keys(docs).sort(), [
      'runtime-gateway-image-linux-amd64.spdx.json',
      'runtime-gateway-image-linux-arm64.spdx.json',
      `${BINARY}.spdx.json`,
    ]);
    const doc = JSON.parse(docs[`${BINARY}.spdx.json`]);
    assert.equal(doc.documentNamespace, `https://github.com/wasmCloud/wasmCloud/releases/download/v2.0.0/${BINARY}.spdx.json`);
    assert.equal(doc.creationInfo.created, '2023-11-14T22:13:20Z');

    assert.equal(run(tools, ['generate']).status, 0);
    assert.deepEqual(sboms(tools), docs);
  } finally {
    rmSync(tools.root, { recursive: true, force: true });
  }
});

test('generate rejects a binary without embedded dependencies', () => {
  const tools = release({ binaryPurls: ['pkg:cargo/app@1'] });
  try {
    const res = run(tools, ['generate']);
    assert.equal(res.status, 1);
    assert.match(res.stderr, new RegExp(`${BINARY}: SBOM has 1 pkg:cargo/ packages`));
  } finally {
    rmSync(tools.root, { recursive: true, force: true });
  }
});

test('generate rejects an image without exactly linux/amd64 and linux/arm64', () => {
  const tools = release({
    manifests: platformIndex('sha256:amd64', 'sha256:arm64', [
      { digest: 'sha256:s390x', platform: { os: 'linux', architecture: 's390x' } },
    ]),
  });
  try {
    const res = run(tools, ['generate']);
    assert.equal(res.status, 1);
    assert.match(res.stderr, /expected platforms linux\/amd64, linux\/arm64, found linux\/amd64, linux\/arm64, linux\/s390x/);
  } finally {
    rmSync(tools.root, { recursive: true, force: true });
  }
});

test('image-subjects returns the scanned platform digests and refuses a changed index', () => {
  const tools = release();
  try {
    assert.equal(run(tools, ['generate']).status, 0);
    const env = { IMAGE, NAME: 'runtime-gateway', DIGEST: 'sha256:index' };
    const ok = run(tools, ['image-subjects'], env);
    assert.equal(ok.status, 0, ok.stderr);
    assert.equal(readFileSync(tools.env.GITHUB_OUTPUT, 'utf8'), 'amd64_digest=sha256:amd64\narm64_digest=sha256:arm64\n');

    tools.writeRegistry({
      [`${IMAGE}@sha256:other`]: { digest: 'sha256:other', manifests: platformIndex('sha256:new', 'sha256:arm64') },
    });
    const changed = run(tools, ['image-subjects'], { ...env, DIGEST: 'sha256:other' });
    assert.equal(changed.status, 1);
    assert.match(changed.stderr, /linux\/amd64: promoted sha256:new, but the SBOM describes sha256:amd64/);
  } finally {
    rmSync(tools.root, { recursive: true, force: true });
  }
});

test('check-attested names every SBOM without a Sigstore bundle', () => {
  const tools = release();
  try {
    assert.equal(run(tools, ['generate']).status, 0);
    const dir = join(tools.root, 'artifacts', 'sboms');
    for (const arch of ['amd64', 'arm64']) {
      writeFileSync(join(dir, `runtime-gateway-image-linux-${arch}.sbom.sigstore.json`), '{}');
    }
    const missing = run(tools, ['check-attested']);
    assert.equal(missing.status, 1);
    assert.match(missing.stderr, new RegExp(`without an attestation bundle.*: ${BINARY}\\.spdx\\.json$`, 'm'));

    writeFileSync(join(dir, `${BINARY}.sbom.sigstore.json`), '{}');
    assert.equal(run(tools, ['check-attested']).status, 0);
  } finally {
    rmSync(tools.root, { recursive: true, force: true });
  }
});

test('check-binary rejects a binary without embedded dependencies', () => {
  const tools = release({ binaryPurls: [] });
  try {
    const res = run(tools, ['check-binary', `artifacts/${BINARY}/${BINARY}`]);
    assert.equal(res.status, 1);
    assert.match(res.stderr, /SBOM has 0 pkg:cargo\/ packages/);
  } finally {
    rmSync(tools.root, { recursive: true, force: true });
  }
});

#!/usr/bin/env node
// Resolves and promotes the container images a wasmCloud release ships.
//
//   resolve  Read-only. Picks each image's release digest: the published
//            `X.Y.Z` digest when the tag already exists (release tags are
//            immutable), otherwise the canary `sha-<merge-sha>` digest. Writes
//            the plan to $GITHUB_OUTPUT as `plan`, which release-sboms.mjs
//            scans before anything is published.
//   promote  Retags each unpublished image from its canary digest and writes
//            `<key>_digest` for the provenance attestations.
//
// Env: IMAGE_TAG and MERGE_SHA for resolve, PLAN for promote, GITHUB_OUTPUT.
// Needs `docker buildx`.

import { spawnSync } from 'node:child_process';
import { appendFileSync } from 'node:fs';

// `suffix` is shared by the canary and release tags. `name` prefixes the SBOM
// assets; `ecosystem` is the purl type the image's application must carry.
const IMAGES = [
  { key: 'wash', image: 'ghcr.io/wasmcloud/wash', suffix: '', name: 'wash', ecosystem: 'cargo' },
  { key: 'gateway', image: 'ghcr.io/wasmcloud/runtime-gateway', suffix: '', name: 'runtime-gateway', ecosystem: 'golang' },
  { key: 'operator', image: 'ghcr.io/wasmcloud/runtime-operator', suffix: '', name: 'runtime-operator', ecosystem: 'golang' },
  { key: 'wash_all_features', image: 'ghcr.io/wasmcloud/wash', suffix: '-all-features', name: 'wash-all-features', ecosystem: 'cargo' },
];

const MISSING_RE = /manifest unknown|no such manifest|not found|MANIFEST_UNKNOWN/i;

function requireEnv(name) {
  const v = process.env[name];
  if (!v) {
    console.error(`missing required env var: ${name}`);
    process.exit(1);
  }
  return v;
}

function fail(message) {
  console.error(message);
  process.exit(1);
}

function output(key, value) {
  appendFileSync(requireEnv('GITHUB_OUTPUT'), `${key}=${value}\n`);
}

// Returns the manifest digest, or null when the registry has no such tag.
// Any other error aborts with the registry's message.
function inspectDigest(ref) {
  const res = spawnSync('docker', ['buildx', 'imagetools', 'inspect', ref, '--format', '{{json .Manifest}}'], {
    encoding: 'utf8',
  });
  if (res.status === 0) return JSON.parse(res.stdout).digest;
  const message = `${res.stderr ?? ''}${res.stdout ?? ''}`;
  if (MISSING_RE.test(message)) return null;
  fail(`inspecting ${ref} failed — refusing to promote\n${message}`);
}

function resolve() {
  const imageTag = requireEnv('IMAGE_TAG');
  const mergeSha = requireEnv('MERGE_SHA');
  const plan = IMAGES.map(({ key, image, suffix, name, ecosystem }) => {
    const tag = `${imageTag}${suffix}`;
    const published = inspectDigest(`${image}:${tag}`);
    if (published) {
      console.log(`${image}:${tag} already exists — skipping (immutable)`);
      return { key, image, tag, digest: published, promote: false, name, ecosystem };
    }
    const src = `${image}:sha-${mergeSha}${suffix}`;
    const digest = inspectDigest(src);
    if (!digest) fail(`canary image ${src} does not exist`);
    console.log(`will promote ${src} (${digest}) -> ${image}:${tag}`);
    return { key, image, tag, digest, promote: true, name, ecosystem };
  });
  output('plan', JSON.stringify(plan));
}

function promote() {
  for (const { key, image, tag, digest, promote } of JSON.parse(requireEnv('PLAN'))) {
    let released = digest;
    if (promote) {
      console.log(`promoting ${image}@${digest} -> ${image}:${tag}`);
      const res = spawnSync('docker', ['buildx', 'imagetools', 'create', '--tag', `${image}:${tag}`, `${image}@${digest}`], {
        stdio: 'inherit',
      });
      if (res.status !== 0) fail(`retagging ${image}:${tag} failed`);
      released = inspectDigest(`${image}:${tag}`);
      if (!released) fail(`${image}:${tag} is missing after the retag`);
      if (released !== digest) {
        console.warn(
          `WARN: retag digest (${released}) differs from canary source (${digest}) — same content, different manifest envelope. Attestation covers ${released}.`,
        );
      }
    }
    output(`${key}_digest`, released);
  }
}

const [command] = process.argv.slice(2);
switch (command) {
  case 'resolve':
    resolve();
    break;
  case 'promote':
    promote();
    break;
  default:
    fail(`unknown command: ${command ?? '(none)'}`);
}

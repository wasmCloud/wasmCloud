#!/usr/bin/env node
// Resolves and promotes the container images a wasmCloud release ships.
//
//   record   Save the manifest creation digest with its workflow identity.
//   resolve  Read-only. Picks each image's release digest: the
//            digest recorded by a successful canary run for the release SHA.
//            Existing release tags must match that digest. Writes
//            the plan to $GITHUB_OUTPUT as `plan`, which release-sboms.mjs
//            scans before anything is published.
//   promote  Retags each unpublished image from its canary digest and writes
//            `<key>_digest` for the provenance attestations.
//
// Env: IMAGE_TAG, MERGE_SHA and GH_REPO for resolve; PLAN for promote.
// record uses IMAGE_NAME, IMAGE_METADATA, IMAGE_RECORD, GITHUB_SHA and GITHUB_RUN_ID.
// Resolution requires digest records from successful canary workflow runs.
// Needs `docker buildx`.

import { execFileSync, spawnSync } from 'node:child_process';
import { appendFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

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
  throw new Error(message);
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

// The record is uploaded by the canary job from imagetools create metadata,
// before a mutable registry tag can be substituted during release recovery.
function record() {
  const name = requireEnv('IMAGE_NAME');
  const image = IMAGES.find((item) => item.name === name)?.image;
  if (!image) fail(`unknown release image: ${name}`);
  const metadata = JSON.parse(readFileSync(requireEnv('IMAGE_METADATA'), 'utf8'));
  const digest = metadata['containerimage.descriptor']?.digest;
  const sha = requireEnv('GITHUB_SHA');
  const runId = requireEnv('GITHUB_RUN_ID');
  if (!/^sha256:[0-9a-f]{64}$/.test(digest ?? '') || !/^[0-9a-f]{40}$/.test(sha) || !/^[1-9][0-9]*$/.test(runId)) {
    fail('invalid canary image metadata or workflow identity');
  }
  writeFileSync(requireEnv('IMAGE_RECORD'), JSON.stringify({ image, digest, sha, runId }) + '\n');
}

function resolve() {
  const imageTag = requireEnv('IMAGE_TAG');
  const mergeSha = requireEnv('MERGE_SHA');
  const repo = requireEnv('GH_REPO');
  if (!/^[0-9a-f]{40}$/.test(mergeSha)) fail('a full release commit SHA is required');
  const root = mkdtempSync(join(tmpdir(), 'release-image-records-'));
  const runs = new Map();
  try {
    const plan = IMAGES.map(({ key, image, suffix, name, ecosystem }) => {
      const workflow = key.startsWith('wash') ? 'wash' : name;
      if (!runs.has(workflow)) {
        const endpoint = `repos/${repo}/actions/workflows/${workflow}.yml/runs?head_sha=${mergeSha}&branch=main&per_page=100`;
        const pages = JSON.parse(execFileSync('gh', ['api', '--paginate', '--slurp', endpoint], { encoding: 'utf8' }));
        const run = pages.flatMap((page) => page.workflow_runs).filter((run) =>
          run.head_sha === mergeSha && run.head_branch === 'main' &&
          run.path === `.github/workflows/${workflow}.yml` &&
          ['push', 'workflow_dispatch'].includes(run.event) &&
          run.status === 'completed' && run.conclusion === 'success',
        ).sort((a, b) => b.id - a.id)[0];
        if (!run || !Number.isSafeInteger(run.id)) fail(`no successful ${workflow} canary run for ${mergeSha}`);
        runs.set(workflow, run.id);
      }
      const runId = runs.get(workflow);
      const directory = join(root, name);
      execFileSync('gh', ['run', 'download', String(runId), '--repo', repo,
        '--name', `release-image-${name}`, '--dir', directory], { stdio: 'inherit' });
      const record = JSON.parse(readFileSync(join(directory, 'image.json'), 'utf8'));
      if (record.image !== image || record.sha !== mergeSha || record.runId !== String(runId) ||
          !/^sha256:[0-9a-f]{64}$/.test(record.digest ?? '')) {
        fail(`invalid ${name} canary digest record for run ${runId}`);
      }
      const digest = record.digest;
      if (inspectDigest(`${image}@${digest}`) !== digest) fail(`verified canary ${image}@${digest} is unavailable`);
      const tag = `${imageTag}${suffix}`;
      const published = inspectDigest(`${image}:${tag}`);
      if (published && published !== digest) fail(`${image}:${tag} differs from successful canary run ${runId}`);
      console.log(`verified ${image}@${digest} from ${workflow} run ${runId}`);
      return { key, image, tag, digest, promote: !published, name, ecosystem };
    });
    output('plan', JSON.stringify(plan));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function promote() {
  for (const { key, image, tag, digest, promote } of JSON.parse(requireEnv('PLAN'))) {
    let released = digest;
    const published = inspectDigest(`${image}:${tag}`);
    if (published && published !== digest) fail(`${image}:${tag} changed since resolution; refusing to promote`);
    if (!published && !promote) fail(`${image}:${tag} disappeared since resolution`);
    if (promote && !published) {
      console.log(`promoting ${image}@${digest} -> ${image}:${tag}`);
      const res = spawnSync('docker', ['buildx', 'imagetools', 'create', '--tag', `${image}:${tag}`, `${image}@${digest}`], {
        stdio: 'inherit',
      });
      if (res.status !== 0) fail(`retagging ${image}:${tag} failed`);
      released = inspectDigest(`${image}:${tag}`);
      if (!released) fail(`${image}:${tag} is missing after the retag`);
      if (released !== digest) {
        fail(`${image}:${tag} digest ${released} differs from verified canary ${digest}`);
      }
    }
    output(`${key}_digest`, released);
  }
}

try {
  const [command] = process.argv.slice(2);
  switch (command) {
    case 'record':
      record();
      break;
    case 'resolve':
      resolve();
      break;
    case 'promote':
      promote();
      break;
    default:
      fail(`unknown command: ${command ?? '(none)'}`);
  }
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}

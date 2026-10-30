#!/usr/bin/env node
// Generates and checks the SPDX SBOMs attached to a wasmCloud release.
//
//   generate            Scans every downloaded canary binary and both
//                       platforms of every release image before anything is
//                       published. Fails on an unexpected image platform or an
//                       inventory without application dependencies.
//   image-subjects      Confirms the promoted image index still references the
//                       platform manifests scanned by `generate` and writes
//                       them to $GITHUB_OUTPUT as the SBOM attestation subjects.
//   check-attested      Fails when an SBOM has no staged Sigstore bundle, so a
//                       binary target without an attestation step cannot ship.
//   check-binary <path> Applies the binary inventory check alone, at build time.
//
// SBOMs are normalized to be byte-identical across re-runs: syft's random
// document namespace becomes the asset's release download URL, and the creation
// time becomes the release commit time.
//
// Inputs (env), for generate: PLAN (the release-tag `images` step output), TAG,
// MERGE_SHA, GITHUB_REPOSITORY, RUNNER_TEMP. image-subjects takes IMAGE, NAME,
// DIGEST, RUNNER_TEMP and GITHUB_OUTPUT. Needs `syft`, and `docker buildx` for
// images.

import { execFileSync } from 'node:child_process';
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const ARTIFACTS = './artifacts';
const SBOMS = join(ARTIFACTS, 'sboms');
const BUNDLE_SUFFIX = '.sbom.sigstore.json';
const PLATFORMS = ['linux/amd64', 'linux/arm64'];

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

function recordPath() {
  return join(requireEnv('RUNNER_TEMP'), 'release-sboms.json');
}

function scan(source, out, platform) {
  const args = ['scan', source, '-q', '-o', `spdx-json=${out}`];
  if (platform) args.push('--platform', platform);
  execFileSync('syft', args, { stdio: 'inherit' });
  return JSON.parse(readFileSync(out, 'utf8'));
}

// Every binary carries a purl for itself, so a dependency means at least two.
function requireDependencies(doc, ecosystem, label) {
  const prefix = `pkg:${ecosystem}/`;
  const count = (doc.packages ?? [])
    .flatMap((p) => p.externalRefs ?? [])
    .filter((r) => r.referenceType === 'purl' && r.referenceLocator.startsWith(prefix)).length;
  if (count < 2) {
    fail(`${label}: SBOM has ${count} ${prefix} packages; the application dependency inventory is missing`);
  }
  console.log(`${label}: ${count} ${prefix} packages`);
}

function inspectRaw(ref) {
  return JSON.parse(
    execFileSync('docker', ['buildx', 'imagetools', 'inspect', ref, '--raw'], { encoding: 'utf8' }),
  );
}

// BuildKit attestation manifests are listed as unknown/unknown.
function platformDigests(image, digest) {
  const manifests = (inspectRaw(`${image}@${digest}`).manifests ?? []).filter(
    (m) => m.platform?.os !== 'unknown',
  );
  const found = manifests.map((m) => `${m.platform?.os}/${m.platform?.architecture}`).sort();
  if (JSON.stringify(found) !== JSON.stringify(PLATFORMS)) {
    fail(`${image}@${digest}: expected platforms ${PLATFORMS.join(', ')}, found ${found.join(', ') || 'none'}`);
  }
  return Object.fromEntries(manifests.map((m) => [m.platform.architecture, m.digest]));
}

function normalize(doc, file, { repo, tag, created }) {
  doc.documentNamespace = `https://github.com/${repo}/releases/download/${tag}/${file}`;
  doc.creationInfo.created = created;
  writeFileSync(join(SBOMS, file), JSON.stringify(doc));
}

function binaryArtifacts() {
  const names = readdirSync(ARTIFACTS, { withFileTypes: true })
    .filter((d) => d.isDirectory() && d.name.startsWith('wash-') && !d.name.endsWith('.zip'))
    .map((d) => d.name)
    .sort();
  if (names.length === 0) fail(`no wash binary artifacts under ${ARTIFACTS}`);
  return names;
}

function generate() {
  const plan = JSON.parse(requireEnv('PLAN'));
  const mergeSha = requireEnv('MERGE_SHA');
  const epoch = execFileSync('git', ['show', '-s', '--format=%ct', mergeSha], { encoding: 'utf8' }).trim();
  const ctx = {
    repo: requireEnv('GITHUB_REPOSITORY'),
    tag: requireEnv('TAG'),
    created: new Date(Number(epoch) * 1000).toISOString().replace(/\.\d{3}Z$/, 'Z'),
  };
  mkdirSync(SBOMS, { recursive: true });
  const scratch = mkdtempSync(join(tmpdir(), 'release-sboms-'));

  const images = {};
  for (const { image, digest, name, ecosystem } of plan) {
    images[name] = platformDigests(image, digest);
    for (const [arch, platformDigest] of Object.entries(images[name])) {
      const file = `${name}-image-linux-${arch}.spdx.json`;
      const doc = scan(`registry:${image}@${platformDigest}`, join(scratch, file), `linux/${arch}`);
      requireDependencies(doc, ecosystem, `${image}@${platformDigest}`);
      normalize(doc, file, ctx);
    }
  }

  for (const name of binaryArtifacts()) {
    const path = join(ARTIFACTS, name, name);
    if (!existsSync(path)) fail(`artifact ${name} does not contain ${name}`);
    const file = `${name}.spdx.json`;
    const doc = scan(`file:${path}`, join(scratch, file));
    requireDependencies(doc, 'cargo', name);
    normalize(doc, file, ctx);
  }

  writeFileSync(recordPath(), JSON.stringify({ images }));
}

function imageSubjects() {
  const name = requireEnv('NAME');
  const image = requireEnv('IMAGE');
  const scanned = JSON.parse(readFileSync(recordPath(), 'utf8')).images[name];
  if (!scanned) fail(`${name}: no SBOMs were generated for this image`);
  const promoted = platformDigests(image, requireEnv('DIGEST'));
  for (const [arch, digest] of Object.entries(promoted)) {
    if (scanned[arch] !== digest) {
      fail(`${name} linux/${arch}: promoted ${digest}, but the SBOM describes ${scanned[arch]}`);
    }
    appendFileSync(requireEnv('GITHUB_OUTPUT'), `${arch}_digest=${digest}\n`);
  }
}

function checkAttested() {
  const missing = readdirSync(SBOMS)
    .filter((f) => f.endsWith('.spdx.json'))
    .filter((f) => !existsSync(join(SBOMS, f.replace(/\.spdx\.json$/, BUNDLE_SUFFIX))));
  if (missing.length > 0) {
    fail(`SBOMs without an attestation bundle (each needs a release-*-sbom step): ${missing.join(', ')}`);
  }
}

function checkBinary(path) {
  if (!path) fail('usage: release-sboms.mjs check-binary <path>');
  const scratch = mkdtempSync(join(tmpdir(), 'release-sboms-'));
  requireDependencies(scan(`file:${path}`, join(scratch, 'binary.spdx.json')), 'cargo', path);
}

const [command, arg] = process.argv.slice(2);
switch (command) {
  case 'generate':
    generate();
    break;
  case 'image-subjects':
    imageSubjects();
    break;
  case 'check-attested':
    checkAttested();
    break;
  case 'check-binary':
    checkBinary(arg);
    break;
  default:
    fail(`unknown command: ${command ?? '(none)'}`);
}

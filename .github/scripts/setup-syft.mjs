#!/usr/bin/env node
// Installs the checksum-pinned Syft CLI for this runner and adds it to PATH.
// Driven by .github/actions/setup-syft.
//
// The hashes come from syft_<version>_checksums.txt on the GitHub release;
// bump SYFT_VERSION and every hash together.
//
// Env (set by GHA): RUNNER_OS, RUNNER_ARCH, RUNNER_TEMP, GITHUB_PATH. Extracts
// with `tar`; on Windows that is System32's bsdtar, which also reads zip.

import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { appendFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const SYFT_VERSION = '1.55.0';
const BUILDS = {
  'Linux-X64': ['linux_amd64.tar.gz', 'bb65249542db5fedeeb4d3f9b018b75e7264dbfbbb10486f044581a92f90490d'],
  'Linux-ARM64': ['linux_arm64.tar.gz', '929bd163db2d7cd2255d3bcb8967208079813d7b46b0c952f7b794d8e2c7d304'],
  'macOS-X64': ['darwin_amd64.tar.gz', 'e2e5831cbabaec9a36fe65c34755c1113982c842a685edce3ce0b54a0ded8003'],
  'macOS-ARM64': ['darwin_arm64.tar.gz', '1217c258bd5eb91ef3189cf22a82e02f9cac07e4ce934350f428e93aff674625'],
  'Windows-X64': ['windows_amd64.zip', 'ada4c9b93af575f479ce80fce6629897272a2d41f300a14bc49bace020767ca2'],
};
const ATTEMPTS = 5;

function requireEnv(name) {
  const v = process.env[name];
  if (!v) {
    console.error(`missing required env var: ${name}`);
    process.exit(1);
  }
  return v;
}

async function download(url) {
  for (let attempt = 1; ; attempt++) {
    try {
      const res = await fetch(url);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      return Buffer.from(await res.arrayBuffer());
    } catch (err) {
      if (attempt === ATTEMPTS) throw new Error(`downloading ${url} failed: ${err.message}`);
      console.warn(`downloading ${url} failed (${err.message}); retrying`);
      await new Promise((r) => setTimeout(r, 2 ** attempt * 1000));
    }
  }
}

const platform = `${requireEnv('RUNNER_OS')}-${requireEnv('RUNNER_ARCH')}`;
const build = BUILDS[platform];
if (!build) {
  console.error(`no pinned Syft build for ${platform}`);
  process.exit(1);
}
const [asset, sha256] = build;
const archive = await download(
  `https://github.com/anchore/syft/releases/download/v${SYFT_VERSION}/syft_${SYFT_VERSION}_${asset}`,
);
const actual = createHash('sha256').update(archive).digest('hex');
if (actual !== sha256) {
  console.error(`syft_${SYFT_VERSION}_${asset}: sha256 ${actual}, expected ${sha256}`);
  process.exit(1);
}

const dir = join(requireEnv('RUNNER_TEMP'), 'syft');
mkdirSync(dir, { recursive: true });
const archivePath = join(dir, `syft_${asset}`);
writeFileSync(archivePath, archive);
const windows = platform.startsWith('Windows');
const tar = windows ? join(requireEnv('SystemRoot'), 'System32', 'tar.exe') : 'tar';
execFileSync(tar, ['-xf', archivePath, '-C', dir, windows ? 'syft.exe' : 'syft'], { stdio: 'inherit' });
appendFileSync(requireEnv('GITHUB_PATH'), `${dir}\n`);
console.log(`installed syft ${SYFT_VERSION} (${asset}) to ${dir}`);

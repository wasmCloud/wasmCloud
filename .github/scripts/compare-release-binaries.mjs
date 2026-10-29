#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat, mkdir, open, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const binaries = ['wash-x86_64-unknown-linux-gnu', 'runtime-operator', 'runtime-gateway'];
const [first, second, reports, ...options] = process.argv.slice(2);
const install = options.includes('--install-diffoscope');
const selected = options.filter((name) => name !== '--install-diffoscope');
if (!first || !second || !reports || selected.some((name) => !binaries.includes(name))) {
  console.error('Usage: compare-release-binaries.mjs FIRST SECOND REPORTS [BINARY...] [--install-diffoscope]');
  process.exit(2);
}

await mkdir(reports, { recursive: true });
let failed = false;
let installed = false;
for (const binary of selected.length ? selected : binaries) {
  const paths = [resolve(first, binary), resolve(second, binary)];
  try {
    const stats = await Promise.all(paths.map((path) => lstat(path)));
    if (stats.some((stat) => !stat.isFile() || stat.size === 0)) throw new Error('not a nonempty regular file');
    const hashes = await Promise.all(paths.map(sha256));
    const summary = hashes.map((hash, index) => `${hash}  ${paths[index]}\n`).join('');
    await writeFile(resolve(reports, `${binary}.sha256`), summary);
    process.stdout.write(summary);
    if (hashes[0] === hashes[1]) continue;

    failed = true;
    console.error(`::error::Non-reproducible binary: ${binary}`);
    const log = await open(resolve(reports, `${binary}.diffoscope.log`), 'w');
    try {
      if (install && !installed) {
        for (const args of [['apt-get', 'update'], ['apt-get', 'install', '--no-install-recommends', '-y', 'diffoscope-minimal']]) {
          const result = spawnSync('sudo', args, { stdio: ['ignore', log.fd, log.fd] });
          if (result.error || result.status !== 0) throw new Error(`Could not install diffoscope: ${result.error?.message || result.signal || result.status}`);
        }
        installed = true;
      }
      // Exit 1 reports differences; diagnostic failures must not hide other mismatches.
      const result = spawnSync('diffoscope', ['--text', resolve(reports, `${binary}.diff.txt`),
        '--html', resolve(reports, `${binary}.diff.html`), ...paths], { stdio: ['ignore', log.fd, log.fd] });
      if (result.error || result.status === null || result.status > 1) {
        throw new Error(`diffoscope failed: ${result.error?.message || result.signal || result.status}`);
      }
    } catch (error) {
      await log.write(`${error.message}\n`);
      console.error(`${binary}: ${error.message}; see its diagnostic log`);
    } finally {
      await log.close();
    }
  } catch (error) {
    failed = true;
    const message = `${binary}: ${error.message}`;
    console.error(message);
    await writeFile(resolve(reports, `${binary}.error.txt`), `${message}\n`);
  }
}
process.exitCode = failed ? 1 : 0;

async function sha256(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}

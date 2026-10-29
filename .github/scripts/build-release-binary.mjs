#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, realpathSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = realpathSync(fileURLToPath(new URL('../..', import.meta.url)));
const components = ['wash', 'runtime-operator', 'runtime-gateway'];
const [component, directory, ...extra] = process.argv.slice(2);
if (!directory || extra.length || ![...components, 'all'].includes(component)) {
  console.error('Usage: build-release-binary.mjs <wash|runtime-operator|runtime-gateway|all> OUTPUT_DIRECTORY');
  process.exit(2);
}

const output = resolve(directory);
mkdirSync(output, { recursive: true });
const timestamp = capture('git', ['log', '-1', '--format=%ct']);
const env = { ...process.env, SOURCE_DATE_EPOCH: timestamp, TZ: 'UTC', LC_ALL: 'C' };

try {
  for (const name of component === 'all' ? components : [component]) build(name);
  if (component === 'all') {
    writeFileSync(resolve(output, 'build-environment.json'), JSON.stringify({
      sha: capture('git', ['rev-parse', 'HEAD']),
      source_date_epoch: timestamp,
      rust: capture('rustc', ['-vV']),
      go: capture('go', ['version']),
      zig: capture('zig', ['version']),
      kernel: capture('uname', ['-a']),
    }, null, 2) + '\n');
  }
} catch (error) {
  console.error(error.message);
  process.exitCode = error.status || 1;
}

function build(name) {
  if (name === 'wash') {
    const cargoHome = resolve(process.env.CARGO_HOME || resolve(homedir(), '.cargo'));
    mkdirSync(cargoHome, { recursive: true });
    const cargo = realpathSync(cargoHome);
    const rustflags = [`--remap-path-prefix=${root}=/workspace`, `--remap-path-prefix=${cargo}=/cargo`];
    const nativeFlags = [`-ffile-prefix-map=${root}=/workspace`, `-ffile-prefix-map=${cargo}=/cargo`]
      .map((flag) => `'${flag.replaceAll("'", "'\\''")}'`).join(' ');
    const rustEnv = {
      ...env,
      CARGO_HOME: cargo,
      RUSTFLAGS: rustflags.join(' '),
      // Encoded flags preserve checkout paths containing spaces.
      CARGO_ENCODED_RUSTFLAGS: rustflags.join('\x1f'),
      CFLAGS: nativeFlags,
      CXXFLAGS: nativeFlags,
      CC_SHELL_ESCAPED_FLAGS: '1',
      CARGO_PROFILE_RELEASE_DEBUG: '0',
      CARGO_PROFILE_RELEASE_STRIP: 'symbols',
      CARGO_INCREMENTAL: '0',
    };
    execFileSync('cargo', ['zigbuild', '--locked', '--release', '--target', 'x86_64-unknown-linux-gnu.2.17', '-p', 'wash', '--bin', 'wash'], {
      cwd: root, env: rustEnv, stdio: 'inherit',
    });
    copyFileSync(resolve(root, process.env.CARGO_TARGET_DIR || 'target', 'x86_64-unknown-linux-gnu/release/wash'),
      resolve(output, 'wash-x86_64-unknown-linux-gnu'));
  } else {
    execFileSync('go', ['build', '-mod=readonly', '-trimpath', '-buildvcs=false', '-ldflags=-s -w -buildid=',
      '-o', resolve(output, name), name === 'runtime-operator' ? './cmd' : './'], {
      cwd: resolve(root, name),
      env: { ...env, CGO_ENABLED: '0', GOOS: 'linux', GOARCH: 'amd64', GOAMD64: 'v1',
        GOWORK: 'off', GOTOOLCHAIN: 'local', GOFLAGS: '' },
      stdio: 'inherit',
    });
  }
}

function capture(command, args) {
  return execFileSync(command, args, { cwd: root, encoding: 'utf8' }).trim();
}

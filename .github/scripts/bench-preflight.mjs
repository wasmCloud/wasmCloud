#!/usr/bin/env node
// Pre-flight checks for the wasmCloud bench host. Refuses to start a
// bench if the host has drifted from the baseline established by
// stage-hetzner.sh + scripts/bench/ansible/provision.yml. A drifted
// host produces meaningless numbers; better to fail fast than to
// publish them.
//
// Invoked from .github/workflows/bench{,-compare}.yml. Reads:
//
//   WASMCLOUD_BENCH_HOSTNAME       expected hostname (workflow: vars.WASMCLOUD_BENCH_HOSTNAME)
//   WASMCLOUD_BENCH_ISOLATED_CPU   override for the isolated-CPU index (default: "5")
//   CARGO_TARGET_DIR               persistent target dir (default: /var/lib/bench/target)
//
// Each invariant prints one line on success ("pre-flight: …") or a
// GitHub Actions error annotation on failure and exits non-zero.

import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, statfsSync } from 'node:fs';
import { hostname } from 'node:os';

const EXPECTED_NPROC = 6;
const EXPECTED_GOVERNOR = 'performance';
const MIN_FREE_BYTES = 5 * 1024 ** 3; // 5 GiB
const MAX_LOAD1 = 1.0;

const isolatedCpu = process.env.WASMCLOUD_BENCH_ISOLATED_CPU ?? '5';
const targetDir = process.env.CARGO_TARGET_DIR ?? '/var/lib/bench/target';

function fail(msg) {
  console.error(`::error::pre-flight: ${msg}`);
  process.exit(1);
}

function ok(msg) {
  console.log(`pre-flight: ${msg}`);
}

function readTrim(path) {
  return readFileSync(path, 'utf8').trim();
}

function runStdout(cmd, args = []) {
  return execFileSync(cmd, args, { encoding: 'utf8' }).trim();
}

// 1. WASMCLOUD_BENCH_HOSTNAME must be exported, and we must be on that host.
const expectedHostname = process.env.WASMCLOUD_BENCH_HOSTNAME;
if (!expectedHostname) {
  fail(
    'WASMCLOUD_BENCH_HOSTNAME not set ' +
      '(workflow: vars.WASMCLOUD_BENCH_HOSTNAME; local: export from 1Password)',
  );
}
const actualHostname = hostname();
if (actualHostname !== expectedHostname) {
  fail(`wrong host: ${actualHostname} (expected ${expectedHostname})`);
}
ok(`host: ${expectedHostname}`);

// 2. nproc == 6 (nosmt active).
const nprocOut = parseInt(runStdout('nproc'), 10);
if (nprocOut !== EXPECTED_NPROC) {
  fail(`expected ${EXPECTED_NPROC} online CPUs (nosmt); got ${nprocOut}`);
}
ok(`nproc: ${nprocOut}  (nosmt active)`);

// 3. cpufreq governor == "performance" on every CPU.
const governors = runStdout('sh', [
  '-c',
  'cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor',
]).split('\n');
for (const g of governors) {
  if (g !== EXPECTED_GOVERNOR) {
    fail(`scaling_governor = ${g} (expected ${EXPECTED_GOVERNOR})`);
  }
}
ok(`governor: ${EXPECTED_GOVERNOR} on every CPU`);

// 4. /sys/devices/system/cpu/isolated matches the expected isolated CPU.
//    Override via WASMCLOUD_BENCH_ISOLATED_CPU for hosts staged differently.
if (!existsSync('/sys/devices/system/cpu/isolated')) {
  fail('/sys/devices/system/cpu/isolated not readable (kernel too old?)');
}
const isolated = readTrim('/sys/devices/system/cpu/isolated');
if (isolated !== isolatedCpu) {
  fail(`isolated CPU mismatch: kernel reports '${isolated}', expected '${isolatedCpu}'`);
}
ok(`isolcpus: CPU ${isolatedCpu} reserved`);

// 5. mdraid must not be resyncing — resync I/O would skew bench numbers.
let mdstat = '';
try {
  mdstat = readFileSync('/proc/mdstat', 'utf8');
} catch {
  // No mdraid configured; nothing to check.
}
if (mdstat.includes('resync')) {
  fail('mdraid resync in progress; refusing to bench');
}
ok('mdraid: clean (no resync)');

// 6. 1-min loadavg < 1.0. The runner agent itself is the only thing on
//    this box; anything higher means something else is eating CPU.
const load1 = parseFloat(readTrim('/proc/loadavg').split(/\s+/)[0]);
if (load1 > MAX_LOAD1) {
  fail(`1-min loadavg=${load1} (something else is busy)`);
}
ok(`loadavg(1m): ${load1}`);

// 7. $CARGO_TARGET_DIR exists and is writable.
mkdirSync(targetDir, { recursive: true });
// Posix W_OK = 2. Use access via spawnSync('test -w …') to avoid pulling
// in fs.constants just to import a value we'd have to JSON-stringify.
const access = spawnSync('test', ['-w', targetDir]);
if (access.status !== 0) {
  fail(`${targetDir} not writable`);
}
ok(`cargo target dir: ${targetDir}`);

// 8. At least 5 GiB free on the target dir's mount.
const fs = statfsSync(targetDir);
const freeBytes = Number(fs.bavail) * Number(fs.bsize);
if (freeBytes < MIN_FREE_BYTES) {
  fail(`less than 5 GiB free at ${targetDir}`);
}
ok(`free space: ${Math.floor(freeBytes / 1024 ** 3)} GiB`);

// 9. rustup-managed cargo must be available — the bench pipeline sources
//    $HOME/.cargo/env in run-bench.sh; here we just confirm the binary
//    exists. (Earlier versions of provision.yml installed rustup with
//    `--default-toolchain none`, which would surface as `cargo` exiting
//    non-zero when called without a project nearby. We accept that path —
//    run-bench.sh enters the workspace where rust-toolchain.toml applies.)
const cargoBin = `${process.env.HOME}/.cargo/bin/cargo`;
if (!existsSync(cargoBin)) {
  fail(`cargo not found at ${cargoBin}`);
}
ok(`cargo: ${runStdout(cargoBin, ['--version'])}`);

ok('all checks passed');

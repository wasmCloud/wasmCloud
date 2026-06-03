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
import { hostname, loadavg, platform } from 'node:os';

const EXPECTED_NPROC = 6;
const EXPECTED_GOVERNOR = 'performance';
const MIN_FREE_BYTES = 5 * 1024 ** 3; // 5 GiB
const MAX_LOAD1 = 1.0;
// When the load is over MAX_LOAD1 at pre-flight it's almost always the
// decaying tail of the previous bench (the release matrix runs them
// back-to-back on this one host), not a rogue process. Poll for up to
// LOAD_SETTLE_SECS. We typically see this fall off over ~60s.
const LOAD_SETTLE_SECS = 240;
const LOAD_POLL_SECS = 5;

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

// Block the synchronous pre-flight for `ms` without spawning a subprocess.
function sleepSync(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

function readLoad1() {
  return loadavg()[0];
}

// Parse a Linux CPU mask string (`/sys/devices/system/cpu/{online,offline,
// isolated,…}` format) into an integer count. Accepts `0-5`, `0,2-5`,
// `0,2,4-7`, the empty string. Used for the online-CPU assertion below.
function countCpuMask(mask) {
  if (!mask) return 0;
  let total = 0;
  for (const part of mask.split(',')) {
    const [lo, hi] = part.split('-').map(Number);
    total += hi === undefined ? 1 : hi - lo + 1;
  }
  return total;
}

// 0. Linux only. Every check below reads /proc or /sys, and os.loadavg()
//    returns [0, 0, 0] on Windows — which would silently sail past the
//    load guard. Refuse outright rather than bench on a bogus baseline.
if (platform() !== 'linux') {
  fail(`unsupported platform '${platform()}'; the bench host must be Linux`);
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

// 2. /sys/.../cpu/online == 6 cores (nosmt collapsed 12 SMT → 6 physical).
//    None of the `nproc` variants are right here:
//      - bare `nproc` honors `sched_getaffinity`, which systemd narrows
//        to "online minus isolcpus" for every service unit it manages
//        (returns 5 inside the actions.runner service even though the
//        machine has 6 cores online).
//      - `nproc --all` reads `/sys/devices/system/cpu/possible`, which
//        is the kernel's max-CPU administrative cap — a function of
//        `CONFIG_NR_CPUS` in the kernel build, not the hardware. The
//        Ubuntu 6.8.0-117 kernel package reports 32 on a 6-core box.
//    Sysfs `online` is the only one that tracks what's actually online,
//    which is the signal we want for "did nosmt take effect". The
//    isolcpus assertion (#4 below) covers the orthogonal concern of
//    "is one core reserved".
const onlineMask = readTrim('/sys/devices/system/cpu/online');
const onlineCount = countCpuMask(onlineMask);
if (onlineCount !== EXPECTED_NPROC) {
  fail(
    `expected ${EXPECTED_NPROC} online CPUs (nosmt); ` +
      `got ${onlineCount} (online mask: '${onlineMask}')`,
  );
}
ok(`online CPUs: ${onlineCount}  (mask '${onlineMask}'; nosmt active)`);

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

// 6. 1-min loadavg must be under MAX_LOAD1 before the bench starts.
//    Anything higher means CPU is being eaten and the numbers would be
//    noisy. Because the release matrix runs benches back-to-back on this
//    one host, a bench can launch into the previous bench's decaying load
//    tail, so wait up to LOAD_SETTLE_SECS for load to drop rather than
//    failing on the tail.
let load1 = readLoad1();
if (load1 > MAX_LOAD1) {
  ok(`loadavg(1m)=${load1} > ${MAX_LOAD1}; waiting up to ${LOAD_SETTLE_SECS}s to settle`);
  const deadline = Date.now() + LOAD_SETTLE_SECS * 1000;
  while (load1 > MAX_LOAD1 && Date.now() < deadline) {
    sleepSync(LOAD_POLL_SECS * 1000);
    load1 = readLoad1();
  }
  if (load1 > MAX_LOAD1) {
    fail(`1-min loadavg=${load1} after ${LOAD_SETTLE_SECS}s settle wait (something else is busy)`);
  }
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

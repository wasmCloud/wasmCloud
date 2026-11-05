#!/usr/bin/env node
// Pre-flight checks for a gpubench box (scripts/gpubench/provision/). Refuses
// to start scripts/gpubench/run.sh on a box that can't produce numbers worth
// keeping: the wrong host, no usable GPU, missing tools, too little disk, or
// another process already on the GPU.
//
// Invoked from .github/workflows/gpubench-run.yml in the checkout. Reads:
//
//   GPUBENCH_BOX                the box label the job's runner carries, e.g. cuda-l4
//                               (gpubench-boxes.mjs)
//   GPUBENCH_REPO_VARS          toJSON(vars); its WASMCLOUD_BENCH_<LABEL>_HOSTNAME
//                               (bench-host.mjs hostnameVar) is the box's expected
//                               hostname. An env var of that name wins.
//   GPUBENCH_CACHE              build and model cache (default ~/.cache/wasmcloud-gpubench)
//   GPUBENCH_MODEL_DIR          model cache (default $GPUBENCH_CACHE/models)
//   GPUBENCH_GPU                cuda: the GPU index the benches use (default 0)
//   GPUBENCH_CUDA_ARCH          cuda: must match that GPU's compute capability
//   GPUBENCH_SMOKE              "1": only the smoke model needs disk
//
// Each check prints one line on success ("pre-flight: …") or a GitHub Actions
// error annotation on failure and exits non-zero.
//
// Untested on the target hardware: neither box exists yet.

import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, statSync, statfsSync } from 'node:fs';
import { arch, cpus, homedir, hostname, loadavg, platform } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { parseBox } from './gpubench-boxes.mjs';

const GIB = 1024 ** 3;
// llama.cpp built three ways, wgpu-native's cargo target, and wasi-sdk.
const BUILD_HEADROOM = 10 * GIB;
// The workspace's own target/: bench-gpu (wash-runtime, wgpu) and bench-tools.
const WORKSPACE_HEADROOM = 10 * GIB;
// Device memory another process may hold before the serverless numbers,
// which are measured over the idle baseline, stop meaning anything.
const MAX_IDLE_GPU_MIB = 1024;
const SMOKE_MODEL = 'qwen2.5-0.5b-q4km';
const SETTLE_SECS = 240;
const POLL_SECS = 5;
// Processes a gpubench run leaves behind when it's cancelled or dies; the
// cleanup step in gpubench-run.yml kills the same set.
const LEFTOVER = /llama-bench|llama-simple|bench-gpu|sample-gpu\.sh/;

const repoRoot = fileURLToPath(new URL('../../', import.meta.url));
const box = process.env.GPUBENCH_BOX ?? '';
const gpu = process.env.GPUBENCH_GPU ?? '0';
const cache = process.env.GPUBENCH_CACHE || join(homedir(), '.cache', 'wasmcloud-gpubench');
const modelDir = process.env.GPUBENCH_MODEL_DIR || join(cache, 'models');
const smoke = process.env.GPUBENCH_SMOKE === '1';

function fail(msg) {
  console.error(`::error::pre-flight: ${msg}`);
  process.exit(1);
}

function ok(msg) {
  console.log(`pre-flight: ${msg}`);
}

function run(cmd, args = []) {
  return execFileSync(cmd, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

function tryRun(cmd, args = []) {
  const res = spawnSync(cmd, args, { encoding: 'utf8' });
  return res.status === 0 ? res.stdout.trim() : null;
}

function onPath(tool) {
  return spawnSync('sh', ['-c', `command -v ${tool}`], { stdio: 'ignore' }).status === 0;
}

function sleepSync(ms) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

// Poll `probe` until it returns null (settled) or SETTLE_SECS pass; returns
// the last non-null reason, or null.
function settle(what, probe) {
  let reason = probe();
  if (reason === null) return null;
  ok(`${what}: ${reason}; waiting up to ${SETTLE_SECS}s to settle`);
  const deadline = Date.now() + SETTLE_SECS * 1000;
  while (reason !== null && Date.now() < deadline) {
    sleepSync(POLL_SECS * 1000);
    reason = probe();
  }
  return reason;
}

function freeBytes(path) {
  const fs = statfsSync(path);
  return Number(fs.bavail) * Number(fs.bsize);
}

function gib(bytes) {
  return `${(bytes / GIB).toFixed(1)} GiB`;
}

// 1. The box, and the platform its kind implies.
let parsed;
try {
  parsed = parseBox(box);
} catch (e) {
  fail(`GPUBENCH_BOX: ${e.message}`);
}
const { kind, hostnameVar: hostVar } = parsed;
const want = { cuda: ['linux', 'x64'], metal: ['darwin', 'arm64'] }[kind];
if (platform() !== want[0] || arch() !== want[1]) {
  fail(`box ${box} must be ${want.join('/')}, this is ${platform()}/${arch()}`);
}
ok(`box: ${box}, kind ${kind} (${platform()}/${arch()})`);

// 2. The expected host. The runner label already picks the box; this catches
//    a runner registered on the wrong machine.
const repoVars = JSON.parse(process.env.GPUBENCH_REPO_VARS || '{}');
const expectedHostname = process.env[hostVar] || repoVars[hostVar];
if (!expectedHostname) fail(`repo variable ${hostVar} not set (install-runner.sh prints its value)`);
// macOS reports `<name>.local` until a HostName is set; Ansible's facts don't.
const actualHostname = hostname().replace(/\.local$/, '');
if (actualHostname !== expectedHostname) {
  fail(`wrong host: ${actualHostname} (expected ${expectedHostname})`);
}
ok(`host: ${expectedHostname}`);

// 3. Tools run.sh and its build scripts call.
const tools = ['git', 'curl', 'jq', 'cmake', 'ninja', 'perl', 'tar', 'zstd'];
if (kind === 'cuda') tools.push('nvidia-smi', 'nvcc', 'vulkaninfo', 'sha256sum');
if (kind === 'metal') tools.push('xcrun', 'system_profiler', 'ioreg', 'shasum', 'xattr', 'sysctl');
const missing = tools.filter((t) => !onPath(t));
if (missing.length > 0) {
  fail(`not on PATH: ${missing.join(', ')} (scripts/gpubench/provision/provision-${kind}.yml installs them)`);
}
const cargoBin = join(homedir(), '.cargo', 'bin', 'cargo');
if (!existsSync(cargoBin)) fail(`cargo not found at ${cargoBin}`);
ok(`tools: ${tools.join(' ')}`);
ok(`cmake: ${run('cmake', ['--version']).split('\n')[0]}`);
ok(`cargo: ${run(cargoBin, ['--version'])}`);
// The Metal build uses the SDK's frameworks; the GPU steps are plain clang.
if (kind === 'metal') {
  const sdk = tryRun('xcrun', ['--show-sdk-path']);
  if (!sdk) fail('xcrun --show-sdk-path failed (install the Xcode Command Line Tools: xcode-select --install)');
  ok(`macOS SDK: ${sdk}`);
}
// macOS ships bash 3.2; the scripts rely on bash 4+ (provision-metal.yml
// installs Homebrew's bash ahead of it on the runner's PATH).
const bashMajor = Number((tryRun('bash', ['-c', 'echo ${BASH_VERSINFO[0]}']) ?? '0'));
if (bashMajor < 4) fail(`bash on PATH is version ${bashMajor}; scripts/gpubench needs bash 4 or later`);
ok(`bash: ${bashMajor}.x`);

// 4. The GPU, through the same tools the benches use.
if (kind === 'cuda') {
  const row = tryRun('nvidia-smi', [
    '-i', gpu, '--query-gpu=name,driver_version,compute_cap,persistence_mode,memory.total',
    '--format=csv,noheader,nounits',
  ]);
  if (!row) fail(`nvidia-smi can't see GPU ${gpu}`);
  const [name, driver, computeCap, persistence, totalMib] = row.split(',').map((s) => s.trim());
  ok(`GPU ${gpu}: ${name}, driver ${driver}, ${totalMib} MiB`);

  // llama.cpp's CUDA build must run on this driver.
  const driverCuda = run('nvidia-smi').match(/CUDA Version:\s*([0-9]+\.[0-9]+)/)?.[1];
  const nvcc = run('nvcc', ['--version']).match(/release ([0-9]+\.[0-9]+)/)?.[1];
  if (!driverCuda || !nvcc || compareVersions(driverCuda, nvcc) < 0) {
    fail(`nvcc is CUDA ${nvcc ?? '?'}, newer than the driver supports (${driverCuda ?? '?'})`);
  }
  ok(`CUDA: driver ${driverCuda}, nvcc ${nvcc}`);

  // Without persistence mode the driver tears down between processes, and
  // every cold start pays for bringing it back up.
  if (persistence !== 'Enabled') fail(`persistence mode is ${persistence} (systemctl enable --now nvidia-persistenced)`);
  ok('persistence mode: Enabled');

  const archWant = computeCap.replace('.', '');
  if (process.env.GPUBENCH_CUDA_ARCH !== archWant) {
    fail(`GPUBENCH_CUDA_ARCH='${process.env.GPUBENCH_CUDA_ARCH ?? ''}', GPU ${gpu} is compute ${computeCap} ` +
      `(set GPUBENCH_CUDA_ARCH=${archWant} in the runner's .env)`);
  }
  ok(`GPUBENCH_CUDA_ARCH: ${archWant}`);

  // wgpu reaches the GPU through Vulkan; without the NVIDIA ICD it falls back
  // to a CPU implementation, or to nothing.
  const vk = tryRun('vulkaninfo', ['--summary']) ?? '';
  if (!/vendorID\s*=\s*0x10de/i.test(vk)) {
    fail('vulkaninfo lists no NVIDIA device (is the driver\'s Vulkan ICD, libnvidia-gl, installed?)');
  }
  ok('vulkan: NVIDIA device present');

  const busy = settle('GPU busy', () => {
    const apps = (tryRun('nvidia-smi', [
      '-i', gpu, '--query-compute-apps=pid,process_name,used_memory', '--format=csv,noheader,nounits',
    ]) ?? '').split('\n').filter(Boolean);
    if (apps.length > 0) return `compute processes: ${apps.join('; ')}`;
    const used = Number(tryRun('nvidia-smi', ['-i', gpu, '--query-gpu=memory.used', '--format=csv,noheader,nounits']));
    if (used > MAX_IDLE_GPU_MIB) return `${used} MiB in use`;
    return null;
  });
  if (busy) fail(`GPU ${gpu} still busy after ${SETTLE_SECS}s: ${busy}`);
  ok(`GPU ${gpu}: idle`);
} else {
  const report = JSON.parse(run('system_profiler', ['-json', 'SPDisplaysDataType']));
  const metal = (report.SPDisplaysDataType ?? []).find((d) => d.spdisplays_mtlgpufamilysupport);
  if (!metal) fail('system_profiler lists no Metal GPU');
  const family = metal.spdisplays_mtlgpufamilysupport.replace(/^spdisplays_/, '');
  ok(`GPU: ${metal.sppci_model ?? metal._name}, ${family}${metal.sppci_cores ? `, ${metal.sppci_cores} cores` : ''}`);
  // sample-gpu.sh reads this counter; without it the memory rows are zeros.
  const ioreg = tryRun('ioreg', ['-r', '-d', '1', '-c', 'IOAccelerator']) ?? '';
  if (!/"In use system memory"=[0-9]+/.test(ioreg)) fail('IOAccelerator has no "In use system memory" counter');
  ok('IOAccelerator memory counter present');
}

// 5. Nothing left over from an earlier run.
const leftovers = settle('leftover bench processes', () => {
  const procs = run('ps', ['-A', '-o', 'pid=,command='])
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => {
      const pid = Number(l.split(/\s+/, 1)[0]);
      return pid !== process.pid && pid !== process.ppid && LEFTOVER.test(l.slice(String(pid).length));
    });
  return procs.length > 0 ? procs.join('; ') : null;
});
if (leftovers) fail(`processes from an earlier run still alive: ${leftovers}`);
ok('no leftover bench processes');

// 6. CPU load: the builds are CPU-bound, and the wasm step runs on one thread.
//    Half the cores, since an idle macOS desktop sits at a load of 2 to 4.
const maxLoad = Math.max(2, cpus().length / 2);
const load = settle('load', () => (loadavg()[0] > maxLoad ? `loadavg(1m)=${loadavg()[0]} > ${maxLoad}` : null));
if (load) fail(`${load} after ${SETTLE_SECS}s (something else is busy)`);
ok(`loadavg(1m): ${loadavg()[0]}`);

// 7. Disk: room for the models not yet cached, the builds, and the workspace.
mkdirSync(cache, { recursive: true });
mkdirSync(modelDir, { recursive: true });
const models = JSON.parse(readFileSync(join(repoRoot, 'scripts/gpubench/models.json'), 'utf8'));
const wanted = smoke ? [SMOKE_MODEL] : Object.keys(models);
let modelBytes = 0;
for (const name of wanted) {
  const m = models[name];
  if (!m) fail(`no model '${name}' in scripts/gpubench/models.json`);
  const path = join(modelDir, m.file);
  if (!existsSync(path) || statSync(path).size !== m.bytes) modelBytes += m.bytes;
}
const cacheNeed = modelBytes + BUILD_HEADROOM;
const cacheFree = freeBytes(cache);
if (cacheFree < cacheNeed) {
  fail(`${gib(cacheFree)} free at ${cache}; need ${gib(cacheNeed)} (${gib(modelBytes)} of models to fetch)`);
}
ok(`cache: ${gib(cacheFree)} free at ${cache} (need ${gib(cacheNeed)})`);
const workspaceFree = freeBytes(repoRoot);
if (workspaceFree < WORKSPACE_HEADROOM) {
  fail(`${gib(workspaceFree)} free at ${repoRoot}; need ${gib(WORKSPACE_HEADROOM)}`);
}
ok(`workspace: ${gib(workspaceFree)} free`);

ok('all checks passed');

function compareVersions(a, b) {
  const [pa, pb] = [a, b].map((v) => v.split('.').map(Number));
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

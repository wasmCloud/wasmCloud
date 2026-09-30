// Every knob a scenario reads, from `k6 run -e KEY=value` (or the environment).
// run.sh sets all of them; a bare `k6 run` gets these defaults.

function env(key, fallback) {
  const v = __ENV[key];
  return v === undefined || v === '' ? fallback : v;
}

function int(key, fallback) {
  const n = parseInt(env(key, String(fallback)), 10);
  if (Number.isNaN(n)) throw new Error(`${key} must be an integer`);
  return n;
}

// "90s", "3m", "1h" or a bare number of seconds.
export function seconds(value) {
  const m = /^(\d+)(s|m|h)?$/.exec(String(value).trim());
  if (!m) throw new Error(`unparseable duration: ${value}`);
  return parseInt(m[1], 10) * { s: 1, m: 60, h: 3600 }[m[2] || 's'];
}

export const cfg = {
  targetUrl: env('TARGET_URL', 'http://localhost:30950'),
  // Empty means the scenario's own default, which matches its manifest.
  hostHeader: env('HOST_HEADER', ''),
  profile: env('PROFILE', 'constant'),
  rate: int('RATE', 1000),
  duration: seconds(env('DURATION', '2m')),
  warmup: seconds(env('WARMUP', '30s')),
  sloP99Ms: int('SLO_P99_MS', 250),
  // Short enough that an unreachable endpoint shows up as errors, not as
  // VUs parked on k6's 60 s default and every iteration dropped.
  requestTimeout: env('REQUEST_TIMEOUT', '10s'),
  // stress: one constant-rate step per entry; the highest step that holds the
  // SLO with <1% errors and no dropped iterations is max_sustainable_rps.
  stressRates: env('STRESS_RATES', '500,1000,2000,3000,4000,6000,8000,10000,12000,15000')
    .split(',')
    .map((r) => parseInt(r, 10)),
  stressStep: seconds(env('STRESS_STEP', '30s')),
  spikeFactor: int('SPIKE_FACTOR', 10),
  preVUs: int('PRE_VUS', 0),
  maxVUs: int('MAX_VUS', 0),
  // many-workloads: requests rotate over this many Host headers.
  workloads: int('WORKLOADS', 1),
  outDir: env('OUT_DIR', '.'),
};

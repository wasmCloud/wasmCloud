// Load profiles as k6 `scenarios` + `thresholds`. Every profile starts with a
// `warmup` scenario whose samples are never reported; everything bench-tools
// reads is a per-scenario submetric (`{scenario:<name>}`).
//
// All executors are open-model (arrival-rate): a slow system can't slow the
// offered load down, so latency isn't hidden by coordinated omission.

import { cfg } from './config.js';

function arrival(rate, duration, startTime) {
  return {
    executor: 'constant-arrival-rate',
    rate,
    timeUnit: '1s',
    duration: `${duration}s`,
    startTime: `${startTime}s`,
    // Pre-allocate for ~50 ms of latency: a VU k6 has to create mid-run
    // delays its iteration, which it counts as dropped and marks the run
    // generator-saturated. maxVUs covers ~500 ms before that happens anyway.
    preAllocatedVUs: cfg.preVUs || Math.max(50, Math.ceil(rate * 0.05)),
    maxVUs: cfg.maxVUs || Math.max(200, Math.ceil(rate * 0.5)),
    gracefulStop: '5s',
  };
}

// A threshold on a submetric is what makes k6 report it in the summary, so
// scenarios bench-tools reads but doesn't gate get always-true thresholds.
function report(name, thresholds) {
  thresholds[`http_reqs{scenario:${name}}`] = ['count>=0'];
  thresholds[`dropped_iterations{scenario:${name}}`] = ['count>=0'];
  thresholds[`http_req_failed{scenario:${name}}`] ??= ['rate>=0'];
  thresholds[`http_req_duration{scenario:${name}}`] ??= ['p(99)>=0'];
}

function constant() {
  const scenarios = {
    warmup: arrival(cfg.rate, cfg.warmup, 0),
    measure: arrival(cfg.rate, cfg.duration, cfg.warmup),
  };
  const thresholds = {
    'http_req_failed{scenario:measure}': ['rate<0.01'],
    'http_req_duration{scenario:measure}': [`p(99)<${cfg.sloP99Ms}`],
  };
  report('measure', thresholds);
  return { scenarios, thresholds };
}

function stress() {
  const scenarios = { warmup: arrival(cfg.stressRates[0], cfg.warmup, 0) };
  const thresholds = {};
  let start = cfg.warmup;
  for (const rate of cfg.stressRates) {
    const name = `step_${rate}`;
    scenarios[name] = arrival(rate, cfg.stressStep, start);
    // Stop climbing once the system is plainly past its knee.
    thresholds[`http_req_failed{scenario:${name}}`] = [
      { threshold: 'rate<0.5', abortOnFail: true, delayAbortEval: '10s' },
    ];
    report(name, thresholds);
    start += cfg.stressStep;
  }
  return { scenarios, thresholds };
}

function spike() {
  const base = cfg.rate;
  const hold = cfg.duration;
  const scenarios = {
    warmup: arrival(base, cfg.warmup, 0),
    spike_pre: arrival(base, hold, cfg.warmup),
    spike_peak: arrival(base * cfg.spikeFactor, hold, cfg.warmup + hold),
    spike_post: arrival(base, hold, cfg.warmup + 2 * hold),
  };
  const thresholds = {
    'http_req_failed{scenario:spike_post}': ['rate<0.01'],
    'http_req_duration{scenario:spike_post}': [`p(99)<${cfg.sloP99Ms}`],
  };
  for (const name of ['spike_pre', 'spike_peak', 'spike_post']) report(name, thresholds);
  return { scenarios, thresholds };
}

const PROFILES = { constant, stress, spike };

export function profile(name = cfg.profile) {
  const build = PROFILES[name];
  if (!build) throw new Error(`unknown PROFILE ${name}; one of ${Object.keys(PROFILES)}`);
  return build();
}

// The options every scenario script exports, before its own tweaks.
export function options(extra = {}) {
  const { scenarios, thresholds } = profile();
  return {
    discardResponseBodies: true,
    summaryTrendStats: ['avg', 'min', 'p(50)', 'p(90)', 'p(95)', 'p(99)', 'p(99.9)', 'max'],
    scenarios,
    thresholds,
    ...extra,
  };
}

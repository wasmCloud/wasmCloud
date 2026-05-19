// k6 bench: sustained-rate HTTP throughput against the hello-world
// component running on the runtime-operator-managed wash host pod.
//
// Reads (required):
//   K6_TARGET_URL    full URL of the workload Service (e.g. http://hello.bench:9191/)
//
// Reads (optional):
//   K6_RPS           target req/s for the constant_rps scenario (default: 200)
//   K6_DURATION      duration (default: "30s")
//   K6_VUS           pre-allocated VUs (default: max(50, K6_RPS / 4))
//   K6_MAX_VUS       max VUs k6 may spin up if it can't hit RPS (default: K6_VUS * 4)
//
// Writes (consumed by bench-tools):
//   ./summary.json   k6 end-of-test summary; bench-tools k6 module ingests this.
//
// The scenario name (`constant_rps`) lands in `group` in the JSONL output.
// The `<rps>rps` tag on the request lands in `param`. Latency is reported
// per request, so percentiles are computed across the steady-state window
// minus k6's natural ramp at start/stop (k6 trims those automatically when
// using `executor: constant-arrival-rate`).

import http from 'k6/http';
import { check } from 'k6';

const TARGET_URL = __ENV.K6_TARGET_URL;
if (!TARGET_URL) {
  throw new Error('K6_TARGET_URL not set');
}

const RPS = Number(__ENV.K6_RPS ?? 200);
const DURATION = __ENV.K6_DURATION ?? '30s';
const VUS = Number(__ENV.K6_VUS ?? Math.max(50, Math.ceil(RPS / 4)));
const MAX_VUS = Number(__ENV.K6_MAX_VUS ?? VUS * 4);

export const options = {
  // One scenario, one stage: constant arrival rate. Keeps the
  // (group, param) cardinality at exactly one row per metric per run,
  // matching how criterion benches like http_invoke produce one
  // (group, param) per parameterized input.
  scenarios: {
    constant_rps: {
      executor: 'constant-arrival-rate',
      rate: RPS,
      timeUnit: '1s',
      duration: DURATION,
      preAllocatedVUs: VUS,
      maxVUs: MAX_VUS,
      // Tag every iteration so the summary parser can attribute
      // metrics to (`constant_rps`, `<rps>rps`).
      tags: { stage: `${RPS}rps` },
    },
  },
  // No thresholds: a slow run shouldn't fail the workflow — the trend
  // dashboard is the source of truth for "did this regress". A run that
  // can't reach the target RPS still produces a row whose req_per_s is
  // visibly low.
  thresholds: {},
  // Trim the absolute first 1 s — that's where k6 spins up VUs and the
  // first connection is being negotiated. 30 s scenarios are mostly
  // steady-state, so this only meaningfully affects short runs but is
  // always accurate.
  summaryTrendStats: ['avg', 'min', 'med', 'max', 'p(50)', 'p(95)', 'p(99)'],
};

export default function () {
  const res = http.get(TARGET_URL);
  check(res, {
    'status is 200': (r) => r.status === 200,
  });
}

// k6 v0.50+: write the end-of-test summary to summary.json (machine
// readable) instead of the default stdout text dump (human readable).
// stdout still gets a short banner so the bench log isn't empty.
export function handleSummary(data) {
  return {
    stdout: `k6 done — see summary.json (scenario=constant_rps stage=${RPS}rps)\n`,
    'summary.json': JSON.stringify(data),
  };
}

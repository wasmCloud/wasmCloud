// handleSummary for every scenario: writes $OUT_DIR/summary.json for
// `bench-tools k6` and prints k6's usual end-of-test text.
//
// summary.json wraps k6's own summary with what bench-tools can't recover from
// it: each scenario's offered rate and length, so rps is computed over the
// measured window rather than the whole test.

import { textSummary } from 'https://jslib.k6.io/k6-summary/0.1.0/index.js';
import { cfg } from './config.js';
import { profile } from './profiles.js';

export function summarize(scenarioName) {
  return function handleSummary(data) {
    const { scenarios } = profile();
    const shape = {};
    for (const [name, s] of Object.entries(scenarios)) {
      shape[name] = { rate: s.rate, duration_s: parseInt(s.duration, 10) };
    }
    const out = {
      schema: 1,
      scenario: scenarioName,
      profile: cfg.profile,
      slo_p99_ms: cfg.sloP99Ms,
      scenarios: shape,
      k6: data,
    };
    return {
      [`${cfg.outDir}/summary.json`]: JSON.stringify(out, null, 2),
      stdout: textSummary(data, { indent: ' ', enableColors: true }),
    };
  };
}

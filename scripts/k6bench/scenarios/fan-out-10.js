// A `relay` that calls 10 co-located `hello` workloads concurrently per
// request, by local routing or Service DNS (`run.sh --routing`). Measures
// latency amplification across an inter-workload call graph.

import http from 'k6/http';
import { cfg } from '../lib/config.js';
import { options as baseOptions } from '../lib/profiles.js';
import { summarize } from '../lib/summary.js';

export const options = baseOptions();
export const handleSummary = summarize('fan-out-10');

const url = `${cfg.targetUrl}/`;
const params = {
  headers: { Host: cfg.hostHeader || 'fanout.k6bench' },
  timeout: cfg.requestTimeout,
};

export default function () {
  http.get(url, params);
}

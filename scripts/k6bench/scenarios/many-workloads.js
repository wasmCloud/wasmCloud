// WORKLOADS `hello` WorkloadDeployments (hello-0.k6bench … hello-N.k6bench) on
// the same hosts; requests rotate over their Host headers. Measures routing
// and per-workload cost as the workload count grows.

import http from 'k6/http';
import exec from 'k6/execution';
import { cfg } from '../lib/config.js';
import { options as baseOptions } from '../lib/profiles.js';
import { summarize } from '../lib/summary.js';

export const options = baseOptions();
export const handleSummary = summarize('many-workloads');

const url = `${cfg.targetUrl}/`;
const params = [];
for (let i = 0; i < cfg.workloads; i++) {
  params.push({ headers: { Host: `hello-${i}.k6bench` }, timeout: cfg.requestTimeout });
}

export default function () {
  http.get(url, params[exec.scenario.iterationInTest % params.length]);
}

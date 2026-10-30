// One `hello` workload: the system-level counterpart of the http_invoke
// criterion bench.
//
//   k6 run -e TARGET_URL=http://localhost:30950 -e HOST_HEADER=hello.k6bench \
//     scripts/k6bench/scenarios/http-hello.js

import http from 'k6/http';
import { cfg } from '../lib/config.js';
import { options as baseOptions } from '../lib/profiles.js';
import { summarize } from '../lib/summary.js';

export const options = baseOptions();
export const handleSummary = summarize('http-hello');

const url = `${cfg.targetUrl}/`;
const params = {
  headers: { Host: cfg.hostHeader || 'hello.k6bench' },
  timeout: cfg.requestTimeout,
};

export default function () {
  http.get(url, params);
}

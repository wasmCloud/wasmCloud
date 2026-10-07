// Five `relay` hops in a line (link-0 → … → link-4), each by local routing or
// Service DNS (`run.sh --routing`). Per-hop overhead ≈ (chain p50 − http-hello p50) / 4.

import http from 'k6/http';
import { cfg } from '../lib/config.js';
import { options as baseOptions } from '../lib/profiles.js';
import { summarize } from '../lib/summary.js';

export const options = baseOptions();
export const handleSummary = summarize('chain-5');

const url = `${cfg.targetUrl}/`;
const params = {
  headers: { Host: cfg.hostHeader || 'chain.k6bench' },
  timeout: cfg.requestTimeout,
};

export default function () {
  http.get(url, params);
}

// Rendering a chart and reading the manifests back out, for the checks that
// assert something about what a chart actually produces.
//
// Needs `helm` and `yq` on PATH; both ship on the GitHub-hosted runners.

import { execFileSync, spawnSync } from 'node:child_process';

const MAX_OUTPUT = 64 * 1024 * 1024;

// Renders a chart, optionally with `--set` overrides (`key=value` strings).
export function helmTemplate(chartDir, releaseName, sets = []) {
  const args = ['template', releaseName, chartDir];
  for (const s of sets) {
    args.push('--set', s);
  }
  return execFileSync('helm', args, { encoding: 'utf8', maxBuffer: MAX_OUTPUT });
}

// Parses a multi-doc YAML stream into objects.
export function yamlToDocs(yamlText) {
  // yq -I 0 emits one compact JSON object per YAML doc, one per line.
  const r = spawnSync('yq', ['-o', 'json', '-I', '0'], {
    input: yamlText,
    encoding: 'utf8',
    maxBuffer: MAX_OUTPUT,
  });
  if (r.status !== 0) {
    throw new Error(`yq failed (exit ${r.status}): ${r.stderr.trim()}`);
  }
  // Empty YAML docs (`---` with no body) round-trip through yq as `null`;
  // drop them so downstream code can assume every doc is an object.
  return r.stdout
    .split('\n')
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line))
    .filter((doc) => doc != null);
}

// Fake `docker`, `syft` and `git` CLIs for the release script tests.
//
// The registry is a JSON file mapping `image:tag` and `image@digest` to
// `{ digest, manifests }`; a value of `"error"` makes inspect fail with an
// unexpected registry error. syft reads purls per scan source from another
// JSON file and writes SPDX with a fresh namespace and time, as syft does.

import { chmodSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';

const DOCKER = `#!/usr/bin/env node
import { readFileSync, writeFileSync } from 'node:fs';
const path = process.env.FAKE_REGISTRY;
const registry = JSON.parse(readFileSync(path, 'utf8'));
const args = process.argv.slice(2);
// \`image@digest\` also matches any tag of that image pointing at the digest.
const lookup = (ref) => {
  if (registry[ref] || !ref.includes('@')) return registry[ref];
  const [image, digest] = ref.split('@');
  return Object.entries(registry).find(([k, v]) => k.startsWith(image + ':') && v.digest === digest)?.[1];
};
if (args[0] !== 'buildx' || args[1] !== 'imagetools') process.exit(2);
if (args[2] === 'create') {
  const tag = args[args.indexOf('--tag') + 1];
  registry[tag] = lookup(args.at(-1));
  writeFileSync(path, JSON.stringify(registry));
  process.exit(0);
}
const entry = lookup(args[3]);
if (entry === 'error') { console.error('ERROR: unauthorized: authentication required'); process.exit(1); }
if (!entry) { console.error('ERROR: ' + args[3] + ': not found'); process.exit(1); }
console.log(JSON.stringify(args.includes('--raw') ? { manifests: entry.manifests } : { digest: entry.digest }));
`;

const SYFT = `#!/usr/bin/env node
import { randomUUID } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
const args = process.argv.slice(2);
const purls = JSON.parse(readFileSync(process.env.FAKE_SBOMS, 'utf8'))[args[1]] ?? [];
const out = args.find((a) => a.startsWith('spdx-json=')).slice('spdx-json='.length);
writeFileSync(out, JSON.stringify({
  spdxVersion: 'SPDX-2.3',
  documentNamespace: 'https://anchore.com/syft/' + randomUUID(),
  creationInfo: { created: new Date().toISOString() },
  packages: purls.map((p) => ({ externalRefs: [{ referenceType: 'purl', referenceLocator: p }] })),
}));
`;

const GIT = `#!/usr/bin/env node
console.log('1700000000');
`;

export function platformIndex(amd64, arm64, extra = []) {
  return [
    { digest: amd64, platform: { os: 'linux', architecture: 'amd64' } },
    { digest: arm64, platform: { os: 'linux', architecture: 'arm64' } },
    { digest: 'sha256:att', platform: { os: 'unknown', architecture: 'unknown' } },
    ...extra,
  ];
}

// Returns `{ root, env, writeRegistry, writeSboms }` with the fakes first on PATH.
export function fakeTools() {
  const root = mkdtempSync(join(tmpdir(), 'release-scripts-test-'));
  const bin = join(root, 'bin');
  mkdirSync(bin);
  for (const [name, body] of Object.entries({ docker: DOCKER, syft: SYFT, git: GIT })) {
    writeFileSync(join(bin, name), body);
    chmodSync(join(bin, name), 0o755);
  }
  const env = {
    ...process.env,
    PATH: `${bin}${delimiter}${process.env.PATH}`,
    FAKE_REGISTRY: join(root, 'registry.json'),
    FAKE_SBOMS: join(root, 'sboms.json'),
    RUNNER_TEMP: join(root, 'runner-temp'),
    GITHUB_OUTPUT: join(root, 'github-output'),
  };
  mkdirSync(env.RUNNER_TEMP);
  writeFileSync(env.GITHUB_OUTPUT, '');
  return {
    root,
    env,
    writeRegistry: (registry) => writeFileSync(env.FAKE_REGISTRY, JSON.stringify(registry)),
    writeSboms: (sboms) => writeFileSync(env.FAKE_SBOMS, JSON.stringify(sboms)),
  };
}

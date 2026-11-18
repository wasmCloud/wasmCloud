// Renders the runtime-operator chart with host component plugin `volumes`
// and `volumeMounts`, and checks both what it produces and what it refuses.
//
// The chart refuses a bad plugin volume at render time, naming the plugin,
// rather than leaving the API server or the host to reject it without that
// context. Each refusal below is one such case; each must keep failing, and
// with a message that says why.
//
// Invoked by .github/workflows/charts.yml. Needs `helm` and `yq` on PATH.

import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { helmTemplate, yamlToDocs } from './lib/helm.mjs';

const CHART_DIR = 'charts/runtime-operator';
const ROOT = '/var/lib/wasmcloud/host-plugin-volumes';

// Renders the chart with `group` as its only host group. Returns the
// manifests, or the error helm failed with.
function render(group) {
  const values = { runtime: { hostGroups: [{ name: 'default', ...group }] } };
  try {
    return { stdout: helmTemplate(CHART_DIR, 'x', [], { values }) };
  } catch (err) {
    return { error: err, stderr: String(err.stderr ?? err.message) };
  }
}

function renderDocs(group) {
  const r = render(group);
  assert.ifError(r.error);
  const docs = yamlToDocs(r.stdout);
  const deployment = docs.find(
    (d) => d.kind === 'Deployment' && d.metadata.name === 'hostgroup-default',
  );
  const configMap = docs.find(
    (d) => d.kind === 'ConfigMap' && d.metadata.name === 'hostgroup-default-host-config',
  );
  assert.ok(deployment, 'no hostgroup-default Deployment rendered');
  assert.ok(configMap, 'no host config ConfigMap rendered');
  const hostConfig = yamlToDocs(configMap.data['config.yaml'])[0];
  return { pod: deployment.spec.template.spec, hostConfig };
}

function assertRefused(group, pattern) {
  const r = render(group);
  assert.ok(r.error, 'the chart rendered values it must refuse');
  assert.match(r.stderr, pattern);
}

// A component plugin entry with the given volumes and mounts.
function plugin(volumes, volumeMounts, extra = {}) {
  return { id: 'p', image: 'example.com/p:1', volumes, volumeMounts, ...extra };
}

describe('host plugin volumes render', () => {
  const { pod, hostConfig } = renderDocs({
    pluginHostPaths: { allowed: ['/srv/models'] },
    plugins: [
      plugin(
        [
          { name: 'data', configMap: { name: 'cm' } },
          { name: 'scratch', emptyDir: {} },
          { name: 'models', hostPath: { path: '/srv/models/../models/llama', type: 'Directory' } },
        ],
        [
          { name: 'data', mountPath: '/data', readOnly: true },
          { name: 'scratch', mountPath: '/scratch' },
          { name: 'models', mountPath: '/models', readOnly: true },
        ],
      ),
      plugin([{ name: 'data', emptyDir: {} }], [{ name: 'data', mountPath: '/data' }], {
        id: 'q',
      }),
    ],
  });
  const host = pod.containers[0];
  const podVolume = (dir) => {
    const mount = host.volumeMounts.find((m) => m.mountPath === dir);
    assert.ok(mount, `the host container does not mount ${dir}`);
    return { mount, volume: pod.volumes.find((v) => v.name === mount.name) };
  };
  const entry = (id) => hostConfig.host.plugins.find((p) => p.id === id);

  it('mounts each volume into the host container under its plugin and name', () => {
    assert.deepEqual(podVolume(`${ROOT}/p/data`).volume.configMap, { name: 'cm' });
    assert.deepEqual(podVolume(`${ROOT}/p/scratch`).volume.emptyDir, {});
    assert.deepEqual(podVolume(`${ROOT}/q/data`).volume.emptyDir, {});
  });

  it('lists pod volumes in plugin declaration order, not id order', () => {
    const { pod: zfirst } = renderDocs({
      plugins: [
        plugin([{ name: 'a', emptyDir: {} }], [{ name: 'a', mountPath: '/a' }], { id: 'z' }),
        plugin([{ name: 'a', emptyDir: {} }], [{ name: 'a', mountPath: '/a' }], { id: 'b' }),
      ],
    });
    const dirs = zfirst.containers[0].volumeMounts
      .map((m) => m.mountPath)
      .filter((p) => p.startsWith(ROOT));
    assert.deepEqual(dirs, [`${ROOT}/z/a`, `${ROOT}/b/a`]);
  });

  it('gives two plugins a volume of one name without a clash', () => {
    assert.notEqual(podVolume(`${ROOT}/p/data`).mount.name, podVolume(`${ROOT}/q/data`).mount.name);
  });

  it('mounts read-only only where every plugin mount is', () => {
    assert.equal(podVolume(`${ROOT}/p/data`).mount.readOnly, true);
    assert.equal(podVolume(`${ROOT}/p/scratch`).mount.readOnly, undefined);
  });

  it('writes a node hostPath cleaned', () => {
    assert.equal(podVolume(`${ROOT}/p/models`).volume.hostPath.path, '/srv/models/llama');
  });

  it('points the host config at the directories the pod mounts', () => {
    assert.deepEqual(entry('p').volumes, [
      { name: 'data', hostPath: { path: `${ROOT}/p/data` } },
      { name: 'scratch', hostPath: { path: `${ROOT}/p/scratch` } },
      { name: 'models', hostPath: { path: `${ROOT}/p/models` } },
    ]);
    assert.deepEqual(entry('p').volumeMounts, [
      { name: 'data', mountPath: '/data', readOnly: true },
      { name: 'scratch', mountPath: '/scratch' },
      { name: 'models', mountPath: '/models', readOnly: true },
    ]);
  });

  it('keeps names that read as other YAML types strings', () => {
    const { hostConfig: cfg } = renderDocs({
      plugins: [plugin([{ name: 'true', emptyDir: {} }], [{ name: 'true', mountPath: '/t' }])],
    });
    assert.equal(cfg.host.plugins[0].volumes[0].name, 'true');
    assert.equal(cfg.host.plugins[0].volumeMounts[0].name, 'true');
  });
});

describe('host plugin volumes the chart refuses', () => {
  const mount = [{ name: 'm', mountPath: '/m' }];
  const cases = [
    ['an entry with no id', { plugins: [{ image: 'a' }, { image: 'b' }] }, /needs an `id`/],
    ['an entry given as a string', { plugins: ['wasmcloud-nats'] }, /must be a map with an `id`/],
    [
      'a repeated id across plugins and hostPlugins',
      {
        plugins: [{ id: 'p', image: 'a', config: { k: 'v' } }],
        hostPlugins: [plugin([{ name: 'm', emptyDir: {} }], mount)],
      },
      /declared more than once/,
    ],
    [
      'volumes on a native plugin',
      { plugins: [{ id: 'wasmcloud-nats', volumes: [{ name: 'm', emptyDir: {} }], volumeMounts: mount }] },
      /need a component plugin/,
    ],
    ['volumes given as a map', { plugins: [plugin({ m: { emptyDir: {} } }, mount)] }, /must be a list/],
    ['a volume given as a string', { plugins: [plugin(['m'], mount)] }, /must be a map/],
    [
      'a volume name that is not a DNS-1123 label',
      { plugins: [plugin([{ name: 'M', emptyDir: {} }], [{ name: 'M', mountPath: '/m' }])] },
      /DNS-1123 label/,
    ],
    [
      'a volume declared twice',
      { plugins: [plugin([{ name: 'm', emptyDir: {} }, { name: 'm', emptyDir: {} }], mount)] },
      /declared twice/,
    ],
    [
      'readOnly on the volume instead of a source',
      { plugins: [plugin([{ name: 'm', readOnly: true }], mount)] },
      /exactly one Pod volume source.*sets readOnly/,
    ],
    [
      'a misspelled source',
      { plugins: [plugin([{ name: 'm', persistentVolumeclaim: { claimName: 'c' } }], mount)] },
      /exactly one Pod volume source/,
    ],
    [
      'two sources on one volume',
      { plugins: [plugin([{ name: 'm', emptyDir: {}, configMap: { name: 'c' } }], mount)] },
      /exactly one Pod volume source/,
    ],
    [
      'a volume no mount uses',
      { plugins: [plugin([{ name: 'm', emptyDir: {} }, { name: 'n', emptyDir: {} }], mount)] },
      /volume "n" is declared but no `volumeMounts` entry uses it/,
    ],
    [
      'a node hostPath with no allowlist',
      { plugins: [plugin([{ name: 'm', hostPath: { path: '/var/lib/kubelet' } }], mount)] },
      /not within this host group's `pluginHostPaths.allowed`/,
    ],
    [
      'a node hostPath climbing out of the allowlist',
      {
        pluginHostPaths: { allowed: ['/srv/models'] },
        plugins: [plugin([{ name: 'm', hostPath: { path: '/srv/models/../../etc' } }], mount)],
      },
      /node path \/etc, which is not within/,
    ],
    [
      'a node hostPath sharing only a name prefix with the allowlist',
      {
        pluginHostPaths: { allowed: ['/srv/models'] },
        plugins: [plugin([{ name: 'm', hostPath: { path: '/srv/models-evil' } }], mount)],
      },
      /node path \/srv\/models-evil, which is not within/,
    ],
    [
      'a node hostPath that is not a directory',
      {
        pluginHostPaths: { allowed: ['/run'] },
        plugins: [plugin([{ name: 'm', hostPath: { path: '/run/app.sock', type: 'Socket' } }], mount)],
      },
      /`hostPath.type: Socket`/,
    ],
    [
      'a hostPath given as a string',
      {
        pluginHostPaths: { allowed: ['/srv'] },
        plugins: [plugin([{ name: 'm', hostPath: '/srv/models' }], mount)],
      },
      /needs `hostPath` to be a map/,
    ],
    [
      'a mount naming an undeclared volume',
      { plugins: [plugin([{ name: 'm', emptyDir: {} }], [...mount, { name: 'x', mountPath: '/x' }])] },
      /volumeMount "x" names no volume/,
    ],
    [
      'a mount with no mountPath',
      { plugins: [plugin([{ name: 'm', emptyDir: {} }], [{ name: 'm' }])] },
      /needs a `mountPath`/,
    ],
    [
      'a mount key the host does not take',
      { plugins: [plugin([{ name: 'm', emptyDir: {} }], [{ name: 'm', mountPath: '/m', subPath: 's' }])] },
      /sets subPath/,
    ],
    [
      'a readOnly that is a string',
      { plugins: [plugin([{ name: 'm', emptyDir: {} }], [{ name: 'm', mountPath: '/m', readOnly: 'false' }])] },
      /must be the boolean true or false/,
    ],
    [
      'a writable mount of a ConfigMap',
      { plugins: [plugin([{ name: 'm', configMap: { name: 'c' } }], mount)] },
      /its volume is a `configMap`/,
    ],
    [
      'a writable mount of a source marked readOnly',
      { plugins: [plugin([{ name: 'm', persistentVolumeClaim: { claimName: 'c', readOnly: true } }], mount)] },
      /`persistentVolumeClaim` source sets `readOnly: true`/,
    ],
    [
      'a plugin id that is not a directory name',
      { plugins: [plugin([{ name: 'm', emptyDir: {} }], mount, { id: '../evil' })] },
      /needs an `id` of letters, digits/,
    ],
    ...['/', '/var/lib', `${ROOT}`, `${ROOT}/p/m`].map((scratchRoot) => [
      `a scratchRoot of ${scratchRoot}`,
      { scratchRoot, plugins: [plugin([{ name: 'm', emptyDir: {} }], mount)] },
      /`scratchRoot` .* overlaps/,
    ]),
  ];
  for (const [what, group, pattern] of cases) {
    it(`refuses ${what}`, () => assertRefused(group, pattern));
  }

  // The host reserves plugin volume directories itself, so a broad workload
  // allowlist renders, as does a scratch root beside the volumes.
  it('renders a broad hostPaths.allowed and a separate scratchRoot', () => {
    const r = render({
      hostPaths: { allowed: ['/'] },
      scratchRoot: '/var/lib/wasmcloud/scratch',
      plugins: [plugin([{ name: 'm', emptyDir: {} }], [{ name: 'm', mountPath: '/m' }])],
    });
    assert.ifError(r.error);
  });

  it('accepts a numeric plugin id', () => {
    const r = render({ plugins: [{ id: 0, image: 'example.com/p:1', config: { k: 'v' } }] });
    assert.ifError(r.error);
  });
});

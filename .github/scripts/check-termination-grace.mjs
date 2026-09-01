#!/usr/bin/env node
// Asserts every pod this repo ships is given long enough to run the shutdown
// its own code implements, plus room to finish stopping afterwards.
//
// The deadlines are written down in code, and this reads them from there
// rather than repeating the number:
//
//   crates/wash-runtime/src/washlet/mod.rs — COMMAND_DRAIN_TIMEOUT, how long a
//     terminating host waits for the commands already running before it
//     abandons them and stops, unbinding their plugins.
//   runtime-gateway/proxy.go — how long the gateway lets the HTTP requests
//     already in flight finish before it stops serving.
//   controller-runtime's own 30s cap on how long a manager waits for its
//     runnables, which bounds the operator and the gateway. That one lives in
//     the dependency rather than in this repo, so it is written out below and
//     the managers are checked for an override that would move it.
//
// A pod killed before its deadline never gets there: SIGKILL lands first and
// the host leaves plugin bindings behind for the operator's unreachable-host
// path to reap, or the gateway hangs up mid-response. Every deployment was
// pinned at `terminationGracePeriodSeconds: 0` at one point, which made those
// shutdowns unreachable in the only environment they were written for. This
// check is what keeps that from coming back — from either side, since raising a
// deadline past its pod's grace breaks the same way as lowering the grace under
// its deadline.
//
// A Deployment this script has no rule for fails the check: a new pod needs
// someone to decide what its shutdown is owed.
//
// Invoked by .github/workflows/charts.yml. Needs `helm` and `yq` on PATH.

import { readFileSync } from 'node:fs';

import { helmTemplate, yamlToDocs } from './lib/helm.mjs';

const CHART_DIR = 'charts/runtime-operator';
const RELEASE_NAME = 'runtime-operator';

// The chart is not the only way the operator is installed; `make deploy` uses
// this kustomize base, and it has its own grace to keep in step.
const KUSTOMIZE_MANAGER = 'runtime-operator/config/manager/manager.yaml';

// What Kubernetes gives a pod whose spec leaves the field out.
const K8S_DEFAULT_GRACE = 30;

// Room over the deadline for the pod to finish stopping once its drain is done
// — the host still has to unbind every plugin, and a grace equal to the drain
// is SIGKILL at the instant that work starts.
const HEADROOM = 5;

// sigs.k8s.io/controller-runtime's `defaultGracefulShutdownPeriod`: how long a
// manager waits for its runnables to return before giving up on them.
const MANAGER_SHUTDOWN = 30;
// The managers that would have to opt out of it for the number to be wrong.
const MANAGERS = ['runtime-operator/cmd/main.go', 'runtime-gateway/main.go'];

// Deadlines read out of the source that implements them.
const DRAINS = {
  hostCommands: {
    file: 'crates/wash-runtime/src/washlet/mod.rs',
    pattern: /COMMAND_DRAIN_TIMEOUT: Duration = Duration::from_secs\((\d+)\)/,
    what: 'the host drains in-flight commands for',
  },
  gatewayRequests: {
    file: 'runtime-gateway/proxy.go',
    pattern: /gracefulShutdownTimeout = (\d+) \* time\.Second/,
    what: 'the gateway drains in-flight requests for',
  },
};

// Reads a shutdown deadline out of the source that implements it.
function drainSeconds({ file, pattern, what }) {
  const found = readFileSync(file, 'utf8').match(pattern);
  if (!found) {
    console.error(`Could not find how long ${what} in ${file}.`);
    console.error('It moved or changed shape; re-point DRAINS at it in this script.');
    process.exit(1);
  }
  return { seconds: Number(found[1]), what };
}

// The managers take controller-runtime's default shutdown period, so nothing in
// this repo states it. An override would make MANAGER_SHUTDOWN a lie.
function checkManagersTakeTheDefault() {
  for (const manager of MANAGERS) {
    if (readFileSync(manager, 'utf8').includes('GracefulShutdownTimeout')) {
      console.error(`${manager} now sets its own GracefulShutdownTimeout.`);
      console.error(`Read it from there instead of assuming ${MANAGER_SHUTDOWN}s in this script.`);
      process.exit(1);
    }
  }
}

checkManagersTakeTheDefault();
const hostCommands = drainSeconds(DRAINS.hostCommands);
const gatewayRequests = drainSeconds(DRAINS.gatewayRequests);
const managerRunnables = { seconds: MANAGER_SHUTDOWN, what: 'its manager waits for runnables for' };

if (gatewayRequests.seconds > managerRunnables.seconds) {
  console.error(
    `The gateway drains requests for ${gatewayRequests.seconds}s, longer than the ` +
      `${managerRunnables.seconds}s its manager waits for that drain to finish. Raise the ` +
      "manager's GracefulShutdownTimeout or lower the drain; no pod grace can fix this one.",
  );
  process.exit(1);
}

// Keyed by the `wasmcloud.com/name` label the chart puts on every deployment.
// `longest` is the slowest shutdown the pod can legitimately take.
const RULES = {
  hostgroup: {
    longest: hostCommands,
    values: 'runtime.terminationGracePeriodSeconds',
  },
  // Both managers are bounded by controller-runtime rather than by their own
  // drains, the gateway's included — that drain runs inside a runnable the
  // manager is waiting on.
  'runtime-gateway': {
    longest: managerRunnables,
    values: 'gateway.terminationGracePeriodSeconds',
  },
  'runtime-operator': {
    longest: managerRunnables,
    values: 'operator.terminationGracePeriodSeconds',
  },
  // NATS closes its client connections and its JetStream store and is gone;
  // nothing bounds it, so all that is asked is that it not be pinned at 0.
  nats: {
    longest: { seconds: 0, what: 'it handles SIGTERM and stops in' },
    values: 'nats.terminationGracePeriodSeconds',
  },
};

const failures = [];

// Checks one pod spec's grace against the shutdown it has to cover.
function checkGrace(name, rule, podSpec, source) {
  const grace = podSpec?.terminationGracePeriodSeconds ?? K8S_DEFAULT_GRACE;
  const required = rule.longest.seconds + HEADROOM;

  if (grace < required) {
    failures.push(
      `${name}: terminationGracePeriodSeconds is ${grace}, but ${rule.longest.what} ` +
        `${rule.longest.seconds}s and stopping takes longer still. ` +
        `Raise \`${rule.values}\` in ${source} to at least ${required}.`,
    );
  } else {
    console.log(`${name}: ${grace}s grace covers ${rule.longest.seconds}s + ${HEADROOM}s headroom`);
  }
}

const deployments = yamlToDocs(helmTemplate(CHART_DIR, RELEASE_NAME)).filter(
  (doc) => doc.kind === 'Deployment',
);
if (deployments.length === 0) {
  console.error(`No Deployments rendered from ${CHART_DIR}; the chart or this check is broken.`);
  process.exit(1);
}

for (const deployment of deployments) {
  const name = deployment.metadata?.name;
  const rule = RULES[deployment.metadata?.labels?.['wasmcloud.com/name']];
  if (!rule) {
    failures.push(
      `${name}: no rule for this deployment. Add one to RULES in this script, keyed by its ` +
        `\`wasmcloud.com/name\` label, saying what its shutdown has to cover.`,
    );
    continue;
  }
  checkGrace(name, rule, deployment.spec?.template?.spec, `${CHART_DIR}/values.yaml`);
}

const kustomized = yamlToDocs(readFileSync(KUSTOMIZE_MANAGER, 'utf8')).find(
  (doc) => doc.kind === 'Deployment',
);
if (!kustomized) {
  console.error(`No Deployment in ${KUSTOMIZE_MANAGER}; re-point this check at the operator's.`);
  process.exit(1);
}
checkGrace(
  `${KUSTOMIZE_MANAGER} (kustomize)`,
  { longest: managerRunnables, values: 'terminationGracePeriodSeconds' },
  kustomized.spec?.template?.spec,
  KUSTOMIZE_MANAGER,
);

if (failures.length > 0) {
  console.error('\nA pod is killed before it can finish shutting down:\n');
  for (const failure of failures) {
    console.error(`  - ${failure}`);
  }
  process.exit(1);
}

console.log(`\nAll ${deployments.length + 1} pods outlive their own shutdown.`);

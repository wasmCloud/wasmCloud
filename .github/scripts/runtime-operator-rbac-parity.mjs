#!/usr/bin/env node
// Verifies that the runtime-operator Helm chart grants every (apiGroup,
// resource, verb) tuple that controller-gen emits into
// runtime-operator/config/rbac/role.yaml.
//
// The chart is hand-mapped from role.yaml — it splits the single generated
// ClusterRole across a structural layout of (Cluster)Role + (Cluster)RoleBinding
// (e.g. pods move to a namespaced Role in the operator's namespace and per-host
// namespaces; configmaps/secrets/services/events move to a ClusterRole bound
// per-namespace when operator.watchNamespaces is set). Because the mapping is
// manual, a new `+kubebuilder:rbac` marker can land in code and `role.yaml`
// without the corresponding chart edit, and the operator hits a permissions
// error only at runtime on a cluster with OwnerReferencesPermissionEnforcement
// (or any plugin that exercises the missing verb) enabled.
//
// This check renders the chart in two modes — defaults and with
// watchNamespaces/hostNamespaces set — so a marker missing from either path
// is caught. The "union of granted verbs" is compared against role.yaml
// without regard to scope.
//
// In addition to that tuple-coverage check, the script also asserts the
// chart's intentional structural shape per mode: in watch-all mode the
// workload-side runtime.wasmcloud.dev CRD verbs must live on an
// operator-bound ClusterRole, and in scoped (watchNamespaces) mode they must
// live only on per-namespace Roles in each watched namespace — never on a
// ClusterRole. This catches accidental re-introduction of a cluster-wide
// grant in scoped installs, which would silently defeat the watchNamespaces
// scoping the chart advertises.

import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

const CHART_DIR = 'charts/runtime-operator';
const GENERATED_ROLE = 'runtime-operator/config/rbac/role.yaml';
// `runtime-operator.fullname` collapses to the release name when the release
// name already contains the chart name, so picking a release name like this
// keeps the rendered SA name short and predictable.
const RELEASE_NAME = 'runtime-operator';
const OPERATOR_SA = RELEASE_NAME;

// Workload-side CRD resources whose RBAC scope follows watchNamespaces — the
// "Support runtime.wasmcloud.dev apiGroup RBAC to be namespaced vs Cluster"
// change. Host is intentionally excluded: Host objects live in the operator's
// own namespace and are granted via the namespaced Role in role.yaml in both
// modes.
const WATCH_SCOPED_RESOURCES = [
  'artifacts',
  'workloads',
  'workloadreplicasets',
  'workloaddeployments',
];

const WATCHED_NS = 'ns-a';
const HOST_NS = 'ns-b';

const RENDER_MODES = [
  {
    name: 'defaults (watchNamespaces empty, hostNamespaces empty)',
    sets: [],
    // In watch-all mode the workload CRD verbs come from a cluster-wide
    // ClusterRole; no per-namespace Roles carry them.
    expectWorkloadScope: 'cluster',
    expectedPerNsRoles: [],
  },
  {
    name: `watched (watchNamespaces=[${WATCHED_NS}], hostNamespaces=[${HOST_NS}])`,
    sets: [
      `operator.watchNamespaces[0]=${WATCHED_NS}`,
      `operator.hostNamespaces[0]=${HOST_NS}`,
    ],
    // In scoped mode the workload CRD verbs must come from per-namespace
    // Roles only — no operator-bound ClusterRole may grant them.
    expectWorkloadScope: 'namespaced',
    expectedPerNsRoles: [
      { namespace: WATCHED_NS, name: `${RELEASE_NAME}-workload-crd` },
      { namespace: WATCHED_NS, name: `${RELEASE_NAME}-workload-namespace` },
      { namespace: WATCHED_NS, name: `${RELEASE_NAME}-endpointslice` },
    ],
  },
];

function yamlToDocs(yamlText) {
  // yq -I 0 emits one compact JSON object per YAML doc, one per line.
  const r = spawnSync('yq', ['-o', 'json', '-I', '0'], {
    input: yamlText,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
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

function helmTemplate(sets) {
  const args = ['template', RELEASE_NAME, CHART_DIR];
  for (const s of sets) {
    args.push('--set', s);
  }
  return execFileSync('helm', args, {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
}

// Flat list of every rule the operator SA picks up across all bindings.
// Scope is intentionally collapsed: tuple coverage doesn't care whether a
// grant is cluster-wide or namespaced. Use operatorBoundRoles when you need
// to assert on scope or namespace.
function operatorRules(docs) {
  const { clusterRoles, namespacedRoles } = operatorBoundRoles(docs);
  return [...clusterRoles, ...namespacedRoles].flatMap((doc) => doc.rules ?? []);
}

// Return the (Cluster)Role docs bound to the operator SA, separated by kind.
// A binding's kind determines scope: a Role bound to the SA grants only within
// the binding's namespace; a ClusterRole bound via ClusterRoleBinding grants
// cluster-wide. Both lists carry the actual Role/ClusterRole doc (including
// metadata.namespace for Roles) so callers can assert on namespace as well as
// name. A (Cluster)Role that isn't bound to the operator is excluded — e.g.
// the gateway ClusterRole also lists `workloads`, but that grant goes to the
// gateway SA, not the operator.
function operatorBoundRoles(docs) {
  const clusterBound = new Set();
  // Map<bindingNamespace, Set<roleName>> — a RoleBinding can only reference a
  // Role in its own namespace, so we key by binding namespace.
  const nsBound = new Map();
  for (const doc of docs) {
    const bindsOperator = (doc.subjects ?? []).some(
      (s) => s.kind === 'ServiceAccount' && s.name === OPERATOR_SA,
    );
    if (!bindsOperator) continue;
    const refName = doc.roleRef?.name;
    const refKind = doc.roleRef?.kind;
    if (!refName) continue;
    if (doc.kind === 'ClusterRoleBinding' && refKind === 'ClusterRole') {
      clusterBound.add(refName);
    } else if (doc.kind === 'RoleBinding') {
      // RoleBinding's roleRef.kind can be Role or ClusterRole; both grant
      // permissions scoped to the binding's namespace.
      const bindingNs = doc.metadata?.namespace;
      if (!bindingNs) continue;
      if (!nsBound.has(bindingNs)) nsBound.set(bindingNs, new Set());
      nsBound.get(bindingNs).add(refName);
    }
  }

  const clusterRoles = [];
  const namespacedRoles = [];
  for (const doc of docs) {
    const name = doc.metadata?.name;
    if (!name) continue;
    if (doc.kind === 'ClusterRole' && clusterBound.has(name)) {
      clusterRoles.push(doc);
    } else if (doc.kind === 'Role') {
      const ns = doc.metadata?.namespace;
      const namesInNs = nsBound.get(ns);
      if (namesInNs?.has(name)) namespacedRoles.push(doc);
    }
  }
  return { clusterRoles, namespacedRoles };
}

// Does any of these rules mention the given (group, resource)? Treats
// subresources like `workloads/status` as a hit for `workloads` so callers can
// ask the simple question "does this Role touch workloads at all?".
function rulesMentionResource(rules, group, resource) {
  return rules.some(
    (r) =>
      (r.apiGroups ?? []).includes(group) &&
      (r.resources ?? []).some(
        (res) => res === resource || res.startsWith(`${resource}/`),
      ),
  );
}

// Verify the chart's RBAC has the structural shape expected for this mode:
//   * workload CRD verbs only come from the expected scope (cluster-wide in
//     watch-all; namespaced in scoped mode);
//   * every per-namespace Role we expect actually exists and is bound to the
//     operator SA in the right namespace.
// Returns an array of failure messages; empty means structure is correct.
function checkStructure(mode, docs) {
  const { clusterRoles, namespacedRoles } = operatorBoundRoles(docs);
  const failures = [];

  // 1. Where do the workload-scoped CRD verbs live?
  const group = 'runtime.wasmcloud.dev';
  for (const resource of WATCH_SCOPED_RESOURCES) {
    const inCluster = clusterRoles.some((cr) =>
      rulesMentionResource(cr.rules ?? [], group, resource),
    );
    const inNs = namespacedRoles
      .filter((r) => rulesMentionResource(r.rules ?? [], group, resource))
      .map((r) => `${r.metadata.namespace}/${r.metadata.name}`);

    if (mode.expectWorkloadScope === 'cluster') {
      if (!inCluster) {
        failures.push(
          `expected an operator-bound ClusterRole to grant ${group}/${resource}, found none`,
        );
      }
      if (inNs.length > 0) {
        failures.push(
          `expected no per-namespace Role to grant ${group}/${resource} in watch-all mode, found: ${inNs.join(', ')}`,
        );
      }
    } else if (mode.expectWorkloadScope === 'namespaced') {
      if (inCluster) {
        failures.push(
          `expected NO operator-bound ClusterRole to grant ${group}/${resource} in scoped mode (cluster-wide grant would defeat watchNamespaces scoping)`,
        );
      }
      if (inNs.length === 0) {
        failures.push(
          `expected at least one per-namespace Role to grant ${group}/${resource} in scoped mode, found none`,
        );
      }
    }
  }

  // 2. Every expected per-namespace Role is present and bound to the SA.
  for (const expected of mode.expectedPerNsRoles) {
    const found = namespacedRoles.some(
      (r) =>
        r.metadata?.name === expected.name &&
        r.metadata?.namespace === expected.namespace,
    );
    if (!found) {
      failures.push(
        `expected operator-bound Role ${expected.namespace}/${expected.name}, not found`,
      );
    }
  }

  return failures;
}

function expandTuples(rule) {
  const out = [];
  for (const g of rule.apiGroups ?? []) {
    for (const r of rule.resources ?? []) {
      for (const v of rule.verbs ?? []) {
        out.push([g, r, v]);
      }
    }
  }
  return out;
}

function ruleCovers(rule, group, resource, verb) {
  return (
    (rule.apiGroups ?? []).includes(group) &&
    (rule.resources ?? []).includes(resource) &&
    (rule.verbs ?? []).includes(verb)
  );
}

function tupleKey(t) {
  return t.join('\0');
}

const generatedDocs = yamlToDocs(readFileSync(GENERATED_ROLE, 'utf8'));
const neededTuples = generatedDocs
  .filter((d) => d?.kind === 'ClusterRole' || d?.kind === 'Role')
  .flatMap((d) => d.rules ?? [])
  .flatMap(expandTuples);
const dedupedNeeded = [...new Map(neededTuples.map((t) => [tupleKey(t), t])).values()];

let parityFailed = false;
let structureFailed = false;
for (const mode of RENDER_MODES) {
  const docs = yamlToDocs(helmTemplate(mode.sets));
  const chartRules = operatorRules(docs);

  const missing = dedupedNeeded.filter(
    ([g, r, v]) => !chartRules.some((rule) => ruleCovers(rule, g, r, v)),
  );

  if (missing.length === 0) {
    console.log(
      `OK   ${mode.name}: chart covers all ${dedupedNeeded.length} tuples in ${GENERATED_ROLE}`,
    );
  } else {
    parityFailed = true;
    console.error(
      `FAIL ${mode.name}: chart is missing ${missing.length} of ${dedupedNeeded.length} tuples:`,
    );
    for (const [g, r, v] of missing) {
      const groupLabel = g === '' ? '(core)' : g;
      console.error(`       - apiGroup=${groupLabel} resource=${r} verb=${v}`);
    }
  }

  const structureFailures = checkStructure(mode, docs);
  if (structureFailures.length === 0) {
    console.log(`OK   ${mode.name}: RBAC structure matches expected scope`);
  } else {
    structureFailed = true;
    console.error(
      `FAIL ${mode.name}: RBAC structure does not match expected scope:`,
    );
    for (const msg of structureFailures) {
      console.error(`       - ${msg}`);
    }
  }
}

if (parityFailed) {
  console.error('');
  console.error(`Chart ${CHART_DIR} does not grant the operator service account`);
  console.error(`every permission that ${GENERATED_ROLE} (the controller-gen output)`);
  console.error('says the operator needs. Add the missing rules to the appropriate');
  console.error(`template under ${CHART_DIR}/templates/operator/ — typically`);
  console.error('clusterrole.yaml for cluster-wide rules, role.yaml for the operator');
  console.error('namespace, or workload-namespace-role.yaml for the watched-namespace');
  console.error('path.');
}

if (structureFailed) {
  console.error('');
  console.error('Chart RBAC structure is wrong for at least one render mode.');
  console.error('In watch-all mode the workload CRD verbs must live on an operator');
  console.error('ClusterRole and not on any per-namespace Role. In scoped mode no');
  console.error('operator-bound ClusterRole may grant those verbs — they must be');
  console.error('granted only by per-namespace Roles in each watchNamespaces entry.');
  console.error(`Adjust the templates under ${CHART_DIR}/templates/operator/.`);
}

if (parityFailed || structureFailed) {
  process.exit(1);
}

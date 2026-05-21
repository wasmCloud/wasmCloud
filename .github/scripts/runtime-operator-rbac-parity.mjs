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
// without regard to scope; namespaced-vs-cluster-wide is the chart's
// deliberate structural choice and not what this script polices.

import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

const CHART_DIR = 'charts/runtime-operator';
const GENERATED_ROLE = 'runtime-operator/config/rbac/role.yaml';
// `runtime-operator.fullname` collapses to the release name when the release
// name already contains the chart name, so picking a release name like this
// keeps the rendered SA name short and predictable.
const RELEASE_NAME = 'runtime-operator';
const OPERATOR_SA = RELEASE_NAME;

const RENDER_MODES = [
  {
    name: 'defaults (watchNamespaces empty, hostNamespaces empty)',
    sets: [],
  },
  {
    name: 'watched (watchNamespaces=[ns-a], hostNamespaces=[ns-b])',
    sets: [
      'operator.watchNamespaces[0]=ns-a',
      'operator.hostNamespaces[0]=ns-b',
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

// Walk bindings, return the set of (Cluster)Role names attached to the
// operator's service account. A (Cluster)Role that isn't bound to the operator
// is irrelevant — e.g. the gateway ClusterRole also lists `workloads`, but
// that grant goes to the gateway SA, not the operator.
function operatorBoundRoleNames(docs) {
  const names = new Set();
  for (const doc of docs) {
    if (doc.kind !== 'ClusterRoleBinding' && doc.kind !== 'RoleBinding') continue;
    const bindsOperator = (doc.subjects ?? []).some(
      (s) => s.kind === 'ServiceAccount' && s.name === OPERATOR_SA,
    );
    if (!bindsOperator) continue;
    if (doc.roleRef?.name) names.add(doc.roleRef.name);
  }
  return names;
}

function operatorRules(docs) {
  const boundRoles = operatorBoundRoleNames(docs);
  const rules = [];
  for (const doc of docs) {
    if (doc.kind !== 'Role' && doc.kind !== 'ClusterRole') continue;
    if (!boundRoles.has(doc.metadata?.name)) continue;
    for (const rule of doc.rules ?? []) {
      rules.push(rule);
    }
  }
  return rules;
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

let failed = false;
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
    continue;
  }

  failed = true;
  console.error(
    `FAIL ${mode.name}: chart is missing ${missing.length} of ${dedupedNeeded.length} tuples:`,
  );
  for (const [g, r, v] of missing) {
    const groupLabel = g === '' ? '(core)' : g;
    console.error(`       - apiGroup=${groupLabel} resource=${r} verb=${v}`);
  }
}

if (failed) {
  console.error('');
  console.error(`Chart ${CHART_DIR} does not grant the operator service account`);
  console.error(`every permission that ${GENERATED_ROLE} (the controller-gen output)`);
  console.error('says the operator needs. Add the missing rules to the appropriate');
  console.error(`template under ${CHART_DIR}/templates/operator/ — typically`);
  console.error('clusterrole.yaml for cluster-wide rules, role.yaml for the operator');
  console.error('namespace, or workload-namespace-role.yaml for the watched-namespace');
  console.error('path.');
  process.exit(1);
}

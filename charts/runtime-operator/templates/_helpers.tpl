{{/*
Expand the name of the chart.
*/}}
{{- define "runtime-operator.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
We truncate at 63 chars because some Kubernetes name fields are limited to this (by the DNS naming spec).
If release name contains chart name it will be used as a full name.
*/}}
{{- define "runtime-operator.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "runtime-operator.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "runtime-operator.labels" -}}
helm.sh/chart: {{ include "runtime-operator.chart" . }}
{{ include "runtime-operator.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Pod labels for the operator (common labels + operator podLabels)
*/}}
{{- define "operator.podLabels" -}}
{{ include "runtime-operator.labels" . }}
{{- with .Values.operator.podLabels }}
{{ toYaml . }}
{{- end }}
{{- end }}

{{/*
Pod labels for NATS (common labels + nats podLabels)
*/}}
{{- define "nats.podLabels" -}}
{{ include "runtime-operator.labels" . }}
{{- with .Values.nats.podLabels }}
{{ toYaml . }}
{{- end }}
{{- end }}

{{/*
Pod labels for runtime (common labels + runtime podLabels)
*/}}
{{- define "runtime.podLabels" -}}
{{ include "runtime-operator.labels" . }}
{{- with .Values.runtime.podLabels }}
{{ toYaml . }}
{{- end }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "runtime-operator.selectorLabels" -}}
app.kubernetes.io/name: {{ include "runtime-operator.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Create the name of the service account to use for the runtime-operator
*/}}
{{- define "operator.serviceAccountName" -}}
{{- if .Values.operator.serviceAccount.create }}
{{- default (include "runtime-operator.fullname" .) .Values.operator.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.operator.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Create the name of the service account to use for the runtime
*/}}
{{- define "runtime.serviceAccountName" -}}
{{- if .Values.runtime.serviceAccount.create }}
{{- default (printf "%s-runtime" (include "runtime-operator.fullname" .)) .Values.runtime.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.runtime.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Create the name of the service account to use for the runtime gateway
*/}}
{{- define "gateway.serviceAccountName" -}}
{{- if .Values.gateway.serviceAccount.create }}
{{- default (printf "%s-gateway" (include "runtime-operator.fullname" .)) .Values.gateway.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.gateway.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Create the name of the service account to use for NATS
*/}}
{{- define "nats.serviceAccountName" -}}
{{- if .Values.nats.serviceAccount.create }}
{{- default (printf "%s-nats" (include "runtime-operator.fullname" .)) .Values.nats.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.nats.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Returns the deduped, comma-separated list of namespaces where host Pods
run, used both for the operator's `-host-namespaces` flag and for the
per-namespace Pod RBAC rendering in host-pod-role.yaml.

Sources:
  1. Explicit operator.hostNamespaces entries.
  2. Every distinct runtime.hostGroups[].namespace.

The operator's own namespace is excluded — Pod RBAC there is granted
by the in-namespace operator Role, and the Pod cache always covers
operatorCfg.Namespace separately. Empty values are dropped. Result is
sorted for stable rendering.

Callers parse the comma-separated string with `splitList ","`.
*/}}
{{- define "runtime-operator.hostNamespaces" -}}
{{- $set := dict }}
{{- range .Values.operator.hostNamespaces }}
  {{- if and . (ne . $.Release.Namespace) }}{{ $_ := set $set . true }}{{ end }}
{{- end }}
{{- range .Values.runtime.hostGroups }}
  {{- $ns := default "" .namespace }}
  {{- if and $ns (ne $ns $.Release.Namespace) }}{{ $_ := set $set $ns true }}{{ end }}
{{- end }}
{{- join "," (keys $set | sortAlpha) }}
{{- end }}

{{/*
Control-plane / scheduler NATS URL. Used by the operator (`-nats-url`) and the
host runtime (`--scheduler-nats-url`).

Resolution order (first non-empty wins):
  1. Per-host-group override (`.schedulerNatsUrl`).
  2. Chart-wide `global.nats.schedulerUrl`.
  3. The in-cluster NATS service built from the release namespace
     (`nats://nats.<namespace>.svc.cluster.local:4222`) — for backwards compatibility.

Callable two ways:
  {{ include "runtime-operator.schedulerNatsUrl" . }} (operator: ctx is top)
  {{ include "runtime-operator.schedulerNatsUrl" (dict "ctx" $top "group" .) }}  (runtime: per-group)
*/}}
{{- define "runtime-operator.schedulerNatsUrl" -}}
{{- $ctx := . }}{{- $group := dict }}
{{- if and (kindIs "map" .) (hasKey . "ctx") }}{{- $ctx = .ctx }}{{- $group = default dict .group }}{{- end }}
{{- $default := default (printf "nats://nats.%s.svc.cluster.local:4222" $ctx.Release.Namespace) $ctx.Values.global.nats.schedulerUrl }}
{{- default $default $group.schedulerNatsUrl -}}
{{- end }}

{{/*
Data-plane NATS URL. Used by the host runtime (`--data-nats-url`) so Wasm
workloads can use a separate NATS cluster from the control plane if desired.
*/}}
{{- define "runtime-operator.dataNatsUrl" -}}
{{- $ctx := . }}{{- $group := dict }}
{{- if and (kindIs "map" .) (hasKey . "ctx") }}{{- $ctx = .ctx }}{{- $group = default dict .group }}{{- end }}
{{- $default := default (printf "nats://nats.%s.svc.cluster.local:4222" $ctx.Release.Namespace) $ctx.Values.global.nats.dataUrl }}
{{- default $default $group.dataNatsUrl -}}
{{- end }}

{{/*
Partitions a host group's plugins[] (plus the deprecated hostPlugins[] alias)
into file-backed entries vs plain CLI-only entries, and collects the deduped
configFrom/secretFrom names referenced across the file-backed set.

An entry is file-backed when it sets anything `--host-plugin` cannot express —
config/configFrom/secretFrom/allowedHosts/allowedIpNameLookups, or any of the
binding fields (workloadConfig/hostOwnedKeys/bindings). A secret must never
land on the command line, and a native entry (no image/file) has nothing to put
there at all, so it is always file-backed.

Shared by deployment.yaml (which needs both partitions, to render
`--host-plugin` args for `cli` and mount volumes for `fileBacked`) and
host-plugin-config.yaml (which needs only `fileBacked`, to render the
`wash host` config file).

Takes the host group dict directly (e.g. `.` inside
`range .Values.runtime.hostGroups`). Returns a JSON object
`{fileBacked, cli, configFromNames, secretFromNames, needsConfigFile}`
and parses the result with `fromJson`.
*/}}
{{- define "runtime-operator.hostPluginPartition" -}}
{{- /* Removed keys, refused rather than ignored. Helm drops values nothing
       reads, so an upgrade that keeps `wasmcloudNats` would render a host with
       no binding, no credential and no grant — visible only as denied calls,
       from a values file that still reads correct. */}}
{{- if .wasmcloudNats }}
{{- fail "runtime.hostGroups[].wasmcloudNats has been removed: declare it under this host group's `plugins` as an entry with `id: wasmcloud-nats`, moving `config`/`configFrom`/`secretFrom` onto the entry and `bindings` across unchanged" }}
{{- end }}
{{- if .wasmcloudNatsWorkloadConfig }}
{{- fail "runtime.hostGroups[].wasmcloudNatsWorkloadConfig has been removed: set `workloadConfig` on this host group's `plugins` entry with `id: wasmcloud-nats`" }}
{{- end }}
{{- $fileBacked := list }}
{{- $cli := list }}
{{- range concat (default list .plugins) (default list .hostPlugins) }}
{{- if or .config .configFrom .secretFrom .allowedHosts .allowedIpNameLookups .workloadConfig .hostOwnedKeys .bindings (not (or .image .file)) }}
{{- $fileBacked = append $fileBacked . }}
{{- else }}
{{- $cli = append $cli . }}
{{- end }}
{{- end }}
{{- $configFromNames := list }}
{{- $secretFromNames := list }}
{{- range $fileBacked }}
{{- range .configFrom }}
{{- $configFromNames = append $configFromNames . }}
{{- end }}
{{- range .secretFrom }}
{{- $secretFromNames = append $secretFromNames . }}
{{- end }}
{{- range $name, $binding := (default dict .bindings) }}
{{- range $binding.configFrom }}
{{- $configFromNames = append $configFromNames . }}
{{- end }}
{{- range $binding.secretFrom }}
{{- $secretFromNames = append $secretFromNames . }}
{{- end }}
{{- end }}
{{- end }}
{{- dict "fileBacked" $fileBacked "cli" $cli "configFromNames" ($configFromNames | uniq) "secretFromNames" ($secretFromNames | uniq) "needsConfigFile" (gt (len $fileBacked) 0) | toJson }}
{{- end }}

{{/*
The `wash host` config file (--config) for ONE host group's file-backed
hostPlugins entries, rendered at column 0 so callers indent it where they need
it.

Two callers, and that is the point: host-plugin-config.yaml projects it into a
ConfigMap, and deployment.yaml hashes it into a `checksum/host-plugin-config`
pod annotation so an edit rolls the host (which reads this file once, at
startup). Hashing the real rendered output rather than the values it derives
from means a change to the shape of this file — not just to a value in it —
also rolls the pod.

Takes a `hostPluginPartition` result, passed in rather than recomputed so each
caller evaluates it exactly once.

`configFrom`/`secretFrom` name plain Kubernetes ConfigMaps/Secrets in the
release namespace. deployment.yaml projects each referenced one as a volume
under /etc/wasmcloud/host-plugin-{config,secrets}/<name> — the directory shape
`dir:` (wash_runtime::config_source) reads, one file per key, matching how
Kubernetes itself projects a ConfigMap/Secret volume. The catalog below points
`dir:` at that exact mount path, so `host.hostPlugins[].configFrom/secretFrom`
resolve by name exactly like `workload.environment.configFrom/secretFrom` do.
Only the host process reads these paths — the target plugin only ever calls
e.g. `wasmcloud:secrets`'s `get`.
*/}}
{{- define "runtime-operator.hostPluginConfigFile" -}}
{{- $partition := . }}
{{- $configNames := $partition.configFromNames }}
{{- $secretNames := $partition.secretFromNames }}
{{- if $configNames }}
configs:
  {{- range $configNames }}
  {{ . }}:
    dir: /etc/wasmcloud/host-plugin-config/{{ . }}
  {{- end }}
{{- end }}
{{- if $secretNames }}
secrets:
  {{- range $secretNames }}
  {{ . }}:
    dir: /etc/wasmcloud/host-plugin-secrets/{{ . }}
  {{- end }}
{{- end }}
host:
  {{- if $partition.fileBacked }}
  plugins:
    {{- range $partition.fileBacked }}
    - id: {{ .id }}
      {{- if .image }}
      image: {{ .image }}
      {{- if .pullPolicy }}
      pullPolicy: {{ .pullPolicy }}
      {{- end }}
      {{- if .digest }}
      expectedDigest: {{ .digest }}
      {{- end }}
      {{- else if .file }}
      file: {{ .file }}
      {{- end }}
      {{- if .maxRestarts }}
      maxRestarts: {{ .maxRestarts }}
      {{- end }}
      {{- with .config }}
      config:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with .configFrom }}
      configFrom:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with .secretFrom }}
      secretFrom:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with .allowedHosts }}
      allowedHosts:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with .allowedIpNameLookups }}
      allowedIpNameLookups:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with .workloadConfig }}
      workloadConfig: {{ . }}
      {{- end }}
      {{- with .hostOwnedKeys }}
      hostOwnedKeys:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with .bindings }}
      bindings:
        {{- toYaml . | nindent 8 }}
      {{- end }}
    {{- end }}
  {{- end }}
{{- end }}

{{/*
Reloader annotations naming the ConfigMaps/Secrets a host group's plugin config
references.

The host reads its config file once, at startup, and the kubelet updates a
projected volume in place — so rotating a ConfigMap/Secret named by
`configFrom`/`secretFrom` changes the bytes on disk while the running host goes
on serving the values it resolved at boot. The `checksum/host-plugin-config`
annotation cannot catch this: the referenced objects live outside this chart,
and their contents are not part of any value it renders.

These annotations delegate that to stakater/Reloader, which watches the named
objects and rolls the Deployment when their data changes. They are inert
without the controller installed, so they cost nothing when it isn't — but
without it, rotation genuinely does not reach a running host until something
else restarts it.

Takes the same `hostPluginPartition` result as `runtime-operator.hostPluginConfigFile`.
*/}}
{{- define "runtime-operator.hostPluginReloaderAnnotations" -}}
{{- $partition := . }}
{{- if $partition.configFromNames }}
configmap.reloader.stakater.com/reload: {{ join "," $partition.configFromNames | quote }}
{{- end }}
{{- if $partition.secretFromNames }}
secret.reloader.stakater.com/reload: {{ join "," $partition.secretFromNames | quote }}
{{- end }}
{{- end }}

{{/*
Create the imagePullSecrets section for the chart.
*/}}
{{- define "runtime-operator.imagePullSecrets" -}}
{{- if .Values.global.image.pullSecrets }}
imagePullSecrets:
{{- range .Values.global.image.pullSecrets }}
  - name: {{ .name }}
{{- end }}
{{- end }}
{{- end }}
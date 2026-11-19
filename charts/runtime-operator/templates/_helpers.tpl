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
config/configFrom/secretFrom/allowedHosts/allowedIpNameLookups/
allowedHostLoopbackPorts/ports/volumes/volumeMounts, or any binding field
(workloadConfig/hostOwnedKeys/bindings). A secret must never
land on the command line, and a native entry (no image/file) has nothing to put
there at all, so it is always file-backed.

Shared by deployment.yaml (which needs both partitions, to render
`--host-plugin` args for `cli` and mount volumes for `fileBacked`) and
host-plugin-config.yaml (which needs only `fileBacked`, to render the
`wash host` config file).

A `secretFrom` name the host group declares under `pluginSecrets` is served by
a host plugin at bind, not by a Kubernetes Secret: it goes to
`pluginSecrets` (rendered into the `secrets:` catalog as is) instead of
`secretFromNames`, so it gets no volume and no Reloader annotation.

Takes the host group dict directly (e.g. `.` inside
`range .Values.runtime.hostGroups`). Returns a JSON object
`{fileBacked, cli, configFromNames, secretFromNames, pluginSecrets, pluginVolumes, needsConfigFile}`,
which callers parse with `fromJson`. `pluginVolumes` is every entry's
`runtime-operator.hostPluginVolumes` in declaration order, and each
`fileBacked` entry carries its own as `resolvedVolumes`.

Refuses an entry that is not a map, has no `id` or repeats one, and, when
any plugin has volumes, a `scratchRoot` that overlaps one of them.
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
{{- $group := . }}
{{- $fileBacked := list }}
{{- $cli := list }}
{{- $pluginVolumes := list }}
{{- $seenIds := dict }}
{{- range concat (default list .plugins) (default list .hostPlugins) }}
{{- if not (kindIs "map" .) }}
{{- fail (printf "host plugin entry %s must be a map with an `id`" (toJson .)) }}
{{- end }}
{{- if or (not (hasKey . "id")) (eq (toString .id) "") (eq (toString .id) "<nil>") }}
{{- fail (printf "host plugin entry needs an `id`: %s" (toJson .)) }}
{{- end }}
{{- /* The host refuses a repeated id anyway. Refusing it here names the
       entry before any pod rolls, and keeps each id's volume directories
       and pod volume names its own. */}}
{{- if hasKey $seenIds (toString .id) }}
{{- fail (printf "host plugin %s: declared more than once across `plugins` and `hostPlugins`; plugin ids must be unique" (toString .id)) }}
{{- end }}
{{- $_ := set $seenIds (toString .id) true }}
{{- $volumes := include "runtime-operator.hostPluginVolumes" (dict "entry" . "group" $group) | fromJsonArray }}
{{- $pluginVolumes = concat $pluginVolumes $volumes }}
{{- if or .config .configFrom .secretFrom .allowedHosts .allowedIpNameLookups .allowedHostLoopbackPorts .ports .workloadConfig .hostOwnedKeys .bindings .volumes .volumeMounts (not (or .image .file)) }}
{{- /* The config file names each volume by the directory the pod mounts it
       at, so the entry carries its resolved volumes. */}}
{{- $fileBacked = append $fileBacked (merge (dict "resolvedVolumes" $volumes) .) }}
{{- else }}
{{- $cli = append $cli . }}
{{- end }}
{{- end }}
{{- /* The host refuses a plugin volume directory that equals, contains or
       lies inside its emptyDir scratch root, and would fail to start. Refused
       here, for each directory the chart mounts, so the error names the
       setting. A relative `scratchRoot`, a `--scratch-root` in `extraArgs` and
       the default under TMPDIR all depend on what only the host sees, so the
       host's own check covers those. */}}
{{- if and .scratchRoot (hasPrefix "/" (toString .scratchRoot)) }}
{{- $scratch := clean (toString .scratchRoot) }}
{{- range $pluginVolumes }}
{{- $dir := .mountPath }}
{{- if or (eq $scratch $dir) (hasPrefix (printf "%s/" (trimSuffix "/" $scratch)) $dir) (hasPrefix (printf "%s/" $dir) $scratch) }}
{{- fail (printf "hostGroup %s: `scratchRoot` %s overlaps plugin volume directory %s; the host refuses a plugin volume inside or around its scratch root" $group.name $scratch $dir) }}
{{- end }}
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
{{- $declared := default dict .pluginSecrets }}
{{- $pluginSecrets := dict }}
{{- $mountedSecretNames := list }}
{{- range ($secretFromNames | uniq) }}
{{- if hasKey $declared . }}
{{- $_ := set $pluginSecrets . (index $declared .) }}
{{- else }}
{{- $mountedSecretNames = append $mountedSecretNames . }}
{{- end }}
{{- end }}
{{- dict "fileBacked" $fileBacked "cli" $cli "configFromNames" ($configFromNames | uniq) "secretFromNames" $mountedSecretNames "pluginSecrets" $pluginSecrets "pluginVolumes" $pluginVolumes "needsConfigFile" (gt (len $fileBacked) 0) | toJson }}
{{- end }}

{{/*
The directory in the host container that every host component plugin
volume is mounted under, as <root>/<plugin id>/<volume name>.

`wash host` reserves each plugin volume directory its config names for the
host's whole life, so no workload `hostPath` volume can open one, whatever
`hostPaths.allowed` or an `--allowed-host-path` in `extraArgs` permits.
*/}}
{{- define "runtime-operator.hostPluginVolumeRoot" -}}
/var/lib/wasmcloud/host-plugin-volumes
{{- end }}

{{/*
One host component plugin entry's `volumes`, checked and resolved into the
Pod volumes deployment.yaml renders and the `hostPath` directories the
`wash host` config file names. Returns a JSON array of
`{name, volume, mountPath, readOnly}`:

- `volume` is the Pod volume: the entry's source under a pod-unique name.
- `mountPath` is where the host container sees it, and so the plugin
  volume's `hostPath`: <runtime-operator.hostPluginVolumeRoot>/<id>/<name>.
  That lies outside every directory the host reserves (the OCI cache, its
  config, certificate and credential mounts), and the host keeps workload
  volumes out of it.
- `readOnly` is true unless a `volumeMounts` entry mounts the volume
  writable, so the host container gets no more access than the plugin.
- A `hostPath` source's `path` is written cleaned, as the allowlist judged
  it, since the API server refuses one containing `..`.

Refused here rather than at host start, naming the plugin:

- volumes on a native entry, whose store is the workload's, so a volume
  would preopen nothing;
- `volumes` or `volumeMounts` that is not a list of maps;
- a volume name that is not a DNS-1123 label or is repeated;
- a volume without exactly one Pod volume source, such as `readOnly` set
  on the volume instead of its mount, or a misspelled source;
- `gitRepo` or `flexVolume` sources, which can execute commands on the node;
- a Kubernetes `hostPath` source outside the host group's
  `pluginHostPaths.allowed`, which is empty by default, or of a `type`
  that is not a directory, since the host opens every volume as one;
- a volume no `volumeMounts` entry uses, which the plugin could never
  reach, so it would only sit in the host container;
- a writable mount of a source kubelet mounts read-only: `configMap`,
  `secret`, `downwardAPI`, `projected` and `image` always, and any
  source that sets `readOnly: true` itself;
- a mount naming an undeclared volume or missing `mountPath`, with a
  `mountPath` that is relative or not normalized or that another mount
  uses, with a
  `readOnly` that is not a boolean, or with a key the host config does not
  take (such as `subPath`), which the host would refuse as unknown.

The runtime checks each `mountPath` again when the plugin loads.

Call with (dict "entry" <plugin entry> "group" <host group>).
*/}}
{{- define "runtime-operator.hostPluginVolumes" -}}
{{- $entry := .entry }}
{{- $group := .group }}
{{- $resolved := list }}
{{- if or $entry.volumes $entry.volumeMounts }}
{{- $id := toString $entry.id }}
{{- if not (or $entry.image $entry.file) }}
{{- fail (printf "host plugin %s: `volumes` and `volumeMounts` need a component plugin (an `image` or `file`); a native plugin runs in the workload's store, so its volumes would mount nothing" $id) }}
{{- end }}
{{- if or (not (regexMatch "^[A-Za-z0-9_.-]+$" $id)) (eq $id ".") (eq $id "..") }}
{{- fail (printf "host plugin %q: an entry with `volumes` needs an `id` of letters, digits, `.`, `_` and `-`; it names the directory the host sees the volume in" $id) }}
{{- end }}
{{- range $field := list "volumes" "volumeMounts" }}
{{- $items := index $entry $field }}
{{- if and $items (not (kindIs "slice" $items)) }}
{{- fail (printf "host plugin %s: `%s` must be a list" $id $field) }}
{{- end }}
{{- range $items }}
{{- if not (kindIs "map" .) }}
{{- fail (printf "host plugin %s: each `%s` entry must be a map with a `name`, got %v" $id $field .) }}
{{- end }}
{{- end }}
{{- end }}
{{- $allowedHostPaths := list }}
{{- range (default dict $group.pluginHostPaths).allowed }}
{{- $allowedHostPaths = append $allowedHostPaths (clean (toString .)) }}
{{- end }}
{{- $volumeSources := list "awsElasticBlockStore" "azureDisk" "azureFile" "cephfs" "cinder" "configMap" "csi" "downwardAPI" "emptyDir" "ephemeral" "fc" "flocker" "gcePersistentDisk" "glusterfs" "hostPath" "image" "iscsi" "nfs" "persistentVolumeClaim" "photonPersistentDisk" "portworxVolume" "projected" "quobyte" "rbd" "scaleIO" "secret" "storageos" "vsphereVolume" }}
{{- $readOnlySources := list "configMap" "secret" "downwardAPI" "projected" "image" }}
{{- $writable := dict }}
{{- $declared := dict }}
{{- $mounted := dict }}
{{- $mountPaths := dict }}
{{- range $entry.volumes }}
{{- $name := toString .name }}
{{- if or (gt (len $name) 63) (not (regexMatch "^[a-z0-9]([-a-z0-9]*[a-z0-9])?$" $name)) }}
{{- fail (printf "host plugin %s: volume name %q must be a DNS-1123 label of at most 63 characters, as a Pod volume's is" $id $name) }}
{{- end }}
{{- if hasKey $declared $name }}
{{- fail (printf "host plugin %s: volume %q is declared twice" $id $name) }}
{{- end }}
{{- $sourceKeys := keys (omit . "name") | sortAlpha }}
{{- /* gitRepo has a container escape (CVE-2024-10220), and FlexVolume
       invokes driver executables on the node. Neither is safe here. */}}
{{- range $source := list "gitRepo" "flexVolume" }}
{{- if has $source $sourceKeys }}
{{- fail (printf "host plugin %s: volume %q uses disabled volume source `%s`; use `emptyDir`, `persistentVolumeClaim` or `csi` instead" $id $name $source) }}
{{- end }}
{{- end }}
{{- if or (ne (len $sourceKeys) 1) (not (has (first $sourceKeys) $volumeSources)) }}
{{- fail (printf "host plugin %s: volume %q needs exactly one Pod volume source, such as `persistentVolumeClaim`, `configMap`, `secret` or `emptyDir`, and nothing else; it sets %s" $id $name (default "nothing" (join ", " $sourceKeys))) }}
{{- end }}
{{- /* A node `hostPath` would hand the host container, and through it the
       plugin, any directory on the node. `hostPaths.allowed` cannot gate it:
       those are paths inside the host container, not on the node. */}}
{{- if hasKey . "hostPath" }}
{{- if not (kindIs "map" .hostPath) }}
{{- fail (printf "host plugin %s: volume %q needs `hostPath` to be a map with a `path`, as a Pod volume's is" $id $name) }}
{{- end }}
{{- if not (has (toString (default "" .hostPath.type)) (list "" "Directory" "DirectoryOrCreate")) }}
{{- fail (printf "host plugin %s: volume %q has `hostPath.type: %s`; the host opens a plugin volume as a directory, so use `Directory`, `DirectoryOrCreate` or no type" $id $name .hostPath.type) }}
{{- end }}
{{- $path := toString .hostPath.path }}
{{- if not (hasPrefix "/" $path) }}
{{- fail (printf "host plugin %s: volume %q needs an absolute `hostPath.path`" $id $name) }}
{{- end }}
{{- $path = clean $path }}
{{- $allowed := false }}
{{- range $allowedHostPaths }}
{{- if or (eq $path .) (hasPrefix (printf "%s/" (trimSuffix "/" .)) $path) }}
{{- $allowed = true }}
{{- end }}
{{- end }}
{{- if not $allowed }}
{{- fail (printf "host plugin %s: volume %q mounts node path %s, which is not within this host group's `pluginHostPaths.allowed`" $id $name $path) }}
{{- end }}
{{- end }}
{{- $_ := set $declared $name . }}
{{- end }}
{{- range $entry.volumeMounts }}
{{- $name := toString .name }}
{{- if not (hasKey $declared $name) }}
{{- fail (printf "host plugin %s: volumeMount %q names no volume in this entry's `volumes`" $id $name) }}
{{- end }}
{{- if not .mountPath }}
{{- fail (printf "host plugin %s: volumeMount %q needs a `mountPath`, where the plugin sees the volume" $id $name) }}
{{- end }}
{{- $mountPath := toString .mountPath }}
{{- if or (not (hasPrefix "/" $mountPath)) (ne (clean $mountPath) $mountPath) }}
{{- fail (printf "host plugin %s: volumeMount %q has `mountPath: %s`; it must be absolute and normalized, with no `.`, `..`, repeated or trailing `/`" $id $name $mountPath) }}
{{- end }}
{{- if hasKey $mountPaths $mountPath }}
{{- fail (printf "host plugin %s: volumeMounts %q and %q both mount at %s" $id (index $mountPaths $mountPath) $name $mountPath) }}
{{- end }}
{{- $_ := set $mountPaths $mountPath $name }}
{{- $_ := set $mounted $name true }}
{{- with omit . "name" "mountPath" "readOnly" }}
{{- fail (printf "host plugin %s: volumeMount %q sets %s; a plugin mount takes only `name`, `mountPath` and `readOnly`" $id $name (keys . | sortAlpha | join ", ")) }}
{{- end }}
{{- if and (hasKey . "readOnly") (not (kindIs "bool" .readOnly)) }}
{{- fail (printf "host plugin %s: volumeMount %q has `readOnly: %q`, which must be the boolean true or false" $id $name (toString .readOnly)) }}
{{- end }}
{{- if not .readOnly }}
{{- $source := first (keys (omit (index $declared $name) "name")) }}
{{- if has $source $readOnlySources }}
{{- fail (printf "host plugin %s: volumeMount %q is writable, but its volume is a `%s`, which Kubernetes always mounts read-only; set `readOnly: true`" $id $name $source) }}
{{- end }}
{{- $sourceSpec := index (index $declared $name) $source }}
{{- if and (kindIs "map" $sourceSpec) (eq (index $sourceSpec "readOnly") true) }}
{{- fail (printf "host plugin %s: volumeMount %q is writable, but its `%s` source sets `readOnly: true`; set `readOnly: true` on the mount too" $id $name $source) }}
{{- end }}
{{- $_ := set $writable $name true }}
{{- end }}
{{- end }}
{{- range $entry.volumes }}
{{- if not (hasKey $mounted (toString .name)) }}
{{- fail (printf "host plugin %s: volume %q is declared but no `volumeMounts` entry uses it; mount it or remove it" $id (toString .name)) }}
{{- end }}
{{- end }}
{{- range $entry.volumes }}
{{- $name := toString .name }}
{{- /* Pod-unique, and a DNS-1123 label: the hash covers the plugin id, so two
       plugins may each declare a volume of the same name. */}}
{{- $podName := printf "hpv-%s-%s" (trunc 48 $name | trimSuffix "-") (sha256sum (printf "%s/%s" $id $name) | trunc 10) }}
{{- $volume := merge (dict "name" $podName) (omit . "name") }}
{{- if hasKey . "hostPath" }}
{{- $_ := set $volume "hostPath" (merge (dict "path" (clean (toString .hostPath.path))) (omit .hostPath "path")) }}
{{- end }}
{{- $dir := printf "%s/%s/%s" (include "runtime-operator.hostPluginVolumeRoot" .) $id $name }}
{{- $resolved = append $resolved (dict "name" $name "volume" $volume "mountPath" $dir "readOnly" (not (hasKey $writable $name))) }}
{{- end }}
{{- end }}
{{- $resolved | toJson }}
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
{{- $pluginSecrets := $partition.pluginSecrets }}
{{- if $configNames }}
configs:
  {{- range $configNames }}
  {{ . }}:
    dir: /etc/wasmcloud/host-plugin-config/{{ . }}
  {{- end }}
{{- end }}
{{- if or $secretNames $pluginSecrets }}
secrets:
  {{- range $secretNames }}
  {{ . }}:
    dir: /etc/wasmcloud/host-plugin-secrets/{{ . }}
  {{- end }}
  {{- range $name, $source := $pluginSecrets }}
  {{ $name }}:
    plugin: {{ required (printf "pluginSecrets.%s needs a `plugin`" $name) $source.plugin }}
    keys:
      {{- required (printf "pluginSecrets.%s needs `keys`" $name) $source.keys | toYaml | nindent 6 }}
    workloads:
      {{- required (printf "pluginSecrets.%s needs `workloads`: the namespaces (and optionally workload names) that may use it, or `all`" $name) $source.workloads | toYaml | nindent 6 }}
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
      {{- with .allowedHostLoopbackPorts }}
      allowedHostLoopbackPorts:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with .ports }}
      ports:
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
      {{- with .resolvedVolumes }}
      volumes:
        {{- range . }}
        - name: {{ .name | quote }}
          hostPath:
            path: {{ .mountPath | quote }}
        {{- end }}
      {{- end }}
      {{- with .volumeMounts }}
      volumeMounts:
        {{- range . }}
        - name: {{ .name | toString | quote }}
          mountPath: {{ .mountPath | quote }}
          {{- if .readOnly }}
          readOnly: true
          {{- end }}
        {{- end }}
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
One host-group probe, resolved against the chart-wide default. Fields resolve
one by one, so a group retuning one timing keeps the rest.

Written as a key-by-key overlay rather than `merge` because sprig's merge
treats every zero as unset: it would drop a group's `enabled: false` and its
`initialDelaySeconds: 0` alike, in favour of whatever the chart-wide block
says.

Call with (dict "group" .probes "chart" $top.Values.runtime.probes "name" "liveness")
and read the result back with `fromJson`.
*/}}
{{- define "runtime-operator.hostProbe" -}}
{{- $probe := dict }}
{{- range $field, $value := (index (.chart | default dict) .name) | default dict }}
{{- $_ := set $probe $field $value }}
{{- end }}
{{- range $field, $value := (index (.group | default dict) .name) | default dict }}
{{- $_ := set $probe $field $value }}
{{- end }}
{{- $probe | toJson }}
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

{{/*
A host env value from a chart value that may be a bare number. A YAML number
arrives as a float64 and would print as `2.68435456e+08`, which the host
cannot parse, so numbers render as integers and anything else as is.
*/}}
{{- define "runtime-operator.envNumber" -}}
{{- if or (kindIs "int64" .) (kindIs "float64" .) }}{{ int64 . }}{{ else }}{{ . }}{{ end }}
{{- end }}

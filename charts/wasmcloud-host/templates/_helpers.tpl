{{/*
Expand the name of the chart.
*/}}
{{- define "wasmcloud-host.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
We truncate at 63 chars because some Kubernetes name fields are limited to this (by the DNS naming spec).
If release name contains chart name it will be used as a full name.
*/}}
{{- define "wasmcloud-host.fullname" -}}
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

{{- define "wasmcloud-host.nats-config-name" -}}
{{- .Release.Name | trunc 51 | trimSuffix "-" }}-nats-config
{{- end }}

{{/*
Create chart name and version as used by the chart label.
*/}}
{{- define "wasmcloud-host.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "wasmcloud-host.labels" -}}
helm.sh/chart: {{ include "wasmcloud-host.chart" . }}
{{ include "wasmcloud-host.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "wasmcloud-host.selectorLabels" -}}
app.kubernetes.io/name: {{ include "wasmcloud-host.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Create the name of the service account to use
*/}}
{{- define "wasmcloud-host.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "wasmcloud-host.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "wasmcloud-host.allowed-insecure" -}}
{{- $list := list }}
{{- range .Values.config.allowedInsecure }}
{{- $list = append $list . }}
{{- end }}
{{- join "," $list }}
{{- end }}

{{- define "wasmcloud-host.nats.address" -}}
{{- if .Values.config.natsAddress }}
url: {{ .Values.config.natsAddress | quote }}
{{- else }}
url: "nats://nats-headless.{{ .Release.Namespace }}.svc.cluster.local"
{{- end }}
{{- end }}
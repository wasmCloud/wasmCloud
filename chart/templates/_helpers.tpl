{{/*
Expand the name of the chart.
*/}}
{{- define "wasmcloud_host.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
We truncate at 63 chars because some Kubernetes name fields are limited to this (by the DNS naming spec).
If release name contains chart name it will be used as a full name.
*/}}
{{- define "wasmcloud_host.fullname" -}}
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
{{- define "wasmcloud_host.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "wasmcloud_host.labels" -}}
helm.sh/chart: {{ include "wasmcloud_host.chart" . }}
{{ include "wasmcloud_host.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels wasmcloud_host
*/}}
{{- define "wasmcloud_host.selectorLabels" -}}
app.kubernetes.io/name: {{ include "wasmcloud_host.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Selector labels wadm
*/}}
{{- define "wadm.selectorLabels" -}}
app.kubernetes.io/name: {{ include "wasmcloud_host.name" . }}
app.kubernetes.io/instance: wadm
{{- end }}


{{/*
Create the name of the service account to use
*/}}
{{- define "wasmcloud_host.serviceAccountName" -}}
{{- if or .Values.serviceAccount.create .Values.wasmcloud.enableApplierSupport }}
{{- default (include "wasmcloud_host.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

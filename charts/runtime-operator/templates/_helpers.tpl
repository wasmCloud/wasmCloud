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
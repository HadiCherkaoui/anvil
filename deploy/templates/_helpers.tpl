{{/*
Common helpers for the anvil chart.
*/}}

{{/* Name of the chart (truncated to 63 chars per DNS-1123). */}}
{{- define "anvil.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* Fully-qualified release name (truncated to 63 chars). */}}
{{- define "anvil.fullname" -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{/* Common labels applied to every resource. */}}
{{- define "anvil.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
app.kubernetes.io/name: {{ include "anvil.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: anvil
{{- end -}}

{{/* Selector labels (subset of `labels`, used for matchLabels). */}}
{{- define "anvil.selectorLabels" -}}
app.kubernetes.io/name: {{ include "anvil.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{/* ServiceAccount name. Honors `.Values.rbac.serviceAccountName` override. */}}
{{- define "anvil.serviceAccountName" -}}
{{- if .Values.rbac.serviceAccountName -}}
{{- .Values.rbac.serviceAccountName -}}
{{- else -}}
{{- include "anvil.fullname" . -}}
{{- end -}}
{{- end -}}

{{/* Container image reference: defaults image.tag to .Chart.AppVersion. */}}
{{- define "anvil.image" -}}
{{- $tag := default .Chart.AppVersion .Values.image.tag -}}
{{- printf "%s:%s" .Values.image.repo $tag -}}
{{- end -}}

{{/* Hard-fail at template time if a required cluster-portability value
     is missing. Forces the operator to set mcDefaults.storageClassName
     instead of letting the chart silently use a wrong default. */}}
{{- define "anvil.requireMcDefaults" -}}
{{- if not .Values.mcDefaults.storageClassName -}}
{{- fail "mcDefaults.storageClassName is required (see docs/cluster-profile.md for legal values; e.g. `tank` on the homelab cluster)" -}}
{{- end -}}
{{- end -}}

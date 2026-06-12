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

{{/* Container image reference. Defaults image.tag to `latest` — the tag CI
     actually publishes alongside :<commit-sha>. (Chart.appVersion is not
     pushed as an image tag, so using it here yields ImagePullBackOff.)
     Pin image.tag to a commit SHA in production for rollback. */}}
{{- define "anvil.image" -}}
{{- $tag := default "latest" .Values.image.tag -}}
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

{{/* Hard-fail when OIDC is disabled or misconfigured. The backend refuses
     to start without the OIDC env vars, so oidc.enabled=false would only
     produce a crash-looping pod — fail at render instead. The cookie
     security flags are pointless over plain HTTP and Authentik will reject
     http:// redirect URIs in any case. */}}
{{- define "anvil.requireOidc" -}}
{{- if not .Values.oidc.enabled -}}
  {{- fail "anvil requires OIDC — the backend refuses to start without it; oidc.enabled=false is not supported" -}}
{{- end -}}
{{- if .Values.oidc.enabled -}}
  {{- if not .Values.ingress.tls.enabled -}}
    {{- fail "oidc.enabled requires ingress.tls.enabled (HTTPS is mandatory for the OIDC redirect_uri and Secure cookie flags)." -}}
  {{- end -}}
  {{- if not .Values.oidc.issuerUrl -}}
    {{- fail "oidc.issuerUrl is required when oidc.enabled (e.g. https://authentik.example/application/o/anvil/)" -}}
  {{- end -}}
  {{- if not .Values.oidc.clientId -}}
    {{- fail "oidc.clientId is required when oidc.enabled" -}}
  {{- end -}}
  {{- if not .Values.oidc.redirectUrl -}}
    {{- fail "oidc.redirectUrl is required when oidc.enabled (e.g. https://anvil.example/api/auth/callback)" -}}
  {{- end -}}
  {{- if and (not .Values.oidc.existingSecret) (not .Values.oidc.clientSecret) -}}
    {{- fail "oidc.clientSecret (or oidc.existingSecret) is required when oidc.enabled" -}}
  {{- end -}}
  {{- if and (not .Values.oidc.existingSecret) (not .Values.oidc.sessionKey) -}}
    {{- fail "oidc.sessionKey (or oidc.existingSecret) is required when oidc.enabled — generate with: openssl rand -base64 32" -}}
  {{- end -}}
{{- end -}}
{{- end -}}

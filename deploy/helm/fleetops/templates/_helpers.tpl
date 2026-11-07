{{- define "fleetops.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
The release's resource name. A release named after the chart (or containing
it) gets just "fleetops", which the generated TLS certificate covers.
*/}}
{{- define "fleetops.fullname" -}}
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

{{- define "fleetops.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "fleetops.selectorLabels" -}}
app.kubernetes.io/name: {{ include "fleetops.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "fleetops.labels" -}}
helm.sh/chart: {{ include "fleetops.chart" . }}
{{ include "fleetops.selectorLabels" . }}
app.kubernetes.io/version: {{ .Values.image.tag | default .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/component: server
{{- end }}

{{- define "fleetops.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "fleetops.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "fleetops.image" -}}
{{- if .Values.image.digest }}
{{- printf "%s@%s" .Values.image.repository .Values.image.digest }}
{{- else }}
{{- printf "%s:%s" .Values.image.repository (.Values.image.tag | default .Chart.AppVersion) }}
{{- end }}
{{- end }}

{{- define "fleetops.claimName" -}}
{{- default (include "fleetops.fullname" .) .Values.persistence.existingClaim }}
{{- end }}

{{/* FLEETOPS_PUBLIC_URL: the value given, else the first TLS ingress host. */}}
{{- define "fleetops.publicUrl" -}}
{{- if .Values.publicUrl }}
{{- .Values.publicUrl }}
{{- else if and .Values.ingress.enabled .Values.ingress.tls }}
{{- with (first .Values.ingress.tls) }}{{ with .hosts }}{{ printf "https://%s" (first .) }}{{ end }}{{ end }}
{{- end }}
{{- end }}

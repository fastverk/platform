{{- define "service-finder.name" -}}
service-finder
{{- end -}}

{{- define "service-finder.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
service-finder
{{- end -}}
{{- end -}}

{{- define "service-finder.labels" -}}
app.kubernetes.io/name: {{ include "service-finder.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end -}}

{{- define "service-finder.selectorLabels" -}}
app.kubernetes.io/name: {{ include "service-finder.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "service-finder.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "service-finder.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

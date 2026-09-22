{{- define "aether-gateway.name" -}}
aether-gateway
{{- end -}}

{{- define "aether-gateway.fullname" -}}
{{- printf "%s" .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "aether-gateway.labels" -}}
app.kubernetes.io/name: {{ include "aether-gateway.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{- define "aether-gateway.image" -}}
{{- $tag := .Values.image.tag | default .Chart.AppVersion -}}
{{- if .Values.image.digest -}}
"{{ .Values.image.repository }}@{{ .Values.image.digest }}"
{{- else -}}
"{{ .Values.image.repository }}:{{ $tag }}"
{{- end -}}
{{- end -}}

{{- define "aether-gateway.env" -}}
- name: AETHER_GATEWAY_DEPLOYMENT_TOPOLOGY
  value: {{ .Values.gateway.deploymentTopology | quote }}
- name: AETHER_DATABASE_DRIVER
  value: "postgres"
- name: AETHER_DATABASE_URL
  value: {{ .Values.database.url | quote }}
- name: AETHER_GATEWAY_DATA_POSTGRES_MIN_CONNECTIONS
  value: {{ .Values.database.minConnections | quote }}
- name: AETHER_GATEWAY_DATA_POSTGRES_MAX_CONNECTIONS
  value: {{ .Values.database.maxConnections | quote }}
- name: AETHER_GATEWAY_DATA_POSTGRES_REQUIRE_SSL
  value: {{ .Values.database.requireSsl | quote }}
- name: AETHER_GATEWAY_DATA_REDIS_URL
  value: {{ .Values.redis.url | quote }}
- name: AETHER_RUNTIME_BACKEND
  value: {{ .Values.gateway.runtimeBackend | quote }}
- name: AETHER_GATEWAY_DATABASE_MODE
  value: {{ .Values.gateway.databaseMode | quote }}
- name: AETHER_CONSISTENCY_FIRST
  value: {{ .Values.gateway.consistencyFirst | quote }}
- name: AETHER_GATEWAY_TRUSTED_INGRESS_CIDRS
  value: {{ .Values.gateway.trustedIngressCidrs | quote }}
- name: AETHER_TUNNEL_RELAY_BASE_URL
  value: {{ .Values.gateway.tunnelRelayBaseUrl | quote }}
- name: AETHER_LOG_DESTINATION
  value: "stdout"
- name: AETHER_LOG_FORMAT
  value: {{ .Values.gateway.logFormat | quote }}
- name: ENVIRONMENT
  value: "production"
- name: APP_PORT
  value: "8084"
- name: JWT_SECRET_KEY
  valueFrom:
    secretKeyRef:
      name: {{ include "aether-gateway.fullname" . }}
      key: jwt-secret-key
- name: ENCRYPTION_KEY
  valueFrom:
    secretKeyRef:
      name: {{ include "aether-gateway.fullname" . }}
      key: encryption-key
{{- end -}}

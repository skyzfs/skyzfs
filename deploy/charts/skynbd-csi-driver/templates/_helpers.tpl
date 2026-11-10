{{- define "skynbd-csi.fullname" -}}
{{- printf "%s" .Release.Name | trunc 53 | trimSuffix "-" -}}
{{- end -}}

{{- define "skynbd-csi.labels" -}}
app.kubernetes.io/name: skynbd-csi-driver
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{- define "skynbd-csi.apiUrl" -}}
{{- if .Values.api.url -}}
{{ .Values.api.url }}
{{- else if .Values.devServer.enabled -}}
http://{{ include "skynbd-csi.fullname" . }}-dev:8080
{{- else -}}
{{ fail "api.url is required unless devServer.enabled" }}
{{- end -}}
{{- end -}}

{{- define "skynbd-csi.nbdHost" -}}
{{- if .Values.nbd.host -}}
{{ .Values.nbd.host }}
{{- else if .Values.devServer.enabled -}}
{{ include "skynbd-csi.fullname" . }}-dev.{{ .Release.Namespace }}.svc
{{- else -}}
{{ fail "nbd.host is required unless devServer.enabled or nbd.socket.enabled" }}
{{- end -}}
{{- end -}}

{{/*
Unix data path (CSI design §2.1). mTLS binds to a client certificate,
which a socket connection does not carry — so the two are mutually
exclusive by construction, and saying so here beats discovering it as a
plaintext attach in production.
*/}}
{{- define "skynbd-csi.checkSocket" -}}
{{- if and .Values.nbd.socket.enabled .Values.nbd.tls.enabled -}}
{{ fail "nbd.socket.enabled requires nbd.tls.enabled=false: a Unix data path is guarded by the socket's file mode, not by client certificates" }}
{{- end -}}
{{- end -}}

{{/* Directory holding the NBD socket — that is what gets hostPath-mounted. */}}
{{- define "skynbd-csi.nbdSocketDir" -}}
{{ .Values.nbd.socket.path | dir }}
{{- end -}}

{{- define "skynbd-csi.tokenVolume" -}}
{{- if .Values.auth.tokenAudience }}
- name: skynbd-token
  projected:
    sources:
      - serviceAccountToken:
          path: token
          audience: {{ .Values.auth.tokenAudience | quote }}
          expirationSeconds: {{ .Values.auth.tokenExpirationSeconds }}
{{- end }}
{{- end -}}

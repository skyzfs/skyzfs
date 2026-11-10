#!/usr/bin/env bash
# NBD STARTTLS smoke (auth design §3; design §8.1's "one smoke scenario").
#
# Real-client interop against forced-TLS skynbdd: qemu-io (GnuTLS)
# upgrades via NBD_OPT_STARTTLS with an mTLS client cert and does a
# write/read round-trip; nbdinfo lists over TLS; plaintext clients must
# be refused. PKI is a throwaway openssl CA generated per run — the
# client cert carries the SPIFFE-style URI SAN the ACL matches.
#
# Runs in nightly CI (job tls-smoke) and by hand: tests/e2e/tls_smoke.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

need() { for t in "$@"; do command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 64; }; done; }
need qemu-io openssl curl

WORK=$(mktemp -d)
DAEMON_PID=""
cleanup() {
  rc=${rc:-$?}
  [ -z "$DAEMON_PID" ] || kill -9 "$DAEMON_PID" 2>/dev/null || true
  if [ "$rc" -ne 0 ]; then
    KEEP="/var/tmp/skynbd-tls-smoke-failed"
    rm -rf "$KEEP"
    { mkdir -p "$KEEP" && cp -r "$WORK"/. "$KEEP/" 2>/dev/null; } || true
    echo "tls smoke FAILED (rc=$rc): evidence under $KEEP" >&2
    tail -50 "$WORK/daemon.log" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

PORT=$((20000 + RANDOM % 20000))
API_PORT=$((PORT + 1))

# ---- throwaway PKI (EC P-256, PKCS8 keys) -----------------------------
PKI="$WORK/pki"
mkdir -p "$PKI"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout "$PKI/ca.key" -out "$PKI/ca.pem" -days 2 \
  -subj "/CN=skynbd-smoke-ca" 2>/dev/null

gen_cert() { # <name> <subject> <san> <eku>
  openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
    -keyout "$PKI/$1.key" -out "$PKI/$1.csr" -subj "$2" 2>/dev/null
  openssl x509 -req -in "$PKI/$1.csr" -CA "$PKI/ca.pem" -CAkey "$PKI/ca.key" \
    -CAcreateserial -days 2 -out "$PKI/$1.pem" \
    -extfile <(printf "subjectAltName=%s\nextendedKeyUsage=%s\nkeyUsage=digitalSignature" \
      "$3" "$4") 2>/dev/null
}
gen_cert server "/CN=skynbd-server" "IP:127.0.0.1,DNS:localhost" serverAuth
gen_cert client "/CN=skynbd-smoke-client" "URI:spiffe://smoke/qemu" clientAuth

# qemu/libnbd x509 creds directory layout.
CREDS="$WORK/creds"
mkdir -p "$CREDS"
cp "$PKI/ca.pem" "$CREDS/ca-cert.pem"
cp "$PKI/client.pem" "$CREDS/client-cert.pem"
cp "$PKI/client.key" "$CREDS/client-key.pem"

# ---- forced-TLS daemon, memory store ----------------------------------
cat > "$WORK/skynbd.toml" <<EOF
nbd_listen = "127.0.0.1:${PORT}"
api_listen = "127.0.0.1:${API_PORT}"

[store]
kind = "memory"

[nbd.tls]
cert_path = "${PKI}/server.pem"
key_path = "${PKI}/server.key"
client_ca_path = "${PKI}/ca.pem"

[[nbd.acl]]
identity = "spiffe://smoke/*"
exports = ["vol1/head"]
write = true

[[volumes]]
name = "vol1"
size = 67108864
EOF

[ -n "${E2E_SKIP_BUILD:-}" ] || cargo build -q -p skynbdd
./target/debug/skynbdd "$WORK/skynbd.toml" > "$WORK/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 50); do
  if curl -sf "http://127.0.0.1:${API_PORT}/healthz" >/dev/null 2>&1; then break; fi
  sleep 0.2
done

IMG="driver=nbd,host=127.0.0.1,port=${PORT},export=vol1/head"

echo "-- qemu-io mTLS write/read round-trip (GnuTLS STARTTLS)"
qemu-io --object "tls-creds-x509,id=tls0,dir=${CREDS},endpoint=client" \
  --image-opts "${IMG},tls-creds=tls0" \
  -c 'write -P 0xab 0 65536' -c 'read -P 0xab 0 65536'

echo "-- plaintext qemu-io is refused (FORCEDTLS)"
if qemu-io --image-opts "$IMG" -c 'read 0 512' 2>"$WORK/plain.err"; then
  echo "plaintext attach unexpectedly succeeded" >&2
  exit 1
fi

if command -v nbdinfo >/dev/null; then
  echo "-- nbdinfo export list over TLS (libnbd)"
  nbdinfo --list "nbds://127.0.0.1:${PORT}/?tls-certificates=${CREDS}" \
    | tee "$WORK/nbdinfo.out"
  grep -q "vol1/head" "$WORK/nbdinfo.out"

  echo "-- plaintext nbdinfo is refused"
  if nbdinfo --list "nbd://127.0.0.1:${PORT}" 2>/dev/null; then
    echo "plaintext nbdinfo unexpectedly succeeded" >&2
    exit 1
  fi
fi

echo "TLS smoke OK"

#!/usr/bin/env bash
# k3s UDS smoke (CSI design §2.1): the node-local Unix data path.
# skynbdd runs as a DaemonSet binding its NBD listener to a hostPath
# socket, and the node plugin attaches with `nbd-client -unix` — no TCP
# stack in the data path and no enrollment, because access control is
# the socket file's mode.
#
# The proof is structural: in this profile skynbdd binds *only* the
# socket (asserted against the rendered config below), so a pod that
# reads back what it wrote can only have gone through it.
#
# Expects: a running cluster (KUBECONFIG set), helm + kubectl on PATH,
# and the two images already present in the cluster's runtime:
#   ${CSI_IMAGE:-docker.io/library/skynbd-csi-driver:smoke}
#   ${SKYNBDD_IMAGE:-docker.io/library/skynbdd:smoke}
set -euo pipefail
cd "$(dirname "$0")/../.."

NS=${NS:-skynbd-uds}
RELEASE=${RELEASE:-uds}
SOCKET=${SOCKET:-/run/skynbd/nbd.sock}
CSI_IMAGE=${CSI_IMAGE:-docker.io/library/skynbd-csi-driver:smoke}
SKYNBDD_IMAGE=${SKYNBDD_IMAGE:-docker.io/library/skynbdd:smoke}

evidence() {
    echo "==== k3s UDS smoke FAILED: evidence ===="
    kubectl get pods,pvc,pv -A -o wide || true
    for pod in uds-writer uds-reader; do
        kubectl -n "$NS" describe pod "$pod" 2>/dev/null | tail -40 || true
    done
    echo "---- host socket ----"
    sudo ls -l "$SOCKET" 2>/dev/null || true
    for sel in controller node dev-server; do
        echo "---- logs: $sel ----"
        kubectl -n "$NS" logs -l app.kubernetes.io/component=$sel \
            --all-containers --tail=60 --prefix 2>/dev/null || true
    done
}
trap 'evidence' ERR

# `exit 1` inside an if-branch does not trip the ERR trap, so loud
# failures must dump evidence themselves.
fail() {
    echo "$*" >&2
    evidence
    exit 1
}

# snapshots.enabled=false: the transport is what is under test, and the
# snapshot CRDs are not installed in this cluster — the csi-snapshotter
# sidecar would sit on a missing API for the whole run.
helm install "$RELEASE" deploy/charts/skynbd-csi-driver \
    --namespace "$NS" --create-namespace \
    --set devServer.enabled=true \
    --set devServer.kind=DaemonSet \
    --set devServer.image="$SKYNBDD_IMAGE" \
    --set nbd.socket.enabled=true \
    --set nbd.socket.path="$SOCKET" \
    --set nbd.tls.enabled=false \
    --set auth.tokenAudience="" \
    --set image.repository="${CSI_IMAGE%:*}" \
    --set image.tag="${CSI_IMAGE##*:}" \
    --set snapshots.enabled=false

kubectl -n "$NS" rollout status "ds/$RELEASE-dev" --timeout=180s
kubectl -n "$NS" rollout status "deploy/$RELEASE-controller" --timeout=180s
kubectl -n "$NS" rollout status "ds/$RELEASE-node" --timeout=180s

echo "-- server binds the socket and nothing else"
kubectl -n "$NS" get configmap "$RELEASE-dev-config" \
    -o jsonpath='{.data.skynbd\.toml}' | grep -q "^nbd_listen = \"unix:$SOCKET\"$" \
    || fail "dev server is not configured for a Unix-only NBD listener"
for _ in $(seq 1 30); do
    sudo test -S "$SOCKET" && break
    sleep 2
done
sudo test -S "$SOCKET" || fail "dev server never bound $SOCKET on the host"

echo "-- provision + mount + write over the socket"
kubectl -n "$NS" apply -f tests/k3s/uds-pvc-pod.yaml
kubectl -n "$NS" wait --for=condition=Ready pod/uds-writer --timeout=300s
kubectl -n "$NS" exec uds-writer -- \
    sh -c 'echo skynbd-uds-ok > /data/probe && sync && cat /data/probe' \
    | grep -q skynbd-uds-ok
echo "-- pod IO ok"

# The node plugin records the endpoint it attached through; anything but
# a unix: endpoint here means the topology silently fell back to TCP.
# Strip SGR escapes before matching: the assertion must hold whatever the
# driver's log colouring is configured to do, and a coloured `field=value`
# puts escape codes between the name, the `=` and the value.
kubectl -n "$NS" logs -l app.kubernetes.io/component=node -c driver --tail=-1 \
    | sed 's/\x1b\[[0-9;]*m//g' \
    | grep -q "endpoint=unix:$SOCKET" \
    || fail "node plugin did not attach over $SOCKET"

echo "-- unstage + restage"
kubectl -n "$NS" delete pod uds-writer --timeout=180s
for _ in $(seq 1 45); do
    ls /sys/block/nbd*/pid >/dev/null 2>&1 || break
    sleep 2
done
if ls /sys/block/nbd*/pid >/dev/null 2>&1; then
    fail "NBD device still attached after unstage"
fi
kubectl -n "$NS" apply -f tests/k3s/uds-pod2.yaml
kubectl -n "$NS" wait --for=condition=Ready pod/uds-reader --timeout=300s
kubectl -n "$NS" exec uds-reader -- cat /data/probe | grep -q skynbd-uds-ok
echo "-- restage content ok"

echo "-- teardown + reclaim"
kubectl -n "$NS" delete pod uds-reader --timeout=180s
kubectl -n "$NS" delete pvc uds-pvc --timeout=180s
# reclaimPolicy Delete: the PV (and the server-side volume) must go away.
for _ in $(seq 1 45); do
    [ -z "$(kubectl get pv -o name 2>/dev/null)" ] && break
    sleep 2
done
if [ -n "$(kubectl get pv -o name 2>/dev/null)" ]; then
    fail "PV not reclaimed after delete"
fi

echo "k3s UDS smoke OK"

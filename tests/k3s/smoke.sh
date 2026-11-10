#!/usr/bin/env bash
# k3s install smoke (CSI design 6c): helm-install the chart with the
# devServer profile (in-cluster memory-store skynbdd, plaintext NBD,
# auth off) and prove the full CSI machinery: dynamic provisioning →
# NodeStage (nbd-client attach + mkfs + mount) → pod IO → teardown →
# PV reclaim (server-side volume delete).
#
# Expects: a running cluster (KUBECONFIG set), helm + kubectl on PATH,
# and the two images already present in the cluster's runtime:
#   ${CSI_IMAGE:-docker.io/library/skynbd-csi-driver:smoke}
#   ${SKYNBDD_IMAGE:-docker.io/library/skynbdd:smoke}
set -euo pipefail
cd "$(dirname "$0")/../.."

NS=${NS:-skynbd-smoke}
RELEASE=${RELEASE:-smoke}
CSI_IMAGE=${CSI_IMAGE:-docker.io/library/skynbd-csi-driver:smoke}
SKYNBDD_IMAGE=${SKYNBDD_IMAGE:-docker.io/library/skynbdd:smoke}

evidence() {
    echo "==== k3s smoke FAILED: evidence ===="
    kubectl get pods,pvc,pv -A -o wide || true
    kubectl get volumesnapshot,volumesnapshotcontent -A 2>/dev/null || true
    for pod in smoke-writer smoke-reader smoke-cloner smoke-thin-writer smoke-thin-reader smoke-thin-cloner; do
        kubectl -n "$NS" describe pod "$pod" 2>/dev/null | tail -40 || true
    done
    echo "---- dm devices ----"
    sudo dmsetup ls 2>/dev/null || true
    losetup -a 2>/dev/null || true
    for sel in controller node dev-server; do
        echo "---- logs: $sel ----"
        kubectl -n "$NS" logs -l app.kubernetes.io/component=$sel \
            --all-containers --tail=60 --prefix 2>/dev/null || true
    done
}
trap 'evidence' ERR

# Manual checks: `exit 1` inside an if-branch does not trip the ERR
# trap, so loud failures must dump evidence themselves.
fail() {
    echo "$*" >&2
    evidence
    exit 1
}

helm install "$RELEASE" deploy/charts/skynbd-csi-driver \
    --namespace "$NS" --create-namespace \
    --set devServer.enabled=true \
    --set devServer.image="$SKYNBDD_IMAGE" \
    --set nbd.tls.enabled=false \
    --set auth.tokenAudience="" \
    --set image.repository="${CSI_IMAGE%:*}" \
    --set image.tag="${CSI_IMAGE##*:}" \
    --set node.cache.enabled=true \
    --set node.cache.sizeBytes="268435456" \
    --set node.cache.readCache.enabled=true \
    --set node.cache.readCache.sizeBytes="67108864"

kubectl -n "$NS" rollout status "deploy/$RELEASE-dev" --timeout=180s
kubectl -n "$NS" rollout status "deploy/$RELEASE-controller" --timeout=180s
kubectl -n "$NS" rollout status "ds/$RELEASE-node" --timeout=180s

echo "-- provision + mount + write"
kubectl -n "$NS" apply -f tests/k3s/pvc-pod.yaml
kubectl -n "$NS" wait --for=condition=Ready pod/smoke-writer --timeout=300s
kubectl -n "$NS" exec smoke-writer -- \
    sh -c 'echo skynbd-smoke-ok > /data/probe && sync && cat /data/probe' \
    | grep -q skynbd-smoke-ok
echo "-- pod IO ok"

echo "-- snapshot (quiesced)"
# Raw-mode snapshots are crash-consistent (no freeze hook); delete the
# writer pod first so the filesystem unmounts cleanly and the RO-mount
# content assertion below is deterministic, then wait for the node
# plugin's detach (kubelet unstages asynchronously after pod deletion —
# an attached kernel NBD device is visible as a sysfs pid file).
kubectl -n "$NS" delete pod smoke-writer --timeout=180s
for _ in $(seq 1 45); do
    ls /sys/block/nbd*/pid >/dev/null 2>&1 || break
    sleep 2
done
kubectl -n "$NS" apply -f tests/k3s/snapshot.yaml
for _ in $(seq 1 60); do
    ready=$(kubectl -n "$NS" get volumesnapshot smoke-snap \
        -o jsonpath='{.status.readyToUse}' 2>/dev/null || true)
    [ "$ready" = "true" ] && break
    sleep 2
done
[ "$ready" = "true" ] || fail "snapshot never became ready"

echo "-- RO snapshot mount"
kubectl -n "$NS" apply -f tests/k3s/ro-pod.yaml
kubectl -n "$NS" wait --for=condition=Ready pod/smoke-reader --timeout=300s
kubectl -n "$NS" exec smoke-reader -- cat /data/probe | grep -q skynbd-smoke-ok
if kubectl -n "$NS" exec smoke-reader -- sh -c 'echo x > /data/nope' 2>/dev/null; then
    fail "RO snapshot mount accepted a write"
fi
echo "-- RO content ok"

echo "-- writable clone"
# RW PVC from the same snapshot: a server fast clone (CSI design 6g).
# It must start at the snapshot's content, accept writes, and diverge
# without disturbing the source: the RO reader (still mounted on the
# snapshot itself) must keep seeing exactly the original content.
kubectl -n "$NS" apply -f tests/k3s/clone-pod.yaml
kubectl -n "$NS" wait --for=condition=Ready pod/smoke-cloner --timeout=300s
kubectl -n "$NS" exec smoke-cloner -- cat /data/probe | grep -q skynbd-smoke-ok
kubectl -n "$NS" exec smoke-cloner -- \
    sh -c 'echo clone-diverged > /data/probe && echo new > /data/clone-only && sync'
kubectl -n "$NS" exec smoke-cloner -- cat /data/probe | grep -q clone-diverged
kubectl -n "$NS" exec smoke-reader -- cat /data/probe | grep -q skynbd-smoke-ok
if kubectl -n "$NS" exec smoke-reader -- test -e /data/clone-only 2>/dev/null; then
    fail "clone write leaked into the source snapshot"
fi
echo "-- clone diverged, source snapshot unchanged"

echo "-- thin-mode volume (publish-only local thin pool)"
kubectl apply -f tests/k3s/thin-sc.yaml
kubectl -n "$NS" apply -f tests/k3s/thin-pvc-pod.yaml
kubectl -n "$NS" wait --for=condition=Ready pod/smoke-thin-writer --timeout=300s
kubectl -n "$NS" exec smoke-thin-writer -- \
    sh -c 'echo skynbd-thin-ok > /data/probe && sync && cat /data/probe' \
    | grep -q skynbd-thin-ok
# The stack really is dm: the host must show this volume's devices.
sudo dmsetup ls | grep -q "^sky-" || fail "no sky-* dm devices while thin volume staged"
echo "-- thin pod IO ok"

# §7.1 read cache (15k): the trio is up, the filesystem's claim lands
# on the CACHE device — not the shim — and Invariant B's guest-facing
# mask holds on the device the guest actually sees: dm-cache advertises
# its own virtual discard limits, so this fstrim probes the rcache's
# queue, the exact surface review finding 2 was about. Refused = the
# mask took; success would mean discards are reaching dm-cache's absorb
# as the only wall.
sudo dmsetup ls | grep -q "rcache" || fail "no sky-*-rcache device while thin volume staged"
sudo findmnt -n | grep -q "sky-.*-rcache" || fail "filesystem does not mount the rcache device"
if kubectl -n "$NS" exec smoke-thin-writer -- fstrim /data 2>/dev/null; then
    fail "guest fstrim succeeded — the mount device's discard mask did not take (Invariant B)"
fi
echo "-- read cache staged: fs mounts the rcache, guest fstrim refused"

echo "-- thin fs-consistent snapshot (6h-2: agent cycle + lineage)"
# Taken while the volume is staged and its embedded agent cycle runs:
# the controller files a snapshot request; the agent freezes, captures,
# objectizes into the lineage volume, and aliases the requested name.
kubectl -n "$NS" apply -f tests/k3s/thin-snapshot.yaml
for _ in $(seq 1 90); do
    thin_ready=$(kubectl -n "$NS" get volumesnapshot smoke-thin-snap         -o jsonpath='{.status.readyToUse}' 2>/dev/null || true)
    [ "$thin_ready" = "true" ] && break
    sleep 2
done
[ "$thin_ready" = "true" ] || {
    kubectl -n "$NS" describe volumesnapshot smoke-thin-snap >&2 || true
    fail "thin snapshot never became ready"
}
# Clone it into a raw-mode volume: the snapshot lives in the lineage
# volume, so the existing fast-clone path serves it; content must match
# what the still-running writer put on the thin filesystem.
kubectl -n "$NS" apply -f tests/k3s/thin-clone-pod.yaml
kubectl -n "$NS" wait --for=condition=Ready pod/smoke-thin-cloner --timeout=300s
kubectl -n "$NS" exec smoke-thin-cloner -- cat /data/probe | grep -q skynbd-thin-ok
echo "-- thin snapshot round-trip ok"
kubectl -n "$NS" delete pod smoke-thin-cloner --timeout=180s
kubectl -n "$NS" delete pvc smoke-thin-clone-pvc --timeout=180s
kubectl -n "$NS" delete volumesnapshot smoke-thin-snap --timeout=180s

# Unstage (final publish + teardown) and restage: content must come back
# through the surviving pool file — its tmeta region is the volume's structure.
kubectl -n "$NS" delete pod smoke-thin-writer --timeout=180s
for _ in $(seq 1 60); do
    sudo dmsetup ls 2>/dev/null | grep -q "^sky-" || break
    sleep 2
done
if sudo dmsetup ls 2>/dev/null | grep -q "^sky-"; then
    fail "thin stack not torn down after unstage"
fi
kubectl -n "$NS" apply -f tests/k3s/thin-pod2.yaml
kubectl -n "$NS" wait --for=condition=Ready pod/smoke-thin-reader --timeout=300s
kubectl -n "$NS" exec smoke-thin-reader -- cat /data/probe | grep -q skynbd-thin-ok
echo "-- thin restage ok"

# Fresh-pool restage over a SURVIVING read-cache file — §7.1's one
# invalidation event (fresh pool => cold cache), on a real kernel: the
# pool file is deleted while the rcache file survives, so the next
# stage must take the pool-lost path (fresh pool reading through the
# lineage origin), build the cache COLD (superblock zeroed and synced
# before the ctr — the write_dev ordering dm-bufio makes load-bearing),
# and serve the published content. A warm adoption here is the
# dormant-entry resurrection class; dm-cache mis-reading the zeroed
# carve would fail the stage outright.
kubectl -n "$NS" delete pod smoke-thin-reader --timeout=180s
for _ in $(seq 1 60); do
    sudo dmsetup ls 2>/dev/null | grep -q "^sky-" || break
    sleep 2
done
sudo dmsetup ls 2>/dev/null | grep -q "^sky-" && fail "thin stack not torn down before pool-loss restage"
pool_file=$(sudo sh -c 'ls /var/lib/skynbd-csi/cache/*.cache 2>/dev/null' | head -1)
rcache_file=$(sudo sh -c 'ls /var/lib/skynbd-csi/rcache/*.rcache 2>/dev/null' | head -1)
[ -n "$pool_file" ] || fail "no pool file under /var/lib/skynbd-csi/cache"
[ -n "$rcache_file" ] || fail "no read-cache file under /var/lib/skynbd-csi/rcache"
sudo rm "$pool_file"
kubectl -n "$NS" apply -f tests/k3s/thin-pod2.yaml
kubectl -n "$NS" wait --for=condition=Ready pod/smoke-thin-reader --timeout=300s
kubectl -n "$NS" exec smoke-thin-reader -- cat /data/probe | grep -q skynbd-thin-ok
# Two log shapes prove the fresh path, and WHICH fires depends on the
# state file: "local pool file lost" is the prior-state replay branch
# (driver restart, state file intact over a missing pool file), while
# a kubelet re-stage — unstage deleted the state file — goes straight
# to the fresh plan and logs the origin-attach line instead. Either
# is the pool-lost fresh path.
#
# Label-selected logs, not `logs ds/...`: the workload-ref form missed
# a line demonstrably present in the pod's log (the evidence dump's
# selector form showed it in the same run), so this uses the exact
# invocation the evidence dump has always used — with an explicit
# tail (selector logs default to 10 lines) and a short retry for the
# kubelet log pipeline.
fresh_path=0
for _ in $(seq 1 30); do
    if kubectl -n "$NS" logs -l app.kubernetes.io/component=node \
        --all-containers --tail=500 2>/dev/null \
        | grep -qE "pool file lost|fresh pool reads through the lineage origin export"; then
        fresh_path=1
        break
    fi
    sleep 1
done
[ "$fresh_path" = 1 ] || fail "restage did not take the pool-lost fresh path"
sudo findmnt -n | grep -q "sky-.*-rcache" || fail "cold restage lost the read cache"
if kubectl -n "$NS" exec smoke-thin-reader -- fstrim /data 2>/dev/null; then
    fail "guest fstrim succeeded after cold restage (Invariant B)"
fi
echo "-- pool-loss restage: fresh pool, cold cache, published content served"

echo "-- teardown + reclaim"
# Clone first: while it lives it pins the source snapshot, so deleting
# the VolumeSnapshot before the clone PVC would be refused server-side.
kubectl -n "$NS" delete pod smoke-thin-reader --timeout=180s
kubectl -n "$NS" delete pvc smoke-thin-pvc --timeout=180s
kubectl -n "$NS" delete pod smoke-cloner --timeout=180s
kubectl -n "$NS" delete pvc smoke-clone-pvc --timeout=180s
kubectl -n "$NS" delete pod smoke-reader --timeout=180s
kubectl -n "$NS" delete pvc smoke-ro-pvc --timeout=180s
kubectl -n "$NS" delete volumesnapshot smoke-snap --timeout=180s
kubectl -n "$NS" delete pvc smoke-pvc --timeout=180s
# reclaimPolicy Delete: the PV (and the server-side volume) must go away.
for _ in $(seq 1 45); do
    [ -z "$(kubectl get pv -o name 2>/dev/null)" ] && break
    sleep 2
done
if [ -n "$(kubectl get pv -o name 2>/dev/null)" ]; then
    echo "PV not reclaimed after delete" >&2
    exit 1
fi

echo "k3s smoke OK"

#!/usr/bin/env bash
# The fill step of replay-rig.sh (the daemon mode), run by itself with the
# pool's mount point replaced by a directory whose `fill` is a full device.
#
#   bash review/m1.t14/fill-try.sh <checkout>/tests/kernel/replay-rig.sh
#
# Shows what the rig says when the fill file does not fit: dd fails with
# "No space left on device". Uses /dev/full; nothing else is touched.
set -u
rig=${1:?replay-rig.sh}
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
{
    echo 'set -euo pipefail'
    echo "OUT=$t/out; mkdir -p \$OUT; PHASE=record; MOUNTPOINT=$t/mnt; mkdir -p \$MOUNTPOINT"
    echo 'ln -s /dev/full "$MOUNTPOINT/fill"; DD=dd; SKYZFSD=skyzfsd; FILL_MB=1'
    sed -n '/^daemon_verdict() {/,/^}/p' "$rig"
    sed -n '/^if \[ -n "\$SKYZFSD" \] && \[ "\$FILL_MB" -gt 0 \]; then/,/^fi/p' "$rig"
    echo 'echo "the fill was written"'
} > "$t/snippet.sh"
rc=0; bash "$t/snippet.sh" || rc=$?
echo "exit $rc"
echo "summary: $(head -n 3 "$t/out/replay-summary.md" 2>/dev/null | tr '\n' ' ')"

#!/bin/bash
# Extract published() from agent-record.sh and run it for a snapshot that
# node A no longer has (destroyed), as asettle does when node B has snap/T.
SCRIPT=$1; W=$2
fn() { awk -v h="$1() {" '!on && index($0, h) == 1 { on = 1; print; next } on { print; if ($0 == "}") exit }' "$SCRIPT"; }
RDIR=$W/r; CDIR=$W/r/cases/gone; MNT=$W/mnt; POOL=skzcut
mkdir -p "$CDIR" "$MNT/.zfs/snapshot"
echo 'snap/g1 abc' > "$CDIR/settle.out"
rig_error() { echo "RIG_ERROR(exit 2): $*"; exit 2; }
zfs() { echo "cannot open '$POOL@g1': dataset does not exist" >&2; return 1; }
eval "$(fn published)"
published g1
echo "published returned $?; published.tsv: $(cat "$RDIR/published.tsv" 2>/dev/null)"

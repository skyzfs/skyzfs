#!/usr/bin/env bash
# Review probe (Q10, third look): what txg_probe.py's line says when the
# probe could not read the kstat, or `zpool sync` failed. P = the head's
# txg_probe.py, D = an empty directory.
P=$1 D=$2
mkdir -p "$D/bin"
printf '#!/bin/sh\necho "cannot open $2: no such pool" >&2\nexit 1\n' > "$D/bin/zpool"
chmod +x "$D/bin/zpool"
run() { # KSTAT LOG
    PATH=$D/bin:$PATH timeout 20 python3 -I "$P" probe "$1" nopool "$2" & local p=$!
    sleep 1; t0=$(date +%s.%N); sleep 4; t1=$(date +%s.%N); sleep 1
    kill -TERM "$p"; wait "$p"
    python3 -I "$P" report "$2" "$t0" "$t1"; echo "   report exit $?"
    echo "   log kinds: $(cut -f1 "$2" | sort | uniq -c | tr -s ' ' | tr '\n' ' ')"
}
echo "== A: no kstat; every zpool sync exits 1"
run "$D/nokstat" "$D/a.tsv"
echo "== B: a kstat with its header alone (zfs_txg_history 0); every zpool sync exits 1"
printf '%s\n' '18 0 0x01 0 0 0 0' 'txg      birth            state ndirty       nread        nwritten     reads    writes   otime        qtime        wtime        stime' > "$D/hdr"
run "$D/hdr" "$D/b.tsv"

#!/usr/bin/env bash
# Review probe (Q10, third look): which stdout the writer beside a take
# holds. HEADDIR = a copy of the head's tests/kernel. Run with stdout a pipe.
HEADDIR=$1 D=$2
ZCR_LIB=1
# shellcheck source=/dev/null
. "$HEADDIR/zfs-cli-record.sh"
OUT=$D/out
mkdir -p "$OUT" "$D/dir"
: > "$OUT/index.tsv"
ZCR_WRITER_MARGIN=0.5
look() {
    local pid
    pid=$(pgrep -f "dsync_writer.py write $D/dir/writer.dat" | head -n 1)
    echo "script's stdout:     $(readlink /proc/$$/fd/1)" >&2
    echo "writer's stdout:     $(readlink "/proc/$pid/fd/1")" >&2
    echo "writer's stderr:     $(readlink "/proc/$pid/fd/2" | sed "s|$D|D|")" >&2
}
writing probe "$D/dir" look
echo "stall: $(cat "$OUT/probe/stall")" >&2

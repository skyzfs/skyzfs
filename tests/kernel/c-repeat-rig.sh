#!/usr/bin/env bash
# Check c's first pass, many times, for one or more daemons in turn on
# one machine, with what the daemon rig cannot do in one job: several
# daemons on one runner, many passes, and udev's watch left as the image
# ships it. It is how the cause of #58's check c failure was found (the
# account: skyzfs/skyzfs#58, comment 6858276171): with the watch on, a
# udev read of the device, requeued at a server's death, held back the
# return of a write the old server had completed, and the model took the
# write as issued again. Until the model's half of M1.T28 has been shown
# through a kernel, this is the rig that can show it.
#
#   c-repeat-rig.sh OUT
#
# Each pass is the suite's first pass of check c (skyzfs-blkcheck with
# tests/kernel/blkcheck-helper.sh and the arguments of check_c in
# daemon-rig.sh), with the kernel's block trace (tests/kernel/blktrace.sh)
# and, from a daemon whose build has it, the request log of every volume
# with the extents of its files. Kept: the first pass of each daemon and
# every pass that failed, with the model's replay of each volume.
#
# Environment: RIG_BLKCHECK (the tool), RIG_DAEMONS ("LABEL=PATH ..."),
# RIG_ROUNDS (passes for each daemon, default 10), RIG_WATCH (on: udev's
# rules as the image ships them, which watch ublk devices on Ubuntu 26.04;
# off: the daemon rig's rule that turns the watch off), RIG_SERVE_ARGS
# (default --store in-place).
#
# It judges nothing: exit 0 when it ran, 2 for an error of its own. The
# summary is OUT/summary.md.
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
# shellcheck source=tests/kernel/blktrace.sh
. "$HERE/blktrace.sh"
# shellcheck source=tests/kernel/request-log.sh
. "$HERE/request-log.sh"
OUT=${1:?usage: c-repeat-rig.sh OUT}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
WORK=/mnt/skz-c-repeat
TOOL=${RIG_BLKCHECK:?}
read -r -a DAEMONS <<< "${RIG_DAEMONS:?}"
ROUNDS=${RIG_ROUNDS:-10}
WATCH=${RIG_WATCH:-on}
SERVE_ARGS=${RIG_SERVE_ARGS:---store in-place}
NOWATCH=/etc/udev/rules.d/61-skyzfs-rig-ublkb-nowatch.rules
SUM=$OUT/summary.md
die() { echo "c-repeat-rig: $*" | tee -a "$SUM" >&2; exit 2; }
note() { echo "$*" | tee -a "$SUM"; }

[ "$(id -u)" = 0 ] || die "needs root"
case $WATCH in on|off) ;; *) die "RIG_WATCH is on or off: '$WATCH'" ;; esac
[[ $ROUNDS =~ ^[0-9]+$ ]] || die "RIG_ROUNDS is a number: '$ROUNDS'"
mkdir -p "$WORK"
[ -e /dev/ublk-control ] || modprobe ublk_drv 2> "$OUT/modprobe.log" || {
    apt-get install -y --no-install-recommends "linux-modules-extra-$(uname -r)" >> "$OUT/modprobe.log" 2>&1
    modprobe ublk_drv 2>> "$OUT/modprobe.log" || die "no ublk driver"
}
why=$(blktrace_ready) || die "$why"
MONITOR=""
# shellcheck disable=SC2317,SC2329  # reached through the trap
cleanup() {
    blktrace_off
    [ -z "$MONITOR" ] || kill "$MONITOR" 2>/dev/null
    if [ "$WATCH" = off ]; then rm -f "$NOWATCH"; lim 30 udevadm control --reload; fi
}
trap cleanup EXIT
if [ "$WATCH" = off ]; then
    printf '%s\n' '# c-repeat-rig.sh: no udev watch on ublk devices (M1.T28).' \
        'ACTION!="remove", SUBSYSTEM=="block", KERNEL=="ublkb*", OPTIONS+="nowatch"' > "$NOWATCH"
    lim 30 udevadm control --reload || die "udevadm control --reload failed"
fi

rl_said=""
for d in "${DAEMONS[@]}"; do
    if has_request_log "${d#*=}"; then rl_said+="${d%%=*} yes; "; else rl_said+="${d%%=*} no; "; fi
done
src=$(findmnt -no SOURCE -T "$WORK" 2>/dev/null)
part=$(basename "$(realpath "$src" 2>/dev/null)" 2>/dev/null)
# shellcheck disable=SC1091
note "## Check c's first pass, ${ROUNDS} times for each daemon, in turn, on this machine ($(. /etc/os-release && echo "$PRETTY_NAME"), kernel $(uname -r))"
note ""
note "- udev $(udevadm --version 2>/dev/null | head -n 1); its watch on ublk devices: $WATCH$([ "$WATCH" = on ] && echo ' (the rules as the image ships them)' || echo " ($NOWATCH)")"
note "- the request log (--request-log): ${rl_said}the block trace is on the clock mono, as the log's mono_ns"
note "- the volumes' file system: $(findmnt -no SOURCE,FSTYPE,OPTIONS -T "$WORK" 2>/dev/null) ($part, starts at sector $(cat "/sys/class/block/$part/start" 2>/dev/null || echo '?') of its disk); a volume's c-cN.extents (filefrag -b512) are in its sectors"
note ""
note "| pass | daemon | exit | cases that FAILED | volumes the model's replay finds wrong | change events of a ublk device | requests issued by udev | seconds |"
note "|---|---|---|---|---|---|---|---|"
declare -A failed_passes=()
for r in $(seq 1 "$ROUNDS"); do
    for d in "${DAEMONS[@]}"; do
        label=${d%%=*}; bin=${d#*=}
        p=$OUT/pass-$r-$label; w=$WORK/p$r-$label
        rm -rf "$w"; mkdir -p "$p" "$w"
        udevadm monitor --udev --subsystem-match=block > "$p/udev.txt" 2>&1 &
        MONITOR=$!
        blktrace_on "$p/ftrace.txt"
        blktrace_mark "pass $r $label start"
        t0=$(date +%s)
        rl=""; has_request_log "$bin" && rl=1
        BLKCHECK_TRACE=$p/c-trace.tsv BLK_SKYZFSD=$bin BLK_WORK=$w BLK_SERVE_ARGS=$SERVE_ARGS BLK_REQUEST_LOG=$rl \
            lim 2400 "$TOOL" --harness "$HERE/blkcheck-helper.sh" --can fail-log \
            --cases 16 --rounds 4 --timeout 60 \
            > "$p/c-cases.tsv" 2> "$p/c-blkcheck.err"; rc=$?
        secs=$(( $(date +%s) - t0 ))
        blktrace_off
        kill "$MONITOR" 2>/dev/null; wait "$MONITOR" 2>/dev/null; MONITOR=""
        cp "$w/helper.log" "$p/c-helper.log" 2>/dev/null
        for f in "$w"/*.serve.log "$w"/*.requests.tsv "$w"/*.extents; do [ -s "$f" ] && cp "$f" "$p/c-$(basename "$f")"; done
        fails=$(awk -F'\t' '$1 == "case" && $3 == "FAIL" { printf "%s: %s; ", $2, substr($4, 1, 160) }' "$p/c-cases.tsv")
        bad=""
        if [ -n "$fails" ] || [ $rc != 0 ]; then
            failed_passes[$label]=$(( ${failed_passes[$label]:-0} + 1 ))
            while read -r v; do
                lim 60 "$TOOL" --replay "$p/c-trace.tsv" "$v" > "$p/replay-$v.txt" 2>&1 < /dev/null || bad+="$v "
            done < <(awk -F'\t' '$3 == "open" { print $2 }' "$p/c-trace.tsv" | sort -un)
        fi
        n_change=$(grep -c 'change .*/ublkb[0-9]* (block)' "$p/udev.txt")
        n_udev=$(grep 'block_rq_issue:' "$p/ftrace.txt" | grep -c 'udev[^]]*\]$')
        note "| $r | $label | $rc | ${fails:-none} | ${bad:-none} | $n_change | $n_udev | $secs |"
        gzip -f "$p/c-trace.tsv" "$p/ftrace.txt" "$p"/c-*.requests.tsv "$p"/c-*.extents 2>/dev/null
        # Keep the first pass of each daemon and every pass that failed.
        if [ "$r" != 1 ] && [ -z "$fails" ] && [ $rc = 0 ]; then rm -rf "$p"; fi
        rm -rf "$w"
    done
done
note ""
for d in "${DAEMONS[@]}"; do
    label=${d%%=*}
    note "- $label: ${failed_passes[$label]:-0} of $ROUNDS passes failed"
done
exit 0

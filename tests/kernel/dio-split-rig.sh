#!/usr/bin/env bash
# The rig for the #58 review's account of a check c failure (run
# 43430825700): a direct write of the suite's tool that reaches the
# server as two requests. It needs root, the ublk driver and a build of
# skyzfsd. Two parts:
#
#  1. The probe (tests/kernel/dio_split_probe.py) on one volume of the
#     first daemon. It runs in the modes plain, discard and reader, under
#     ftrace: block_rq_issue on every device, and a function trace of
#     direct_write_fallback (the kernel's way from a direct write to a
#     buffered one) where the kernel can trace it. udevadm monitor runs
#     beside each mode, so that the summary says whether udev answered
#     the discard.
#  2. The suite's first pass of check c (skyzfs-blkcheck with the
#     helper, the arguments of check_c in daemon-rig.sh), RIG_ROUNDS
#     times for each daemon, the daemons in turn on this one machine.
#     The same ftrace runs during each pass, so a failed pass can be
#     read from both sides: the tool's calls (c-trace.tsv) and the
#     requests the kernel issued to the device (ftrace.txt.gz).
#
#   dio-split-rig.sh OUT
#
# Environment: RIG_BLKCHECK (the tool), RIG_DAEMONS ("LABEL=PATH ...",
# the first is the probe's), RIG_ROUNDS (passes for each daemon, 0 for
# the probe only), RIG_PROBE_ROUNDS (pairs of writes in each mode,
# default 2000), RIG_SERVE_ARGS (default --store in-place).
#
# What it writes: OUT/summary.md, OUT/probe-*.json, and OUT/pass-* for
# the first pass of each daemon and every pass that failed. It judges
# nothing: exit 0 when it ran, 2 for an error of its own.
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
OUT=${1:?usage: dio-split-rig.sh OUT}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
WORK=/mnt/skz-dio-split
TOOL=${RIG_BLKCHECK:?}
read -r -a DAEMONS <<< "${RIG_DAEMONS:?}"
ROUNDS=${RIG_ROUNDS:-0}
PROBE_ROUNDS=${RIG_PROBE_ROUNDS:-2000}
SERVE_ARGS=${RIG_SERVE_ARGS:---store in-place}
T=/sys/kernel/tracing
SUM=$OUT/summary.md
die() { echo "dio-split-rig: $*" | tee -a "$SUM" >&2; exit 2; }
note() { echo "$*" | tee -a "$SUM"; }

[ "$(id -u)" = 0 ] || die "needs root"
mkdir -p "$WORK"
[ -e /dev/ublk-control ] || modprobe ublk_drv 2> "$OUT/modprobe.log" || {
    apt-get install -y --no-install-recommends "linux-modules-extra-$(uname -r)" >> "$OUT/modprobe.log" 2>&1
    modprobe ublk_drv 2>> "$OUT/modprobe.log" || die "no ublk driver"
}
[ -d "$T/events/block/block_rq_issue" ] || die "no ftrace event block_rq_issue under $T"
FALLBACK=""
grep -qx 'direct_write_fallback' "$T/available_filter_functions" 2>/dev/null && FALLBACK=direct_write_fallback

# shellcheck disable=SC1091
note "## The direct-write split probe ($(. /etc/os-release && echo "$PRETTY_NAME"), kernel $(uname -r))"
note ""
note "- udev: $(udevadm --version 2>/dev/null | head -n 1); rules that name ublkb: $(grep -l 'ublkb' /usr/lib/udev/rules.d/*.rules /etc/udev/rules.d/*.rules 2>/dev/null | tr '\n' ' ')"
grep -h 'ublkb' /usr/lib/udev/rules.d/*.rules /etc/udev/rules.d/*.rules 2>/dev/null | cut -c1-400 > "$OUT/udev-ublkb-rules.txt"
note "- the rule lines that name ublkb are in udev-ublkb-rules.txt ($(wc -l < "$OUT/udev-ublkb-rules.txt") lines); with OPTIONS watch: $(grep -c 'watch' "$OUT/udev-ublkb-rules.txt")"
note "- function trace of the fallback: ${FALLBACK:-not traceable on this kernel (direct_write_fallback is not in available_filter_functions)}"

TRACE_PID=""
trace_on() { # FILE
    echo 0 > "$T/tracing_on"
    echo nop > "$T/current_tracer"
    echo > "$T/trace"
    echo 65536 > "$T/buffer_size_kb"
    if [ -n "$FALLBACK" ]; then
        echo "$FALLBACK" > "$T/set_ftrace_filter"
        echo function > "$T/current_tracer"
    fi
    echo 1 > "$T/events/block/block_rq_issue/enable"
    cat "$T/trace_pipe" > "$1" &
    TRACE_PID=$!
    echo 1 > "$T/tracing_on"
}
trace_off() {
    echo 0 > "$T/tracing_on"
    sleep 1
    kill "$TRACE_PID" 2>/dev/null; wait "$TRACE_PID" 2>/dev/null
    echo 0 > "$T/events/block/block_rq_issue/enable"
    echo nop > "$T/current_tracer"
    echo > "$T/set_ftrace_filter" 2>/dev/null
    TRACE_PID=""
}
trap '[ -z "$TRACE_PID" ] || trace_off' EXIT

helper() { # WORK VERB N [DAEMON]
    BLK_WORK=$1 BLK_SKYZFSD=$4 BLK_SERVE_ARGS=$SERVE_ARGS lim 120 "$HERE/blkcheck-helper.sh" "$2" "$3"
}

# --- 1. the probe ---------------------------------------------------
probe_daemon=${DAEMONS[0]#*=}
pw=$WORK/probe; rm -rf "$pw"; mkdir -p "$pw"
dev=$(helper "$pw" create 1 "$probe_daemon" 2> "$OUT/probe-helper.err" | tail -n 1)
[ -b "$dev" ] || die "the probe's volume was not served: $(tail -n 3 "$OUT/probe-helper.err" | tr '\n' ' ')"
read -r maj min <<< "$(stat -c '%t %T' "$dev")"
majmin="$((16#$maj)),$((16#$min))"
note "- the probe's device: $dev ($majmin), served by ${DAEMONS[0]%%=*}"
note ""
note "| mode | pairs | write calls | write requests | merges | pairs with more than 2 | pairs with other sectors | shared blocks 16377-16379 (A wins / B wins / mixed) | wrong blocks | fallback calls (all / python3) | udev change events of the device | requests seen in the tail by shape |"
note "|---|---|---|---|---|---|---|---|---|---|---|---|"
for mode in plain discard reader; do
    udevadm monitor --udev --subsystem-match=block > "$OUT/probe-$mode-udev.txt" 2>&1 &
    mon=$!
    sleep 0.5
    trace_on "$OUT/probe-$mode-ftrace.txt"
    lim 900 python3 "$HERE/dio_split_probe.py" "$dev" "$mode" "$PROBE_ROUNDS" "$OUT/probe-$mode.json" \
        > "$OUT/probe-$mode.out" 2> "$OUT/probe-$mode.err"; rc=$?
    trace_off
    kill "$mon" 2>/dev/null; wait "$mon" 2>/dev/null
    if [ $rc != 0 ]; then
        note "| $mode | probe exit $rc: $(tail -n 2 "$OUT/probe-$mode.err" | tr '\n' ' ') |"
        continue
    fi
    shapes=$(awk -v d="$majmin" '$0 ~ /block_rq_issue:/ { for (i = 1; i <= NF; i++) if ($i == "block_rq_issue:") break;
            if ($(i+1) != d || $(i+2) !~ /W/) next; s = $(i+5); n = $(i+7); if (s >= 130944) c[(s / 8) "+" (n / 8)]++ }
            END { for (k in c) printf "%s x%d; ", k, c[k] }' "$OUT/probe-$mode-ftrace.txt" | tr ';' '\n' | sort -t x -k2 -rn | head -n 8 | tr '\n' ';')
    fb_all=$(grep -c "$FALLBACK" "$OUT/probe-$mode-ftrace.txt" 2>/dev/null)
    fb_py=$(grep "$FALLBACK" "$OUT/probe-$mode-ftrace.txt" 2>/dev/null | grep -c 'python3')
    [ -n "$FALLBACK" ] || { fb_all=-; fb_py=-; }
    udev_n=$(grep -c "change.*/${dev#/dev/} (block)" "$OUT/probe-$mode-udev.txt")
    python3 -I - "$OUT/probe-$mode.json" "$mode" "$fb_all / $fb_py" "$udev_n" "$shapes" >> "$SUM" <<'EOF'
import json, sys
r = json.load(open(sys.argv[1]))
o = r['overlap']
a, b = o.get('AAA', 0), o.get('BBB', 0)
mixed = sum(v for k, v in o.items() if k not in ('AAA', 'BBB'))
mixed_kinds = ', '.join(f'{k} x{v}' for k, v in o.items() if k not in ('AAA', 'BBB'))
err = ('; ERRORS: ' + ' | '.join(r['errors'])) if r['errors'] else ''
print(f"| {sys.argv[2]} | {r['calls'] // 2} | {r['calls']} | {r['requests']} | {r['merges']} | {r['split_rounds']} | "
      f"{r['extra_sectors_rounds']} | {a} / {b} / {mixed}{' (' + mixed_kinds + ')' if mixed_kinds else ''} | "
      f"{r['torn_rounds']} | {sys.argv[3]} | {sys.argv[4]} | {sys.argv[5]}{err} |")
EOF
    gzip -f "$OUT/probe-$mode-ftrace.txt"
done
helper "$pw" destroy 1 "$probe_daemon" > /dev/null 2>> "$OUT/probe-helper.err"
note ""
note "A pair is two calls. More than two write requests, or merges, in a pair means a call reached the device as more than one request. \"Mixed\" is a shared block range that neither call won whole, which two whole requests cannot give."

[ "$ROUNDS" -gt 0 ] || exit 0

# --- 2. the suite's first pass, RIG_ROUNDS times for each daemon -------
note ""
note "## Check c's first pass, ${ROUNDS} times for each daemon, in turn, on this machine"
note ""
note "| pass | daemon | exit | cases that FAILED | volumes the model replay finds wrong | fallback calls during the pass (all / the tool's) | seconds |"
note "|---|---|---|---|---|---|---|"
declare -A failed_passes=() fb_total=()
for r in $(seq 1 "$ROUNDS"); do
    for d in "${DAEMONS[@]}"; do
        label=${d%%=*}; bin=${d#*=}
        p=$OUT/pass-$r-$label; w=$WORK/p$r-$label
        rm -rf "$w"; mkdir -p "$p" "$w"
        trace_on "$p/ftrace.txt"
        echo "pass $r $label start" > "$T/trace_marker"
        t0=$(date +%s)
        BLKCHECK_TRACE=$p/c-trace.tsv BLK_SKYZFSD=$bin BLK_WORK=$w BLK_SERVE_ARGS=$SERVE_ARGS \
            lim 2400 "$TOOL" --harness "$HERE/blkcheck-helper.sh" --can fail-log \
            --cases 16 --rounds 4 --timeout 60 \
            > "$p/c-cases.tsv" 2> "$p/c-blkcheck.err"; rc=$?
        secs=$(( $(date +%s) - t0 ))
        trace_off
        cp "$w/helper.log" "$p/c-helper.log" 2>/dev/null
        for f in "$w"/*.serve.log; do [ -s "$f" ] && cp "$f" "$p/c-$(basename "$f")"; done
        fails=$(awk -F'\t' '$1 == "case" && $3 == "FAIL" { printf "%s: %s; ", $2, substr($4, 1, 160) }' "$p/c-cases.tsv")
        bad=""
        if [ -n "$fails" ] || [ $rc != 0 ]; then
            while read -r v; do
                lim 60 "$TOOL" --replay "$p/c-trace.tsv" "$v" > "$p/replay-$v.txt" 2>&1 < /dev/null || bad+="$v "
            done < <(awk -F'\t' '$3 == "open" { print $2 }' "$p/c-trace.tsv" | sort -un)
        fi
        if [ -n "$FALLBACK" ]; then
            fb="$(grep -c "$FALLBACK" "$p/ftrace.txt") / $(grep "$FALLBACK" "$p/ftrace.txt" | grep -c 'skyzfs-blkchec')"
        else
            fb="-"
        fi
        fb_total[$label]="${fb_total[$label]:-} $fb;"
        note "| $r | $label | $rc | ${fails:-none} | ${bad:-none} | $fb | $secs |"
        if [ -n "$fails" ] || [ $rc != 0 ]; then
            failed_passes[$label]=$(( ${failed_passes[$label]:-0} + 1 ))
        fi
        gzip -f "$p/c-trace.tsv" "$p/ftrace.txt"
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

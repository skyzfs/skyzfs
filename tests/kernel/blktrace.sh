# shellcheck shell=bash
# The kernel's block layer, traced with ftrace, for a rig that must be
# able to say what each device was handed (M1.T28): every request issued
# to a driver, a bio split, a request put back in the queue (the ublk
# driver requeues what was in flight when its server died) and every
# completion, of every device, on the clock `mono`. The daemon's request
# log (--request-log) stamps CLOCK_MONOTONIC, so the two line up.
#
#   blktrace_ready          0 when the events and the clock are there; else
#                           prints why and returns 1
#   blktrace_on FILE        start; the trace streams to FILE
#   blktrace_off            stop, and put back what blktrace_on found:
#                           tracing_on, the buffer's size, the clock, the
#                           tracer and each event's switch
#   blktrace_mark TEXT      a line in the trace, on its clock
#
# One trace at a time per machine: ftrace is the kernel's, not a
# process's. Needs root. BLKTRACE_T names the tracefs directory; a
# directory of plain files stands in for it in a test with no kernel.
BLKTRACE_T=${BLKTRACE_T:-/sys/kernel/tracing}
BLKTRACE_EVENTS="block_rq_issue block_split block_rq_requeue block_rq_complete"
# KiB per CPU. A pass of check c traces about 15 MB of text in two to
# three minutes, and the reader drains the buffer as it goes.
BLKTRACE_BUFFER_KB=${BLKTRACE_BUFFER_KB:-8192}
BLKTRACE_PID=""
BLKTRACE_WAS=()

blktrace_ready() {
    local e
    for e in $BLKTRACE_EVENTS; do
        [ -d "$BLKTRACE_T/events/block/$e" ] || { echo "no ftrace event block/$e under $BLKTRACE_T"; return 1; }
    done
    grep -qw mono "$BLKTRACE_T/trace_clock" 2>/dev/null || { echo "no trace clock mono"; return 1; }
    return 0
}

# The word in brackets of a file such as trace_clock ("[local] global
# mono"), or its first word (current_tracer, buffer_size_kb's
# "7 (expanded: 1408)" gives 7).
blktrace_word() {
    local v
    v=$(cat "$1" 2>/dev/null) || return 0
    if [[ $v =~ \[([^]]+)\] ]]; then echo "${BASH_REMATCH[1]}"; else echo "${v%% *}"; fi
}

blktrace_on() {
    local e t=$BLKTRACE_T
    BLKTRACE_WAS=("tracing_on=$(blktrace_word "$t/tracing_on")"
        "buffer_size_kb=$(blktrace_word "$t/buffer_size_kb")"
        "trace_clock=$(blktrace_word "$t/trace_clock")"
        "current_tracer=$(blktrace_word "$t/current_tracer")")
    for e in $BLKTRACE_EVENTS; do
        BLKTRACE_WAS+=("events/block/$e/enable=$(blktrace_word "$t/events/block/$e/enable")")
    done
    echo 0 > "$t/tracing_on"
    echo nop > "$t/current_tracer"
    echo > "$t/trace"
    echo "$BLKTRACE_BUFFER_KB" > "$t/buffer_size_kb"
    echo mono > "$t/trace_clock"
    for e in $BLKTRACE_EVENTS; do echo 1 > "$t/events/block/$e/enable"; done
    cat "$t/trace_pipe" > "$1" &
    BLKTRACE_PID=$!
    echo 1 > "$t/tracing_on"
}

blktrace_off() {
    local kv t=$BLKTRACE_T
    [ -n "$BLKTRACE_PID" ] || return 0
    echo 0 2>/dev/null > "$t/tracing_on"
    # What is still in the buffer reaches the file before the reader goes.
    sleep 1
    kill "$BLKTRACE_PID" 2>/dev/null
    wait "$BLKTRACE_PID" 2>/dev/null
    BLKTRACE_PID=""
    echo 2>/dev/null > "$t/trace"
    # The events first and tracing_on last, so that nothing is traced
    # between the two.
    for kv in "${BLKTRACE_WAS[@]:4}" "${BLKTRACE_WAS[@]:1:3}" "${BLKTRACE_WAS[@]:0:1}"; do
        [ -n "${kv#*=}" ] || continue
        echo "${kv#*=}" 2>/dev/null > "$t/${kv%%=*}"
    done
    return 0
}

blktrace_mark() { echo "$*" 2>/dev/null > "$BLKTRACE_T/trace_marker"; }

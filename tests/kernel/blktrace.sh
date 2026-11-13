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
#   blktrace_off            stop, and leave ftrace as it was found
#   blktrace_mark TEXT      a line in the trace, on its clock
#
# One trace at a time per machine: ftrace is the kernel's, not a
# process's. Needs root. With BLKTRACE_FALLBACK=1 it also traces calls of
# direct_write_fallback (the way from a direct write to a buffered one),
# where the kernel can trace that function.
BLKTRACE_T=${BLKTRACE_T:-/sys/kernel/tracing}
BLKTRACE_EVENTS="block_rq_issue block_split block_rq_requeue block_rq_complete"
BLKTRACE_PID=""

blktrace_ready() {
    local e
    for e in $BLKTRACE_EVENTS; do
        [ -d "$BLKTRACE_T/events/block/$e" ] || { echo "no ftrace event block/$e under $BLKTRACE_T"; return 1; }
    done
    grep -qw mono "$BLKTRACE_T/trace_clock" 2>/dev/null || { echo "no trace clock mono"; return 1; }
    return 0
}

blktrace_on() {
    local e t=$BLKTRACE_T
    echo 0 > "$t/tracing_on"
    echo nop > "$t/current_tracer"
    echo > "$t/trace"
    echo 65536 > "$t/buffer_size_kb"
    echo mono > "$t/trace_clock"
    if [ -n "${BLKTRACE_FALLBACK:-}" ] && grep -qx direct_write_fallback "$t/available_filter_functions" 2>/dev/null; then
        echo direct_write_fallback > "$t/set_ftrace_filter"
        echo function > "$t/current_tracer"
    fi
    for e in $BLKTRACE_EVENTS; do echo 1 > "$t/events/block/$e/enable"; done
    cat "$t/trace_pipe" > "$1" &
    BLKTRACE_PID=$!
    echo 1 > "$t/tracing_on"
}

blktrace_off() {
    local e t=$BLKTRACE_T
    [ -n "$BLKTRACE_PID" ] || return 0
    echo 0 > "$t/tracing_on"
    # What is still in the buffer reaches the file before the reader goes.
    sleep 1
    kill "$BLKTRACE_PID" 2>/dev/null
    wait "$BLKTRACE_PID" 2>/dev/null
    for e in $BLKTRACE_EVENTS; do echo 0 > "$t/events/block/$e/enable"; done
    echo nop > "$t/current_tracer"
    echo > "$t/set_ftrace_filter" 2>/dev/null
    echo local > "$t/trace_clock"
    BLKTRACE_PID=""
    return 0
}

blktrace_mark() { echo "$*" > "$BLKTRACE_T/trace_marker" 2>/dev/null; }

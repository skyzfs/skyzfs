#!/usr/bin/env bash
# What tests/kernel/blktrace.sh leaves in ftrace: the state before
# blktrace_on, and after blktrace_off. Needs root and tracefs.
#   ftrace-state.sh WORKTREE
set -uo pipefail
T=/sys/kernel/tracing
state() {
    local f
    for f in tracing_on current_tracer trace_clock buffer_size_kb buffer_total_size_kb; do
        printf '  %-22s %s\n' "$f" "$(tr '\n' ' ' < "$T/$f")"
    done
    printf '  %-22s %s\n' set_ftrace_filter "$(head -c 60 "$T/set_ftrace_filter" | tr '\n' ' ')"
    for f in block_rq_issue block_split block_rq_requeue block_rq_complete; do
        printf '  %-22s %s\n' "events/block/$f" "$(cat "$T/events/block/$f/enable")"
    done
    printf '  %-22s %s\n' "MemFree kB" "$(awk '$1 == "MemFree:" { print $2 }' /proc/meminfo)"
    printf '  %-22s %s\n' "online CPUs" "$(nproc)"
}
# shellcheck disable=SC1091
. "$1/tests/kernel/blktrace.sh"
echo "kernel $(uname -r); blktrace.sh of $(git -C "$1" rev-parse --short HEAD)"
echo "ready: $(blktrace_ready && echo yes)"
echo "before blktrace_on:"; state
out=$(mktemp)
blktrace_on "$out"
blktrace_mark "ftrace-state start"
dd if=/dev/zero of="$(dirname "$out")/ftrace-state.io" bs=1M count=8 oflag=direct 2>/dev/null
sync
echo "while tracing:"; state
blktrace_off
unlink "$(dirname "$out")/ftrace-state.io"
echo "after blktrace_off:"; state
echo "the trace: $(wc -l < "$out") lines, $(grep -c block_rq_issue "$out") block_rq_issue, $(grep -c 'LOST' "$out") lines that say events were lost"
unlink "$out"

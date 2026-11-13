#!/usr/bin/env bash
# The same question as ftrace-state.sh with no kernel: BLKTRACE_T at a
# directory of plain files that hold what a booted kernel shows, then
# blktrace_on and blktrace_off, and a diff of the directory.
#   as-found-nokernel.sh WORKTREE
set -uo pipefail
T=$(mktemp -d) || exit 2
trap 'rm -rf "${T:?}"' EXIT
mkdir -p "$T/t/events/block"
for e in block_rq_issue block_split block_rq_requeue block_rq_complete; do
    mkdir "$T/t/events/block/$e"; echo 0 > "$T/t/events/block/$e/enable"
done
echo 1 > "$T/t/tracing_on"; echo nop > "$T/t/current_tracer"; echo 1408 > "$T/t/buffer_size_kb"
echo global > "$T/t/trace_clock.set"          # what the machine had chosen
echo 'local global counter uptime perf mono mono_raw boot tai' > "$T/t/trace_clock"
: > "$T/t/trace"; : > "$T/t/trace_pipe"; : > "$T/t/trace_marker"; : > "$T/t/set_ftrace_filter"
cp -r "$T/t" "$T/before"
export BLKTRACE_T=$T/t
# shellcheck disable=SC1091
. "$1/tests/kernel/blktrace.sh"
blktrace_ready || exit 2
blktrace_on "$T/out.txt"
blktrace_off
diff -r "$T/before" "$T/t" && echo "as found"

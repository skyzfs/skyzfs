#!/usr/bin/env bash
# Tests of tests/kernel/blktrace.sh with no kernel: BLKTRACE_T points at
# a directory of plain files laid out as tracefs. blktrace_on sets what
# the trace needs, and blktrace_off puts back what it found, whatever
# that was (M1.T28 review: tracing_on and a 64 MiB buffer per CPU were
# left behind).
#
#   bash tests/kernel/test_blktrace.sh
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
T=$(mktemp -d) || exit 2
trap 'rm -rf "$T"' EXIT
FAILS=0
check() { # name, then a command that must succeed
    local name=$1; shift
    if "$@"; then echo "ok   $name"; else echo "FAIL $name"; FAILS=$((FAILS + 1)); fi
}
is() { [ "$(cat "$T/$1")" = "$2" ]; }

# A machine that is not at its defaults: tracing on, one event already
# enabled, another clock and tracer.
echo 1 > "$T/tracing_on"
echo '7 (expanded: 1408)' > "$T/buffer_size_kb"
echo 'local [global] mono' > "$T/trace_clock"
echo function > "$T/current_tracer"
printf 'a request\nanother\n' > "$T/trace_pipe"
: > "$T/trace"; : > "$T/trace_marker"
for e in block_rq_issue block_split block_rq_requeue block_rq_complete; do
    mkdir -p "$T/events/block/$e"; echo 0 > "$T/events/block/$e/enable"
done
echo 1 > "$T/events/block/block_split/enable"

BLKTRACE_T=$T
# shellcheck source=tests/kernel/blktrace.sh
. "$HERE/blktrace.sh"
# shellcheck disable=SC2034  # read by blktrace.sh
BLKTRACE_BUFFER_KB=4096

check "ready with the events and the clock mono" blktrace_ready
blktrace_on "$T/out.txt"
check "on: the clock is mono" is trace_clock mono
check "on: the buffer is the trace's" is buffer_size_kb 4096
check "on: no other tracer" is current_tracer nop
check "on: the four events are enabled" is events/block/block_rq_issue/enable 1
check "on: tracing is on" is tracing_on 1
blktrace_mark "a mark"
check "the mark goes to trace_marker" is trace_marker "a mark"
blktrace_off
check "off: the trace reached its file" grep -q 'another' "$T/out.txt"
check "off: tracing_on as found" is tracing_on 1
check "off: the buffer as found" is buffer_size_kb 7
check "off: the clock as found" is trace_clock global
check "off: the tracer as found" is current_tracer function
check "off: an event that was off is off" is events/block/block_rq_issue/enable 0
check "off: an event that was on is on" is events/block/block_split/enable 1
check "off twice does nothing" blktrace_off

rm -rf "$T/events/block/block_split"
check "not ready without an event" eval '! blktrace_ready > /dev/null'

[ "$FAILS" = 0 ] || { echo "$FAILS failed"; exit 1; }
echo "all passed"

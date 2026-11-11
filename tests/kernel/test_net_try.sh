#!/bin/bash
# Tests of tests/lib/net-try.sh. Run: bash tests/kernel/test_net_try.sh
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=tests/lib/net-try.sh
. "$HERE/../lib/net-try.sh"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
is() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi; }
NET_TRY_PAUSE=0
flaky() { echo x >> "$T/calls"; [ "$(wc -l < "$T/calls")" -ge "$1" ]; }       # succeeds at its Nth call

: > "$T/calls"; net_try flaky 1; is "a command that works is run once" "$?/$(wc -l < "$T/calls" | tr -d ' ')/$NET_TRIES" 0/1/1
: > "$T/calls"; net_try flaky 3; is "one that works at the third try" "$?/$(wc -l < "$T/calls" | tr -d ' ')/$NET_TRIES" 0/3/3
: > "$T/calls"; net_try flaky 4; rc=$?
is "one that never works is tried three times and fails" "$([ $rc != 0 ] && echo failed)/$(wc -l < "$T/calls" | tr -d ' ')" failed/3
net_try sh -c 'exit 7'; is "the last status is passed on" "$?" 7

[ "$fails" = 0 ] || { echo "$fails failed"; exit 1; }
echo "all passed"

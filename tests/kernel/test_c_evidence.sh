#!/usr/bin/env bash
# Tests of check c's evidence in daemon-rig.sh (M1.T28) with no kernel:
# c_evidence_end keeps the request logs, the extents and the block trace
# in the artifact for a pass in which a case failed or the tool did not
# end, and removes them for a pass that passed. The function is taken
# from the rig's text, with blktrace_off and the udev monitor stubbed.
#
#   bash tests/kernel/test_c_evidence.sh
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
TMP=$(mktemp -d) || exit 2
trap 'rm -rf "$TMP"' EXIT
FAILS=0
check() { # name, then a command that must succeed
    local name=$1; shift
    if "$@"; then echo "ok   $name"; else echo "FAIL $name"; FAILS=$((FAILS + 1)); fi
}

eval "$(sed -n '/^c_evidence_end() {$/,/^}$/p' "$HERE/daemon-rig.sh")"
declare -F c_evidence_end > /dev/null || { echo "FAIL c_evidence_end not found in daemon-rig.sh"; exit 1; }
blktrace_off() { :; }

# pass TAG RC CASE_RESULT [LOG TRACE EVENT]: a pass with one case whose
# result is CASE_RESULT; with LOG (default 1) one volume's log and
# extents, from a server that keeps a log (C_RL), and with TRACE (default
# 1) a trace of two lines, one of them a request udev's probe issued;
# the udev monitor saw one event of kind EVENT (default add).
pass() {
    OUT=$TMP/out-$1-$2-$3-${4:-1}${5:-1}; local work=$TMP/work-$1-$2-$3-${4:-1}${5:-1}
    mkdir -p "$OUT" "$work"
    if [ "${4:-1}" = 1 ]; then
        printf 'mono\tpid\trecv\n' > "$work/c60.requests.tsv"
        # ENDED: more lines of the log, the commands the driver ended.
        [ -z "${ENDED:-}" ] || printf '%b' "$ENDED" >> "$work/c60.requests.tsv"
        printf 'extents\n' > "$work/c60.extents"
    fi
    if [ "${5:-1}" = 1 ]; then
        printf '%s\n' 'x-1 [001] ..... 1.0: block_rq_issue: 259,0 RA 4096 () 131056 + 8 be,0,4 [(udev-worker)]' \
            'x-2 [001] ..... 1.1: block_rq_issue: 259,0 WS 4096 () 0 + 8 be,0,4 [skyzfs-blkcheck]' > "$OUT/$1-ftrace.txt"
    fi
    printf 'UDEV [1.0] %s   /devices/virtual/block/ublkb0 (block)\n' "${6:-add}" > "$OUT/$1-udev.txt"
    printf 'declared\tx\tWEAKER\ncase\tx\t%s\tdetail\n' "$3" > "$OUT/$1-cases.tsv"
    # No monitor runs here: the function's kill and wait find nothing.
    # shellcheck disable=SC2034  # read by c_evidence_end
    C_MONITOR=""
    C_EVIDENCE=""; C_MISSING=""
    # shellcheck disable=SC2034  # read by c_evidence_end
    C_RL=1
    c_evidence_end "$1" "$work" "$2"
}

pass c 0 WEAKER
check "a pass that passed keeps nothing" \
    test -z "$(find "$OUT" -name '*requests*' -o -name '*extents*' -o -name '*ftrace*')"
check "and says so, with the counts" \
    grep -q 'pass c: change events of a ublk device 0; requests issued by udev 1; request logs 1; block trace 2 lines; not kept: no case failed' <<< "$C_EVIDENCE"
check "a pass with all it must leave misses nothing" test -z "$C_MISSING"

pass cd 0 FAIL
check "a failed case keeps the logs, the extents and the trace, compressed" \
    test -s "$OUT/cd-ftrace.txt.gz" -a -s "$OUT/cd-c60.requests.tsv.gz" -a -s "$OUT/cd-c60.extents.gz"
check "and says where" grep -q 'pass cd: .*kept in the artifact (cd-ftrace.txt.gz cd-cN.requests.tsv.gz cd-cN.extents.gz)' <<< "$C_EVIDENCE"

pass c 124 WEAKER
check "a tool that did not end keeps them too" \
    test -s "$OUT/c-ftrace.txt.gz" -a -s "$OUT/c-c60.requests.tsv.gz"

pass c 0 WEAKER 1 0
check "a pass that left no trace is missing it" grep -q 'pass c left no block trace' <<< "$C_MISSING"

pass c 0 WEAKER 0 1
check "a server that keeps a log and left none is missing it" grep -q 'pass c left no request log' <<< "$C_MISSING"

pass c 0 WEAKER 1 1 change
check "a change event with the watch off is missing evidence" grep -q 'pass c saw 1 change events' <<< "$C_MISSING"

pass cd 0 FAIL 0 1
check "a kept pass names only what it kept" grep -q 'kept in the artifact (cd-ftrace.txt.gz)' <<< "$C_EVIDENCE"

# The commands the driver ended (M1.T30): counted from every request
# log of the pass, the driver's abort (-19) apart from any other result.
# The counts start unset above: the function takes them as 0.
C_CMD_LOGS=0; C_CMD_FAILED=0; C_CMD_ABORTED=0; C_CMD_FIRST=""; C_CMD_PASSES=""
pass c 0 WEAKER
check "a log with no ended line counts none" test "$C_CMD_LOGS $C_CMD_FAILED $C_CMD_ABORTED" = "1 0 0"
ENDED='1\t9\tended\t3\tcommit\t-\t-\t-19\n2\t9\tended\t5\tcommit\t-\t-\t-22\n3\t9\tended\t6\tfetch\t-\t-\t-5\n' pass cd 0 WEAKER
check "an abort apart from two refused commands, summed over the passes" \
    test "$C_CMD_LOGS $C_CMD_FAILED $C_CMD_ABORTED" = "2 2 1"
check "the first refused one is named" test "$C_CMD_FIRST" = "pass cd: tag 5, commit, result -22"
check "and both passes" test "$C_CMD_PASSES" = "c cd "

[ "$FAILS" = 0 ] || { echo "$FAILS failed"; exit 1; }
echo "all passed"

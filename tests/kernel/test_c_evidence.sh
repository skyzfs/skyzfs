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

# pass TAG RC CASE_RESULT: a pass with one volume's log and extents, a
# trace, and one case whose result is CASE_RESULT.
pass() {
    OUT=$TMP/out-$1-$2-$3; local work=$TMP/work-$1-$2-$3
    mkdir -p "$OUT" "$work"
    printf 'mono\tpid\trecv\n' > "$work/c60.requests.tsv"
    printf 'extents\n' > "$work/c60.extents"
    printf 'line\nline\n' > "$OUT/$1-ftrace.txt"
    printf 'UDEV [1.0] change   /devices/virtual/block/ublkb0 (block)\n' > "$OUT/$1-udev.txt"
    printf 'declared\tx\tWEAKER\ncase\tx\t%s\tdetail\n' "$3" > "$OUT/$1-cases.tsv"
    # No monitor runs here: the function's kill and wait find nothing.
    # shellcheck disable=SC2034  # read by c_evidence_end
    C_MONITOR=""
    C_EVIDENCE=""
    c_evidence_end "$1" "$work" "$2"
}

pass c 0 WEAKER
check "a pass that passed keeps nothing" \
    test -z "$(find "$OUT" -name '*requests*' -o -name '*extents*' -o -name '*ftrace*')"
check "and says so, with the counts" \
    grep -q 'pass c: change events of a ublk device 1; request logs 1; block trace 2 lines; not kept: no case failed' <<< "$C_EVIDENCE"

pass cd 0 FAIL
check "a failed case keeps the logs, the extents and the trace, compressed" \
    test -s "$OUT/cd-ftrace.txt.gz" -a -s "$OUT/cd-c60.requests.tsv.gz" -a -s "$OUT/cd-c60.extents.gz"
check "and says where" grep -q 'pass cd: .*kept in the artifact' <<< "$C_EVIDENCE"

pass c 124 WEAKER
check "a tool that did not end keeps them too" \
    test -s "$OUT/c-ftrace.txt.gz" -a -s "$OUT/c-c60.requests.tsv.gz"

[ "$FAILS" = 0 ] || { echo "$FAILS failed"; exit 1; }
echo "all passed"

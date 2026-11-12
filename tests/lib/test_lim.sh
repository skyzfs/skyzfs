#!/usr/bin/env bash
# Tests of tests/lib/lim.sh. No kernel: bash tests/lib/test_lim.sh
#
# lim must behave the same in a script with errexit and pipefail as in
# one without: return the command's status, pass on what it wrote, and
# give up at the limit. Under `set -e` it once ended its caller with
# status 2 and no message when the command had just ended.
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
fails=0
check() { # name expected got
    if [ "$2" = "$3" ]; then echo "ok: $1"; else echo "FAIL: $1: expected '$2', got '$3'"; fails=$((fails + 1)); fi
}
# case NAME OPTIONS BODY: run BODY in a new bash with those options and
# lim sourced; -> its stdout and its exit status.
run() {
    bash -c "$1"'
. "$0/lim.sh"
'"$2" "$HERE" 2>/dev/null
}
for opts in ':' 'set -e' 'set -euo pipefail'; do
    name="[$opts]"
    out=$(run "$opts" 'lim 5 true; echo "after $?"'); rc=$?
    check "$name a command that succeeds, and the caller goes on" "after 0 / 0" "$out / $rc"
    out=$(run "$opts" 'lim 5 sh -c "echo said; exit 3" || echo "status $?"; echo went-on'); rc=$?
    check "$name a failing command's status and output, in an || list" "said status 3 went-on / 0" "${out//$'\n'/ } / $rc"
    out=$(run "$opts" 'if lim 5 false; then echo yes; else echo "no $?"; fi'); rc=$?
    check "$name a failing command in an if" "no 1 / 0" "$out / $rc"
    # shellcheck disable=SC2016  # the inner shell expands it
    out=$(run "$opts" 'x=$(lim 5 sh -c "echo captured") && echo "$x"'); rc=$?
    check "$name output through a command substitution" "captured / 0" "$out / $rc"
    t0=$(date +%s)
    out=$(run "$opts" 'lim 1 sleep 30 || echo "status $?"'); rc=$?
    took=$(( $(date +%s) - t0 ))
    check "$name the limit: 124" "status 124 / 0" "$out / $rc"
    [ "$took" -le 5 ] || { echo "FAIL: $name the limit took $took s"; fails=$((fails + 1)); }
done
# Under errexit a bare failing lim ends the caller with the command's
# status, like any failing command, and what it wrote is passed on first.
out=$(run 'set -e' 'lim 5 sh -c "echo before-the-end; exit 7"; echo not-reached'); rc=$?
check "[set -e] a bare failing lim ends the caller with the command's status, its output passed on" "before-the-end / 7" "$out / $rc"
if [ "$fails" != 0 ]; then echo "$fails failed"; exit 1; fi
echo "all passed"

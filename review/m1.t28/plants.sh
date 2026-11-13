#!/usr/bin/env bash
# Defects planted in a copy of tests/kernel/daemon-rig.sh, one at a
# time, and what the two no-kernel tests of check c's evidence say.
#   plants.sh WORKTREE
set -uo pipefail
SRC=$1/tests
TMP=$(mktemp -d) || exit 2
trap 'rm -rf "${TMP:?}"' EXIT
plant() { # name, python expression on the text `t` giving the new text
    local name=$1 d=$TMP/$1 a b
    mkdir -p "$d"; cp -r "$SRC" "$d/tests"
    if ! python3 -I - "$d/tests/kernel/daemon-rig.sh" "$2" <<'PY'
import sys
p, expr = sys.argv[1], sys.argv[2]
t = open(p, encoding='utf-8').read()
n = eval(expr)
if n == t:
    sys.exit(1)
open(p, 'w', encoding='utf-8').write(n)
PY
    then echo "$name: THE PLANT DID NOT APPLY"; return; fi
    a=$(bash "$d/tests/kernel/test_c_evidence.sh" 2>&1 | grep -c '^FAIL')
    b=$(python3 "$d/tests/kernel/test_daemon_check.py" 2>&1 | grep -E '^(FAIL|ERROR):' | sed 's/ (.*//' | tr '\n' ' ')
    echo "$name: test_c_evidence.sh $a checks fail; test_daemon_check.py: ${b:-nothing fails}"
}
echo "unchanged: $(bash "$SRC/kernel/test_c_evidence.sh" | tail -n 1); $(python3 "$SRC/kernel/test_daemon_check.py" 2>&1 | tail -n 1)"
K='if [ "$rc" != 0 ] || awk'
plant keep-never           "t.replace('\$3 == \"FAIL\" { f = 1 }', '\$3 == \"FAILED\" { f = 1 }').replace('if [ \"\$rc\" != 0 ] || awk', 'if [ \"\$rc\" = x ] || awk')"
plant keep-always          "t.replace('if [ \"\$rc\" != 0 ] || awk', 'if true || awk')"
plant keep-only-on-rc      "t.replace('\$3 == \"FAIL\" { f = 1 }', '\$3 == \"FAILED\" { f = 1 }')"
plant keep-only-on-fail    "t.replace('if [ \"\$rc\" != 0 ] || awk', 'if [ \"\$rc\" = x ] || awk')"
plant extents-not-kept     "t.replace('gzip -f \"\$OUT/\$tag-ftrace.txt\" \"\$OUT/\$tag\"-c*.requests.tsv \"\$OUT/\$tag\"-c*.extents 2>/dev/null', 'gzip -f \"\$OUT/\$tag-ftrace.txt\" \"\$OUT/\$tag\"-c*.requests.tsv 2>/dev/null')"
plant trace-not-kept       "t.replace('gzip -f \"\$OUT/\$tag-ftrace.txt\" \"\$OUT/\$tag\"-c*.requests.tsv', 'rm -f \"\$OUT/\$tag-ftrace.txt\"; gzip -f \"\$OUT/\$tag\"-c*.requests.tsv')"
plant passed-keeps-trace   "t.replace('rm -f \"\$OUT/\$tag-ftrace.txt\" \"\$OUT/\$tag\"-c*.requests.tsv', 'rm -f \"\$OUT/\$tag\"-c*.requests.tsv')"
plant change-count-zero    "t.replace(\"n_change=\$(grep -c 'change .*/ublkb[0-9]* (block)'\", \"n_change=\$(grep -c 'change .*/ublkc[0-9]* (block)'\")"
plant no-watch-back-exit   "t.replace('    blktrace_off\n    c_watch_back\n    local p', '    blktrace_off\n    local p')"
plant no-watch-back-end    "t.replace('    c_watch_back\n    # The trace is asked for on every run', '    # The trace is asked for on every run')"
plant watch-never-off      "t.replace('    if c_watch_off; then', '    if true; then')"
plant no-trace-off-exit    "t.replace('    blktrace_off\n    c_watch_back\n    local p', '    c_watch_back\n    local p')"
plant no-end-of-cd         "t.replace('    c_evidence_end cd \"\$work\" \"\$rc\"\n', '')"
plant start-after-the-tool "t.replace('    c_evidence_start c\n', '').replace('    C_RC=\$rc\n', '    C_RC=\$rc\n    c_evidence_start c\n')"
plant no-request-log       "t.replace('BLK_SERVE_ARGS=\${RIG_SERVE_ARGS:-} BLK_REQUEST_LOG=\$C_RL', 'BLK_SERVE_ARGS=\${RIG_SERVE_ARGS:-}')"
plant missing-trace-is-ok  "t.replace('obs c_evidence \"not run: no block trace: \$C_TRACE_WHY. \${C_EVIDENCE}\"', 'obs c_evidence \"no block trace: \$C_TRACE_WHY. \${C_EVIDENCE}\"')"
plant trace-never-started  "t.replace('        blktrace_on \"\$OUT/\$1-ftrace.txt\"\n', '')"

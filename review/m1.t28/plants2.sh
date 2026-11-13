#!/usr/bin/env bash
# The second look (c812539): the six plants no test caught at 9addf69,
# and five in tests/kernel/blktrace.sh, against the no-kernel tests.
#   plants2.sh WORKTREE
set -uo pipefail
SRC=$1/tests
TMP=$(mktemp -d) || exit 2
trap 'rm -rf "${TMP:?}"' EXIT
plant() { # name file python-expression on `t`
    local name=$1 d=$TMP/$1 a b c
    mkdir -p "$d"; cp -r "$SRC" "$d/tests"
    if ! python3 -I - "$d/tests/kernel/$2" "$3" <<'PY'
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
    c=$(bash "$d/tests/kernel/test_blktrace.sh" 2>&1 | grep -c '^FAIL')
    b=$(python3 "$d/tests/kernel/test_daemon_check.py" 2>&1 | grep -E '^(FAIL|ERROR):' | sed 's/ (.*//' | tr '\n' ' ')
    echo "$name: test_c_evidence.sh $a fail; test_blktrace.sh $c fail; test_daemon_check.py: ${b:-nothing fails}"
}
R=daemon-rig.sh; B=blktrace.sh
plant watch-never-off        $R "t.replace('    if c_watch_off; then', '    if true; then')"
plant no-request-log         $R "t.replace('BLK_SERVE_ARGS=\${RIG_SERVE_ARGS:-} BLK_REQUEST_LOG=\$C_RL', 'BLK_SERVE_ARGS=\${RIG_SERVE_ARGS:-}')"
plant trace-never-started    $R "t.replace('        blktrace_on \"\$OUT/\$1-ftrace.txt\"\n', '')"
plant start-after-the-tool   $R "t.replace('    c_evidence_start c\n', '').replace('    C_RC=\$rc\n', '    C_RC=\$rc\n    c_evidence_start c\n')"
plant no-trace-off-exit      $R "t.replace('    touch \"\$WORK/stop-all\"\n    blktrace_off\n', '    touch \"\$WORK/stop-all\"\n')"
plant missing-is-ok          $R "t.replace('obs c_evidence \"not run: \${C_MISSING}\${C_EVIDENCE}\"', 'obs c_evidence \"\${C_MISSING}\${C_EVIDENCE}\"')"
plant no-trace-is-not-missing $R "t.replace('    [ \"\${n_lines:-0}\" -gt 0 ] || C_MISSING+=', '    true || C_MISSING+=')"
plant change-is-not-missing  $R "t.replace('        *) C_MISSING+=\"pass \$tag saw', '        *) : \"pass \$tag saw')"
plant watch-back-before-low  $R "t.replace('    check_c\n    check_c_low\n    # udev', '    check_c\n    c_watch_back\n    check_c_low\n    # udev')"
plant off-leaves-tracing-off $B "t.replace('\"\${BLKTRACE_WAS[@]:0:1}\"; do', '; do')"
plant off-keeps-the-buffer   $B "t.replace('\"\${BLKTRACE_WAS[@]:1:3}\"', '\"\${BLKTRACE_WAS[@]:2:2}\"')"
plant off-keeps-the-clock    $B "t.replace('\"\${BLKTRACE_WAS[@]:1:3}\"', '\"\${BLKTRACE_WAS[@]:1:1}\" \"\${BLKTRACE_WAS[@]:3:1}\"')"
plant off-leaves-events-on   $B "t.replace('\"\${BLKTRACE_WAS[@]:4}\" ', '')"
plant on-does-not-save       $B "t.replace('    BLKTRACE_WAS=(\"tracing_on=\$(blktrace_word \"\$t/tracing_on\")\"', '    BLKTRACE_WAS=(\"tracing_on=1\"')"

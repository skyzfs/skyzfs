#!/bin/bash
cd "$1" || exit 9
for t in tests/kernel/test_zfs_cli_record.sh tests/kernel/test_agent_record.sh tests/kernel/test_cut_record.sh tests/lib/test_lim.sh; do
    s=$(date +%s); out=$(timeout 900 bash "$t" 2>&1); rc=$?
    echo "== $t rc=$rc ok=$(grep -c '^ok' <<<"$out") fail=$(grep -c 'FAIL' <<<"$out") secs=$(($(date +%s)-s))"
    grep 'FAIL' <<<"$out" | head -20
    tail -n 2 <<<"$out"
done
for t in tests/kernel/test_agent_check.py tests/kernel/test_txg_probe.py tests/kernel/test_dsync_writer.py tests/kernel/test_zfs_cli_json.py; do
    out=$(timeout 600 python3 -I "$t" 2>&1); rc=$?
    echo "== $t rc=$rc $(tail -n 3 <<<"$out" | tr '\n' ' ')"
done

#!/bin/bash
# The local gates of #171 at a worktree: WT (the head) and BASE (zfs-topology).
WT=${1:?worktree} BASE=${2:?base worktree}
cd "$WT" || exit 1
echo "head: $(git log --oneline -1)"
for t in tests/kernel/test_agent_check.py tests/kernel/test_publish_check.py tests/kernel/test_publish_check_main.py \
         tests/kernel/test_nodeb_check.py tests/kernel/test_txg_probe.py tests/kernel/test_dsync_writer.py \
         tests/kernel/test_zfs_cli_json.py tests/kernel/test_cut_check.py tests/kernel/test_cut_daemon_check.py; do
    r=$(cd "$WT" && timeout 600 python3 -I "$t" 2>&1 | tail -n 3 | tr '\n' ' ')
    echo "$t: $r"
done
for t in tests/kernel/test_zfs_cli_record.sh tests/kernel/test_cut_record.sh tests/lib/test_lim.sh; do
    out=$(timeout 600 bash "$t" 2>&1 > /dev/null; echo "rc=$?")
    full=$(timeout 600 bash "$t" 2>&1 | tail -n 2 | tr '\n' ' ')
    echo "$t: $full"
done
for k in 1 2 3; do
    echo "tests/kernel/test_agent_record.sh run $k: $(timeout 600 bash tests/kernel/test_agent_record.sh 2>&1 | grep -E '^(FAIL|not ok|all passed)|fail' | head -5 | tr '\n' ' ')"
done
python3 -c 'import sys, yaml
for f in sys.argv[1:]:
    yaml.safe_load(open(f)); print("yaml ok:", f)' .github/workflows/skyzfs-agent-rig.yml .github/workflows/skyzfs-zfs-cli-record.yml
for f in tests/kernel/agent-record.sh tests/kernel/cut-record.sh tests/kernel/zfs-cli-record.sh \
         tests/kernel/test_agent_record.sh tests/kernel/test_zfs_cli_record.sh; do
    a=$(shellcheck -f gcc "$WT/$f" 2>&1 | sed 's/:[0-9]*:[0-9]*:/:/' | sed "s|$WT/||" | sort | uniq -c | sort)
    b=$(shellcheck -f gcc "$BASE/$f" 2>&1 | sed 's/:[0-9]*:[0-9]*:/:/' | sed "s|$BASE/||" | sort | uniq -c | sort)
    echo "shellcheck $f: head $(shellcheck -f gcc "$WT/$f" 2>&1 | wc -l) findings, base $(shellcheck -f gcc "$BASE/$f" 2>&1 | wc -l)"
    diff <(echo "$b") <(echo "$a") | sed 's/^/    /'
done

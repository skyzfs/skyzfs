#!/bin/bash
# The no-kernel gates of #178, run from a checkout at the head (b7202e1),
# as the publish rig's unit job and ci.yml run them. shellcheck here is
# the container's 0.9.0, older than CI's: its notes (SC2317 and others)
# are compared with the base's, not read as findings.
set -u
for t in "python3 tests/kernel/test_publish_agent.py" "python3 tests/kernel/test_publish_check.py" \
         "python3 tests/kernel/test_publish_check_main.py" "bash tests/kernel/test_cut_record.sh" \
         "bash tests/kernel/test_agent_record.sh"; do
    out=$(timeout 600 $t 2>&1); rc=$?
    echo "$t: exit $rc: $(printf '%s\n' "$out" | grep -E '^(Ran|OK|FAILED|all passed|FAIL)' | tr '\n' ' ')"
done

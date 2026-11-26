#!/bin/bash
# adump and scen_both of agent-record.sh at c580c72 run with stubs (no
# kernel, agent or daemon): what the recorder writes for the check.
# Usage: bash next_stubs.sh WORKTREE
SCRIPT=$1/tests/kernel/agent-record.sh
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fn() {
    awk -v h="$1() {" '!on && index($0, h) == 1 { on = 1; print; if ($0 ~ /}$/) exit; next }
        on { print; if ($0 == "}") exit }' "$SCRIPT"
}
eval "$(fn adump)"
eval "$(fn scen_both)"
printf '#!/bin/bash\necho "volume aaaa"\necho "skyzfs-agent: d: the record is corrupt" >&2\nexit 6\n' > "$T/agent"
chmod +x "$T/agent"
AGENT=$T/agent; ADIR=x; CDIR=$T
adump record-after
echo "adump, a record that does not read: txt '$(cat "$T/record-after.txt")', err '$(cat "$T/record-after.err")', rc $(cat "$T/record-after.rc")"
abg() { :; }; await_agent() { :; }; serve_again() { :; }
sleep 30 & SERVE_PID=$!; scen_both x; echo "scen_both, the daemon serving: $(tr '\t' ' ' < "$T/both-kill.tsv")"
( exit 0 ) & SERVE_PID=$!; sleep 0.3; scen_both x; echo "scen_both, the daemon had ended 0: $(tr '\t' ' ' < "$T/both-kill.tsv")"
( exit 7 ) & SERVE_PID=$!; sleep 0.3; scen_both x; echo "scen_both, the daemon had ended 7: $(tr '\t' ' ' < "$T/both-kill.tsv")"

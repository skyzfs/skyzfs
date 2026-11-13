#!/bin/bash
# say.sh RIG_DIR DMESG_FILE [mark]: run the rig's zfs_stuck_say here.
# The function is cut out of the rig; lim and klog_since are the rig's
# own; dmesg is a stand-in that prints DMESG_FILE.
set -uo pipefail
rig=$1 log=$2; shift 2
bin=$(mktemp -d); OUT=$(mktemp -d)
printf '#!/bin/sh\ncat "%s"\n' "$log" > "$bin/dmesg"; chmod +x "$bin/dmesg"
PATH=$bin:$PATH
. "$rig/tests/lib/lim.sh"
. "$rig/tests/kernel/klog.sh"
eval "$(sed -n '/^zfs_stuck_say() {$/,/^}$/p' "$rig/tests/kernel/daemon-rig.sh")"
declare -F zfs_stuck_say > /dev/null || { echo "the function was not found"; exit 2; }
t0=$(date +%s.%N)
zfs_stuck_say "$@"
echo "rc $? in $(echo "$(date +%s.%N) - $t0" | bc) s" >&2

#!/bin/bash
# say2.sh RIG_DIR DMESG_FILE [mark]: the rig's zfs_stuck_say from its own
# file (tests/kernel/zfs-stuck.sh), with dmesg a stand-in printing the file.
set -uo pipefail
rig=$1 log=$2; shift 2
OUT=$(mktemp -d) || exit 2
dmesg() { cat "$log"; }
. "$rig/tests/lib/lim.sh"
. "$rig/tests/kernel/klog.sh"
. "$rig/tests/kernel/zfs-stuck.sh"
zfs_stuck_say "$@"

#!/usr/bin/env bash
# The libublk spike (docs/skyzfs-ublk-spike.md, work package M1.3b):
# drives crates/skyzfs-ublk's `skyzfs-ublk-spike` through the questions
# of that file and prints what the kernel answered. It judges nothing:
# every line `spike_<name>=<value>` is an observation, and the checklist
# is filled in from them by hand.
#
#   sudo tests/kernel/ublk-spike.sh <path to skyzfs-ublk-spike> <out dir>
#
# Needs root, ublk_drv, fio and util-linux. No ZFS: a failed or lost
# write shows to fio directly, where a pool would hide it behind a
# retry.
set -u
S=$1
OUT=$2
mkdir -p "$OUT"
exec > >(tee "$OUT/spike.log") 2>&1

lim() { timeout --kill-after=5 "$@"; }
step() { echo; echo "== $*"; }
img() { echo "/dev/shm/skz-spike-$1.img"; }
# nodes N what: which of the device's nodes exist, to tell states apart
nodes() {
    local c=no b=no y=no
    [ -e "/dev/ublkc$1" ] && c=yes
    [ -e "/dev/ublkb$1" ] && b=yes
    [ -e "/sys/class/ublk-char/ublkc$1" ] && y=yes
    echo "spike_nodes_$2=ublkc$1 $c, ublkb$1 $b, sysfs ublk-char $y"
}
stats() { echo "spike_stats_$1=$(tr -d '\n' < "$(img "$2").stats" 2>/dev/null)"; }
# start N args...: a server in the background; waits for its ready line
SERVER_PID=""
start() {
    local how=$1 n=$2
    shift 2
    "$S" "$how" "$n" "$@" > "$OUT/serve-$n.$how.log" 2>&1 &
    SERVER_PID=$!
    for _ in $(seq 1 200); do
        grep -q '^ready ' "$OUT/serve-$n.$how.log" && { cat "$OUT/serve-$n.$how.log"; return 0; }
        kill -0 "$SERVER_PID" 2>/dev/null || break
        sleep 0.1
    done
    echo "spike_${how}_$n=NOT READY"; cat "$OUT/serve-$n.$how.log"
    return 1
}
stop() { # N: the orderly stop
    touch "$(img "$1").stop"
    for _ in $(seq 1 100); do kill -0 "$SERVER_PID" 2>/dev/null || break; sleep 0.1; done
    kill -0 "$SERVER_PID" 2>/dev/null && { echo "spike_stop_$1=STILL RUNNING after 10 s"; kill -9 "$SERVER_PID"; }
    wait "$SERVER_PID" 2>/dev/null
    echo "spike_stop_rc_$1=$?"
}

step "the kernel"
echo "spike_kernel=$(uname -r)"
[ -e /dev/ublk-control ] || modprobe ublk_drv || {
    apt-get install -y --no-install-recommends "linux-modules-extra-$(uname -r)" > "$OUT/apt.log" 2>&1
    modprobe ublk_drv
}
[ -e /dev/ublk-control ] || { echo "spike_ublk_drv=NO"; exit 2; }
echo "spike_ublk_params=$(grep -H . /sys/module/ublk_drv/parameters/* 2>/dev/null | sed 's|.*/||' | tr '\n' ' ')"

step "5: the feature mask, with no device; 13c: the header's constants"
lim 20 "$S" features
lim 20 "$S" consts

step "6, 9: a device that is added and never started; the control object dropped after disown()"
lim 20 "$S" info 1
lim 30 "$S" add 1 --no-start; echo "spike_add_no_start_rc=$?"
lim 20 "$S" info 1
nodes 1 added_never_started

step "9: kill -9 between the add and the start"
"$S" add 2 --no-start --linger 60 > "$OUT/add-2.log" 2>&1 &
p=$!
for _ in $(seq 1 50); do grep -q '^spike_add=' "$OUT/add-2.log" && break; sleep 0.1; done
cat "$OUT/add-2.log"
kill -9 "$p"; wait "$p" 2>/dev/null
sleep 0.5
lim 20 "$S" info 2

step "9: a control object that is NOT disowned deletes its device when dropped"
lim 60 "$S" add 5 --no-start --plain-delete-on-drop; echo "spike_add_owned_rc=$?"
lim 20 "$S" info 5

step "8: an add under a number that is taken: the driver's own answer, then the library's"
lim 20 "$S" rawinfo 7
lim 20 "$S" rawinfo 1
lim 20 "$S" rawadd 1
lim 20 "$S" rawinfo 1
lim 20 "$S" rawadd 6
lim 20 "$S" rawinfo 6
echo "spike_library_add_on_taken=follows; device 6 was added by this script and is nobody's to delete"
lim 30 "$S" add 6 --no-start; echo "spike_add_taken_rc=$?"
lim 20 "$S" rawinfo 6

step "10: the asynchronous delete of devices nobody holds"
lim 30 "$S" delete 1
lim 30 "$S" delete 2
sleep 0.5
lim 20 "$S" info 1
lim 20 "$S" info 2

step "1, 3, 7: a served device; requests of 4 KiB and 1 MiB"
start add 3 --size-mib 256 || exit 3
D=/dev/ublkb3
lim 20 "$S" info 3
nodes 3 live
echo "spike_sysfs_3=$(cd /sys/block/ublkb3/queue && for f in logical_block_size physical_block_size max_sectors_kb discard_max_bytes write_zeroes_max_bytes write_cache nr_requests; do printf '%s %s ' "$f" "$(cat $f)"; done)"
find /run/ublksrvd -maxdepth 1 -type f 2>/dev/null | sed 's/^/run-ublksrvd: /'
head -c $((8 << 20)) /dev/urandom > "$OUT/pattern"
lim 60 dd if="$OUT/pattern" of=$D bs=4k count=256 oflag=direct status=none; echo "spike_write_4k_rc=$?"
lim 60 dd if="$OUT/pattern" of=$D bs=1M count=8 seek=8 oflag=direct status=none; echo "spike_write_1m_rc=$?"
lim 60 dd if=$D bs=4k count=256 iflag=direct status=none | cmp -s - <(head -c $((1 << 20)) "$OUT/pattern"); echo "spike_read_4k_same=$?"
lim 60 dd if=$D bs=1M count=8 skip=8 iflag=direct status=none | cmp -s - "$OUT/pattern"; echo "spike_read_1m_same=$?"
lim 60 dd if="$OUT/pattern" of=$D bs=4M count=2 seek=8 oflag=direct status=none; echo "spike_write_4m_rc=$? (split by the kernel into requests of at most max_sectors)"
sleep 0.3; stats after_sizes 3

step "13: discard and write-zeroes arrive as their own operations"
lim 30 blkdiscard -o 0 -l $((1 << 20)) $D; echo "spike_blkdiscard_rc=$?"
lim 30 blkdiscard -z -o $((2 << 20)) -l $((1 << 20)) $D; echo "spike_blkdiscard_z_rc=$?"
lim 30 fallocate -p -o $((4 << 20)) -l $((1 << 20)) $D; echo "spike_fallocate_punch_rc=$?"
sync; sleep 0.3; stats after_discards 3

step "1: a verified write load through the worker thread"
lim 180 fio --name=load --filename=$D --direct=1 --ioengine=libaio --iodepth=32 --rw=randwrite --bs=4k-128k \
    --size=128M --verify=crc32c --do_verify=1 --verify_fatal=1 --output="$OUT/fio-load.log"
echo "spike_fio_load_rc=$?"
grep -E 'err=|WRITE:|READ:' "$OUT/fio-load.log" | sed 's/^/fio: /'
stats after_load 3

step "2: what wakes the queue with no traffic"
sleep 45
stats idle 3

step "15: an exclusive open, with nobody, with a plain holder, with an exclusive holder"
lim 20 "$S" excl $D
python3 -c 'import sys,time; f=open(sys.argv[1], "rb"); time.sleep(600)' $D &
HOLDER=$!
sleep 0.2
echo "spike_excl_case=plain holder"; lim 20 "$S" excl $D
python3 -c 'import os,sys,time; fd=os.open(sys.argv[1], os.O_RDONLY|os.O_EXCL); time.sleep(600)' $D &
XHOLDER=$!
sleep 0.5
echo "spike_excl_case=exclusive holder"; lim 20 "$S" excl $D
kill "$XHOLDER"; wait "$XHOLDER" 2>/dev/null

step "11, 14: kill -9 under a verified write load, then a take-over"
dmesg -C 2>/dev/null
for round in 1 2 3; do
    # The load runs for 12 s; the server is killed 3 s into it, and the
    # writes that were in flight wait for the next server.
    lim 300 fio --name=kill --filename=$D --direct=1 --ioengine=libaio --iodepth=32 --rw=randwrite --bs=4k-128k \
        --size=128M --time_based --runtime=12 --verify=crc32c --do_verify=1 --verify_fatal=1 --output="$OUT/fio-kill-$round.log" &
    FIO=$!
    sleep 3
    echo "spike_inflight_before_kill_$round=$(tr -s ' ' < /sys/block/ublkb3/inflight 2>/dev/null)"
    "$S" watch 3 4000 > "$OUT/watch-$round.log" 2>&1 &
    W=$!
    sleep 0.2
    echo "spike_kill_$round=at $(date +%s.%N)"
    kill -9 "$SERVER_PID"; wait "$SERVER_PID" 2>/dev/null
    wait "$W"; sed "s/^spike_watch=/spike_watch_$round=/" "$OUT/watch-$round.log"
    lim 20 "$S" info 3
    nodes 3 "waiting_$round"
    echo "spike_excl_case=waiting device, round $round"; lim 20 "$S" excl $D
    start takeover 3 || { echo "spike_takeover_$round=FAILED"; break; }
    wait "$FIO"; echo "spike_fio_kill_rc_$round=$?"
    grep -E 'err=|WRITE:|READ:' "$OUT/fio-kill-$round.log" | sed 's/^/fio: /'
    stats "after_kill_$round" 3
done
echo "spike_dmesg_io_errors=$(dmesg 2>/dev/null | grep -ciE 'I/O error|ublk.*(abort|fail)')"
dmesg 2>/dev/null | grep -iE 'I/O error|ublk' | tail -n 20 | sed 's/^/dmesg: /'

step "12: the parameters read back after a take-over"
lim 20 "$S" info 3

step "13b: a recovery that was begun, and the server died before it ended it"
kill -9 "$SERVER_PID"; wait "$SERVER_PID" 2>/dev/null
sleep 1
lim 20 "$S" info 3
lim 60 "$S" takeover 3 --die-after-begin; echo "spike_die_after_begin_rc=$?"
lim 20 "$S" info 3
start takeover 3 && echo "spike_takeover_after_begun=ok"
lim 20 "$S" info 3
lim 60 dd if=$D bs=1M count=4 iflag=direct status=none > /dev/null; echo "spike_read_after_second_takeover_rc=$?"

step "4: an orderly stop, with a holder; the state it leaves"
stop 3
grep '^spike_stop' "$OUT/serve-3.takeover.log"
sleep 0.5
lim 20 "$S" info 3
stats at_stop 3

step "6, 8, 10: the asynchronous delete of a device that is held; an add under its number"
lim 30 "$S" delete 3
lim 20 "$S" info 3
nodes 3 deleted_and_held
lim 20 "$S" rawinfo 3
lim 20 "$S" rawadd 3; echo "spike_rawadd_on_held_deleted_rc=$?"
lim 10 "$S" add 3 --no-start; echo "spike_add_on_held_deleted_rc=$? (124: it did not return in 10 s)"
lim 20 "$S" rawinfo 3
echo "spike_excl_case=deleted and held"; lim 20 "$S" excl $D
kill "$HOLDER"; wait "$HOLDER" 2>/dev/null
sleep 1
lim 20 "$S" info 3

step "13: write-zeroes with no limit set for it"
start add 4 --size-mib 64 --no-wz && {
    lim 30 blkdiscard -z -o 0 -l $((1 << 20)) /dev/ublkb4; echo "spike_no_wz_blkdiscard_z_rc=$?"
    lim 30 blkdiscard -o $((2 << 20)) -l $((1 << 20)) /dev/ublkb4; echo "spike_no_wz_blkdiscard_rc=$?"
    sleep 0.3; stats no_wz 4
    stop 4
    lim 30 "$S" delete 4
}

step "13a: the driver's limit on the number of devices"
first=""
for n in $(seq 10 90); do
    lim 30 "$S" add "$n" --no-start > "$OUT/limit-$n.log" 2>&1 || { first=$n; break; }
done
echo "spike_limit_first_refused=${first:-none up to 90}"
[ -z "$first" ] || grep '^spike_add=' "$OUT/limit-$first.log"
echo "spike_limit_devices_present=$(find /sys/class/ublk-char -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)"
for n in $(seq 10 90); do [ -e "/sys/class/ublk-char/ublkc$n" ] && lim 30 "$S" delete "$n" > /dev/null 2>&1; done
sleep 1
echo "spike_devices_left=$(find /sys/class/ublk-char -mindepth 1 -maxdepth 1 -printf '%f ' 2>/dev/null)"

step "done"
grep '^spike_' "$OUT/spike.log" > "$OUT/answers.txt"
exit 0

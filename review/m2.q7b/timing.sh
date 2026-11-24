#!/bin/bash
# Timing and repeat runs of the new tests, from the binaries bins.sh kept,
# idle and beside busy loops (8 and 16 on this machine's 4 CPUs).
# Usage: timing.sh OUTDIR CRATEDIR_SKYZFSD CRATEDIR_LOCAL
O=$1; DD=$2; LD=$3
B=$O/bin
R=$O/timing.out; : > $R
note() { echo "$*" >> $R; }
busy_on() { for i in $(seq 1 $1); do (timeout 3600 sh -c 'while :; do :; done # q7b-busy' &); done; sleep 1; note "-- $1 busy loops on; $(uptime)"; }
busy_off() { pkill -f "q7b-busy"; sleep 2; note "-- busy loops off; $(uptime)"; }
room() { # binary runs label
  (cd $DD && ROOM_RUNS=$2 timeout 900 $B/$1 --exact control::tests::socket::review_q7b_room_lateness --nocapture 2>&1) \
    | grep "REVIEW room" > "$O/room_$3.out"; note "room $3 ($1, $2 runs): $(grep summary "$O/room_$3.out")"
}
repeat() { # label n dir binary args...: how many runs failed
  local label=$1 n=$2 dir=$3 bin=$4; shift 4; local fail=0 i
  for i in $(seq 1 $n); do
    (cd $dir && timeout 600 $B/$bin "$@" > "$O/last_$label.out" 2>&1) || { fail=$((fail+1)); cp "$O/last_$label.out" "$O/fail_${label}_$i.out"; }
  done
  note "$label: $((n-fail)) of $n passed ($bin $*)"
}
NEW_SOCK=control::tests::socket::a_connect_is_made_soon_after_the_backlog_has_room
S1B=inplace::tests::cuts::a_release_on_a_failed_store_is_named_by_the_acknowledging_start_after_a_power_cut
FST=fs::tests::the_drop_of_a_file_waits_for_the_sync_its_helper_is_doing
note "idle: $(uptime)"
room room_head 30 head_idle
room room_CE 10 CE_idle
room room_CE5 10 CE5_idle
room room_CE8 10 CE8_idle
room room_CF 10 CF_idle
repeat s1b_idle 200 $LD l_head --exact $S1B
repeat fs_idle 100 $LD l_head --exact $FST
repeat fsJ_idle 50 $LD l_J --exact $FST
repeat sock_idle 10 $DD d_head control::tests::socket::
busy_on 8
room room_head 30 head_b8
room room_CE 10 CE_b8
repeat sock_b8 20 $DD d_head control::tests::socket::
repeat new_b8 20 $DD d_head --exact $NEW_SOCK
repeat dlib_b8 3 $DD d_head
repeat s1b_b8 200 $LD l_head --exact $S1B
repeat cuts_b8 2 $LD l_head inplace::tests::cuts::
repeat fs_b8 100 $LD l_head --exact $FST
repeat fsJ_b8 50 $LD l_J --exact $FST
busy_off
busy_on 16
room room_head 30 head_b16
room room_CE 10 CE_b16
repeat sock_b16 20 $DD d_head control::tests::socket::
repeat dlib_b16 3 $DD d_head
repeat s1b_b16 100 $LD l_head --exact $S1B
repeat fs_b16 50 $LD l_head --exact $FST
repeat fsJ_b16 50 $LD l_J --exact $FST
busy_off
note done

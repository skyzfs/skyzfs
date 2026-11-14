#!/usr/bin/env bash
# The control rig: what the daemon's binding to the ublk driver
# (crates/skyzfs-ublk-sys) says and does about a device in each state
# the driver knows, its own and another server's ("strangers", made
# with the ublk project's reference server). Design §4.2, "The states
# of a device" and "Library".
#
#   control-rig.sh OUT [WORK]            as root, on a machine to throw away
#
#   RIG_CONTROL       the test program skyzfs-ublk-control (a built binary)
#   RIG_KERNEL_TESTS  the built test binary of crates/skyzfs-ublk-sys/tests/kernel.rs
#   UBLKSRV_REF       the reference server's commit (default: the pinned one)
#
# OUT/results.tsv gets one line per result, `key<tab>value`:
#   control_<property>  PASS | FAIL: why     from the crate's own kernel tests
#   pin_<name>          PASS | FAIL | ERROR, then ": detail"
#   ctl_<name>          the same; a control is expected to FAIL
#   obs_<name>          an observation
# and tests/kernel/control_check.py judges them against its table. This
# script judges nothing beyond each line's own PASS or FAIL. ERROR is
# the rig's failure (a step it needed did not happen), never a verdict.
#
# Every step of the binding is one call of the test program, which
# prints the binding's answer in one line and decides nothing; the
# answers are all in OUT/steps.log, in order.
set -uo pipefail

# shellcheck source=tests/lib/gnu-dd.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib/gnu-dd.sh"
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
# shellcheck source=tests/kernel/ublksrvd-look.sh
. "$HERE/ublksrvd-look.sh"
# shellcheck source=tests/lib/net-try.sh
. "$HERE/../lib/net-try.sh"

OUT=${1:?usage: control-rig.sh OUT [WORK]}
WORK=${2:-/mnt/skz-control-rig}
[ "$(id -u)" = 0 ] || { echo "needs root" >&2; exit 64; }
mkdir -p "$OUT" "$WORK"
OUT=$(cd "$OUT" && pwd)
R=$OUT/results.tsv
: > "$R"; : > "$OUT/steps.log"; : > "$OUT/control.err"
export DEBIAN_FRONTEND=noninteractive
CTL=${RIG_CONTROL:-}
KTEST=${RIG_KERNEL_TESTS:-}

# The numbers: the binding's own devices, the strangers, and (from 200
# on) the crate's kernel tests. Far from 0, where anything else on the
# machine would start.
OWN=40; OWN2=41; OWN3=42; OWN4=43      # and 46 to 49: the idle and the fresh device of each delete under a held request
# A commit the driver refuses, of a read, a write and a flush: then a take-over, or a delete; and the control.
RC_READ=44; RC_READ_DEL=45; RC_WRITE=51; RC_WRITE_DEL=52; RC_FLUSH=53; RC_FLUSH_DEL=54; RC_CTL=50
S_LIVE=60; S_WAITING=61; S_NEVER=62; S_DELETED=63; S_CTL=64; S_FREE=65
SIZE=$((64 << 20))
HOLD_A=$((32 << 20))         # the byte whose read is held across a kill
HOLD_B=$((48 << 20))         # the byte whose read is held across the delete

# ------------------------------------------------------------ recording
rec() { # key value. Tabs and newlines in the value are flattened.
    local v=${2//$'\t'/ }
    v=${v//$'\n'/; }
    printf '%s\t%s\n' "$1" "$v" >> "$R"
    printf '== %s: %s\n' "$1" "$v"
}
pin() { rec "pin_$1" "$2: $3"; }     # name PASS|FAIL|ERROR detail
ctl() { rec "ctl_$1" "$2: $3"; }
obs() { rec "obs_$1" "$2"; }
pin_is() { # name detail test...: PASS if the test holds, else FAIL
    local name=$1 detail=$2; shift 2
    if "$@"; then pin "$name" PASS "$detail"; else pin "$name" FAIL "$detail"; fi
}
step() { printf '\n######## %s\n' "$*"; }
now_ms() { echo $(( $(date +%s%N) / 1000000 )); }

# ------------------------------------------------- the binding, by steps
# say STEP N [ARGS]: one step of the test program -> ANSWER (its answer
# without the step and the number), SAY_RC.
say() {
    local out
    out=$(lim 60 "$CTL" "$@" 2>> "$OUT/control.err"); SAY_RC=$?
    printf '%s\n' "$out" >> "$OUT/steps.log"
    ANSWER=$(printf '%s\n' "$out" | head -n 1 | cut -f3-)
    printf '   %s -> %s\n' "$*" "${ANSWER:-(no answer, exit $SAY_RC)}"
}
kv() { printf '%s\n' "$1" | tr ' ' '\n' | sed -n "s/^$2=//p" | head -n 1; }      # answer key
# info N -> ANSWER, and the state in STATE ("nothing" when there is none)
info() {
    say info "$1"
    if [ "$ANSWER" = nothing ]; then STATE=nothing; else STATE=$(kv "$ANSWER" state); fi
}
# wait_state N WANT SECONDS -> 0 when the binding's info says WANT; WAITED_MS
wait_state() {
    local t0; t0=$(now_ms)
    while :; do
        info "$1"; WAITED_MS=$(( $(now_ms) - t0 ))
        [ "$STATE" = "$2" ] && return 0
        [ "$WAITED_MS" -lt $(($3 * 1000)) ] || return 1
        sleep 0.1
    done
}
# free_for_add N SECONDS: "free" as a caller means it, an add under the
# number that succeeds. A lookup that finds nothing comes before that:
# in the crate's own test an add right after such a lookup was answered
# taken, 1 and 2 times of 39 on one image (runs 43079066283 and
# 43080463343), and succeeded at most 31 ms after the delete.
# -> 0 when an add succeeded within SECONDS; FREE_SAID says how. The
# device it added is deleted again.
free_for_add() {
    local t0 taken=0 ms; t0=$(now_ms)
    while :; do
        say add "$1" --depth 8; ms=$(( $(now_ms) - t0 ))
        if [ "$ANSWER" = added ] || [[ $ANSWER == added\ * ]]; then
            FREE_SAID="an add under $1 succeeded after $ms ms, answered taken $taken times before"
            say delete "$1"; wait_state "$1" nothing 10 || FREE_SAID+="; THE RIG'S OWN DELETE OF IT LEFT: $ANSWER"
            return 0
        fi
        [ "$ANSWER" = taken ] && taken=$((taken + 1))
        if [ "$ms" -ge $(($2 * 1000)) ]; then
            FREE_SAID="no add under $1 succeeded in $ms ms ($taken answered taken; the last answer: ${ANSWER:-none})"
            return 1
        fi
        sleep 0.01
    done
}
# serve_bg TAG STEP N ARGS: a server of the test program in the
# background -> SRV_PID, SRV_OUT, SRV_LINE (its first answer); 0 when
# that says the device is live.
serve_bg() {
    local tag=$1; shift
    SRV_OUT=$WORK/$tag.out; : > "$SRV_OUT"
    "$CTL" "$@" > "$SRV_OUT" 2>> "$OUT/control.err" &
    SRV_PID=$!
    for _ in $(seq 1 400); do
        [ -s "$SRV_OUT" ] && break
        kill -0 "$SRV_PID" 2>/dev/null || break
        sleep 0.1
    done
    sleep 0.1
    SRV_LINE=$(head -n 1 "$SRV_OUT" | cut -f3-)
    head -n 1 "$SRV_OUT" >> "$OUT/steps.log"
    printf '   %s -> %s (pid %s)\n' "$*" "${SRV_LINE:-(no answer)}" "$SRV_PID"
    case $SRV_LINE in "live after"*) return 0 ;; *) return 1 ;; esac
}
# PID: a kill -9 and a wait of at most 30 s. A server that the kill
# does not end (it sleeps in the driver) is left behind: the looks that
# follow then say what the device reads, and no bare `wait` holds the
# job to its limit.
srv_kill() {
    kill -9 "$1" 2>/dev/null
    for _ in $(seq 1 300); do kill -0 "$1" 2>/dev/null || break; sleep 0.1; done
    if kill -0 "$1" 2>/dev/null; then printf '   server %s had not ended 30 s after SIGKILL\n' "$1"; return 1; fi
    wait "$1" 2>/dev/null
    return 0
}
# srv_end PID OUT SECONDS: wait for a server to end -> SRV_SERVED, its last line; 1 if it did not end
srv_end() {
    for _ in $(seq 1 $(($3 * 10))); do kill -0 "$1" 2>/dev/null || break; sleep 0.1; done
    if kill -0 "$1" 2>/dev/null; then SRV_SERVED="still running after $3 s"; return 1; fi
    wait "$1" 2>/dev/null
    SRV_SERVED=$(sed -n '2p' "$2" | cut -f3-)
    sed -n '2p' "$2" >> "$OUT/steps.log"
    return 0
}
nodes_of() { local f o=""; for f in "/dev/ublkb$1" "/dev/ublkc$1"; do [ -e "$f" ] && o+="$f "; done; echo "${o:-none}"; }
handed_before() { printf '%s\n' "$1" | sed -n 's/.*handed before the start: \([0-9]*\).*/\1/p'; }

# ------------------------------------------------- a process that holds
# holder_start TAG DEVICE excl|plain -> H_PID, H_OUT, H_FD (commands go in there); 0 when it has the device open
holder_start() {
    H_FIFO=$WORK/$1.fifo; H_OUT=$WORK/$1.answers
    rm -f "$H_FIFO"; mkfifo "$H_FIFO"; : > "$H_OUT"
    python3 -I "$HERE/control-holder.py" "$2" "$3" < "$H_FIFO" > "$H_OUT" 2>> "$OUT/control.err" &
    H_PID=$!
    exec {H_FD}> "$H_FIFO"
    for _ in $(seq 1 100); do [ -s "$H_OUT" ] && break; sleep 0.1; done
    [ "$(head -n 1 "$H_OUT")" = open ]
}
holder_cmd() { echo "$1" >&"$H_FD"; }
# holder_answer LINE SECONDS: the holder's answer in that line, once it is there
holder_answer() {
    for _ in $(seq 1 $(($2 * 10))); do
        [ "$(wc -l < "$H_OUT")" -ge "$1" ] && break
        sleep 0.1
    done
    sed -n "${1}p" "$H_OUT"
}
holder_end() {
    holder_cmd close 2>/dev/null
    exec {H_FD}>&-
    for _ in $(seq 1 50); do kill -0 "$H_PID" 2>/dev/null || break; sleep 0.1; done
    kill -9 "$H_PID" 2>/dev/null
    H_PID=""
}

# ---------------------------------------------- the reference server
ref_list() { lim 20 ublk list -n "$1" 2>/dev/null; }
ref_state() { ref_list "$1" | sed -n 's/.*state \([A-Z_]*\).*/\1/p' | head -n 1; }
ref_pid() { ref_list "$1" | sed -n 's/.*daemon pid \([0-9]*\).*/\1/p' | head -n 1; }
ref_add() { # N: a device of the reference server, live, that can wait for a server
    truncate -s "$SIZE" "$WORK/stranger-$1.img"
    lim 60 ublk add -t loop -n "$1" -f "$WORK/stranger-$1.img" -q 1 -d 32 -r 1 -i 1 > "$OUT/stranger-$1-add.log" 2>&1
    for _ in $(seq 1 50); do
        [ -b "/dev/ublkb$1" ] && [ "$(ref_state "$1")" = LIVE ] && return 0
        sleep 0.1
    done
    return 1
}
# look N -> LOOK: what the driver holds of the device, by the binding's
# raw values and by the reference tool, without the lines that change
# by themselves (a queue's thread).
look() {
    local a
    info "$1"; a=$ANSWER
    LOOK="binding: raw_state=$(kv "$a" raw_state) raw_flags=$(kv "$a" raw_flags) max_io=$(kv "$a" max_io) server_pid=$(kv "$a" server_pid) size=$(kv "$a" size) queues=$(kv "$a" queues) depth=$(kv "$a" depth) state=${STATE}; ublk list: $(ref_list "$1" | grep -v '^[[:space:]]*queue ' | tr -s ' \t\n' ' ')"
}
# stranger NAME N WANT_STATE: the three pins of a stranger that is set
# up under number N. -> ST_BEFORE, ST_AFTER
stranger() {
    local name=$1 n=$2 want=$3 taken
    look "$n"; ST_BEFORE=$LOOK
    say add "$n" --depth 8; taken=$ANSWER
    look "$n"; ST_AFTER=$LOOK
    pin_is "add_on_${name}_stranger_is_taken" "the binding's add under number $n answered: ${taken:-nothing}" [ "$taken" = taken ]
    ST_EXTRA=""
    if [ "$name" = live ]; then
        if lim 30 "$DD" if=/dev/urandom of="/dev/ublkb$n" bs=4k count=4 oflag=direct conv=nocreat status=none 2>> "$OUT/control.err" \
            && lim 30 "$DD" if="/dev/ublkb$n" of=/dev/null bs=4k count=4 iflag=direct status=none 2>> "$OUT/control.err"; then
            ST_EXTRA="; a write and a read through it succeeded"
        else
            ST_EXTRA="; A WRITE OR A READ THROUGH IT FAILED"
        fi
    fi
    if [ "$ST_BEFORE" = "$ST_AFTER" ] && [ "$ST_EXTRA" != "; A WRITE OR A READ THROUGH IT FAILED" ]; then
        pin "${name}_stranger_untouched" PASS "before and after the refused add: $ST_AFTER$ST_EXTRA"
    else
        pin "${name}_stranger_untouched" FAIL "before: $ST_BEFORE; after: $ST_AFTER$ST_EXTRA"
    fi
    pin_is "info_of_${name}_stranger" "the binding reports state=$STATE (wanted $want); the reference tool: ${ST_AFTER#*ublk list: }" [ "$STATE" = "$want" ]
}
strangers_error() { # name why: the stranger could not be set up
    pin "add_on_${1}_stranger_is_taken" ERROR "$2"
    pin "${1}_stranger_untouched" ERROR "$2"
    pin "info_of_${1}_stranger" ERROR "$2"
}

# shellcheck disable=SC2317,SC2329  # reached through the trap
cleanup() {
    set +e
    local n
    [ -z "${H_PID:-}" ] || kill -9 "$H_PID" 2>/dev/null
    pkill -9 -f "$CTL serve" 2>/dev/null; pkill -9 -f "$CTL take-over" 2>/dev/null
    for n in $OWN $OWN2 $OWN3 $OWN4 46 47 48 49 $RC_READ $RC_READ_DEL $RC_WRITE $RC_WRITE_DEL $RC_FLUSH $RC_FLUSH_DEL $RC_CTL $S_NEVER $S_FREE; do [ -z "$CTL" ] || lim 20 "$CTL" delete "$n" > /dev/null 2>&1; done
    command -v ublk > /dev/null && lim 60 ublk del -a --async 2>/dev/null
    dmesg > "$OUT/dmesg.txt" 2>/dev/null
    chmod -R a+rX "$OUT"
}
trap cleanup EXIT

# =============================================================== setup
step "setup"
rec kernel "$(uname -r)"
# shellcheck disable=SC1091
rec os "$(. /etc/os-release && echo "$PRETTY_NAME")"
SETUP_OK=1
# Installing packages and cloning need the network, and the runners'
# name resolution fails for minutes at a time: three tries, a minute
# apart. A setup that still fails is the rig's error, with its reason.
built=""
for try in 1 2 3; do
    apt-get update > "$OUT/apt.log" 2>&1
    if "$HERE/build-ublksrv.sh" "${UBLKSRV_REF:-6d0d922614ec18e6f2b5d476af8d540a1d9c1de8}" > "$OUT/ublksrv.ver" 2> "$OUT/ublksrv-build.log"; then built=1; break; fi
    cp "$OUT/ublksrv-build.log" "$OUT/ublksrv-build-try$try.log"
    [ "$try" = 3 ] || sleep 60
done
if [ -n "$built" ]; then
    rec ublk_server "$(tail -n 1 "$OUT/ublksrv.ver") (built at try $try)"
else
    rec ublk_server "BUILD FAILED in three tries: $(net_why "$OUT/ublksrv-build.log" "$OUT/apt.log")"; SETUP_OK=""
fi
modprobe ublk_drv 2>> "$OUT/control.err" || true
[ -e /dev/ublk-control ] || { rec ublk_driver "no /dev/ublk-control after modprobe ublk_drv"; SETUP_OK=""; }
{ [ -n "$CTL" ] && [ -x "$CTL" ]; } || { rec test_program "RIG_CONTROL is not a program: '${CTL}'"; SETUP_OK=""; }
{ [ -n "$KTEST" ] && [ -x "$KTEST" ]; } || { rec kernel_tests "RIG_KERNEL_TESTS is not a program: '${KTEST}'"; SETUP_OK=""; }
if [ -z "$SETUP_OK" ]; then
    rec setup "FAILED: $(grep -E '^(ublk_server|ublk_driver|test_program|kernel_tests)\b' "$R" | grep -E 'FAILED|no /dev|is not a program' | cut -f2 | tr '\n' ';')"
    exit 2
fi
rec setup ok
T0=$(( $(date +%s) - 1 ))       # for the look at /run/ublksrvd at the end

# ================================================ the crate's own tests
# They need no stranger. Each prints its own line; a test that printed
# none is missing from the results and the judge says so.
step "the binding's own kernel tests"
# Ten minutes, and the output shown as it comes: one of these tests
# hung once with a defect put back, and a kill did not end the process
# (run 43076882940). A run that hangs has then said what it saw, and
# `lim` gives up on a process that cannot be killed. The exit status is
# judged: a test can fail without a FAIL line of its own (a panic
# outside its verdict), and did under a green job twice.
: > "$OUT/kernel-tests.log"
tail -n +1 -f "$OUT/kernel-tests.log" 2>/dev/null & TAIL_PID=$!
# shellcheck disable=SC2016  # the inner shell expands them
lim 600 bash -c 'exec "$0" --ignored --test-threads 1 --nocapture > "$1" 2>&1' "$KTEST" "$OUT/kernel-tests.log"
KT_RC=$?
sleep 0.5; kill "$TAIL_PID" 2>/dev/null; wait "$TAIL_PID" 2>/dev/null
rec kernel_tests_exit "$KT_RC"
KT_END=$(grep -a '^test result:' "$OUT/kernel-tests.log" | tail -n 1)
case $KT_RC in
    0) pin kernel_tests_end PASS "the test binary ended with 0: ${KT_END:-no result line}" ;;
    124) pin kernel_tests_end FAIL "the test binary had not ended after 600 s and was given up on; its last line: $(tail -n 1 "$OUT/kernel-tests.log" | cut -c1-300)" ;;
    *) pin kernel_tests_end FAIL "the test binary ended with $KT_RC: ${KT_END:-no result line}; failed: $(grep -a -E '^test .* FAILED|panicked at' "$OUT/kernel-tests.log" | head -n 5 | tr '\n' ';' | cut -c1-600)" ;;
esac
# With --nocapture a test's first line follows the harness's own
# "test NAME ... " on the same line, so a line is taken from where its
# key starts and not only from the start of a line (run 43070686788
# lost five verdicts that way).
grep -a -o $'control_[a-z0-9_]*\t.*' "$OUT/kernel-tests.log" >> "$R"
echo "== the lines taken from it:"
grep -a -o $'control_[a-z0-9_]*\t.*' "$OUT/kernel-tests.log" | sed 's/^/== /'
# The aborts at a stop (M1.3ac, #71): the crate's test of the program's
# own delete, with nothing held, prints how many of the queue's commands
# the driver ended with its abort. The device has a depth of 8 and the
# driver aborts the command waiting on every tag (ublk_cancel_dev at
# v6.11, v6.17 and v7.0), so the count is 8: the daemon's
# commands_aborted counts what this shows. After SIGTERM the server
# returns before the driver aborts anything, so a count there is 0 by
# construction and pins nothing.
v=$(awk -F'\t' '$1 == "control_commands_aborted_at_stop_detail" { print $2 }' "$R" | tail -n 1)
if [ -n "$v" ]; then
    pin_is commands_aborted_at_a_stop "the driver ended $v of the queue's commands with its abort when the program's own delete stopped the device (its depth: 8)" [ "$v" = 8 ]
elif [ "$KT_RC" = 0 ]; then
    pin commands_aborted_at_a_stop ERROR "the kernel tests ended with 0, and the rig found no count in their output"
else
    pin commands_aborted_at_a_stop FAIL "no count: the kernel tests ended with $KT_RC"
fi

# ====================================================== its own device
own_device() {
    local a before after t0 ms srv srv_out v rd rd_rc want got held_line later_line k r
    truncate -s "$SIZE" "$WORK/own.img"
    # Known content where reads will be held: the file is the server's store.
    "$DD" if=/dev/urandom of="$WORK/own.img" bs=4k count=1 seek=$((HOLD_A / 4096)) conv=notrunc status=none
    want=$("$DD" if="$WORK/own.img" bs=4k count=1 skip=$((HOLD_A / 4096)) status=none | sha256sum | cut -d' ' -f1)

    step "own: a free number"
    say begin-recovery "$OWN"
    pin_is begin_recovery_on_free_says_nothing "begin-recovery on the free number $OWN answered: ${ANSWER:-nothing}" [ "$ANSWER" = nothing ]

    step "own: added, never started, its adder gone"
    say add "$OWN" --depth 8
    if [ "$ANSWER" != added ] && [[ $ANSWER != added\ * ]]; then
        for v in added_device_is_dead_when_its_adder_ends begin_recovery_on_never_started_says_busy orderly_stop_leaves_waiting \
            killed_server_device_waits take_over_hands_nothing_before_the_end take_over_reissues_a_request_in_flight \
            abandoned_recovery_leaves_waiting begin_recovery_on_live_says_busy delete_returns_while_held \
            deleted_held_is_dead_and_keeps_number delete_fails_requests number_free_when_holder_lets_go \
            dead_after_delete_is_not_taken_over client_holds_exclusive_holder client_holds_plain_reader_unknown \
            set_params_refused_once_started; do
            pin "$v" ERROR "the binding's add under the free number $OWN answered: ${ANSWER:-nothing}"
        done
        obs killed_server_live_ms "not run: no device"; obs orderly_stop_live_ms "not run: no device"
        obs flags_beyond_the_mask "not run: no device"
        obs serve_of_a_device_whose_adder_ended "not run: no device"
        OWN_PID_OK="not run: no device"
        return
    fi
    info "$OWN"; a=$ANSWER
    pin_is added_device_is_dead_when_its_adder_ends "after the adding process ended, info of $OWN: $a" [ "$STATE" = dead ]
    obs flags_beyond_the_mask "raw_flags $(kv "$a" raw_flags), under the mask $(kv "$a" flags); raw_state $(kv "$a" raw_state), max_io $(kv "$a" max_io)"
    before=$a
    say begin-recovery "$OWN"; v=$ANSWER
    info "$OWN"
    { if [ "$v" = busy ] && [ "$ANSWER" = "$before" ]; then r=PASS; else r=FAIL; fi; pin begin_recovery_on_never_started_says_busy "$r" "begin-recovery on the never-started $OWN answered: ${v:-nothing}; info before: $before; after: $ANSWER"; }

    step "own: another process starts it"
    if serve_bg own-1 serve "$OWN" --file "$WORK/own.img"; then
        obs serve_of_a_device_whose_adder_ended "another process started it: $SRV_LINE"
    else
        obs serve_of_a_device_whose_adder_ended "another process could not start it: ${SRV_LINE:-no answer}"
        for v in orderly_stop_leaves_waiting killed_server_device_waits take_over_hands_nothing_before_the_end \
            take_over_reissues_a_request_in_flight abandoned_recovery_leaves_waiting begin_recovery_on_live_says_busy \
            delete_returns_while_held deleted_held_is_dead_and_keeps_number delete_fails_requests \
            number_free_when_holder_lets_go dead_after_delete_is_not_taken_over client_holds_exclusive_holder \
            client_holds_plain_reader_unknown set_params_refused_once_started; do
            pin "$v" ERROR "the device $OWN was not started: ${SRV_LINE:-no answer}"
        done
        obs killed_server_live_ms "not run: no server"; obs orderly_stop_live_ms "not run: no server"
        OWN_PID_OK="not run: no server"
        return
    fi
    srv=$SRV_PID; srv_out=$SRV_OUT

    step "own: live"
    info "$OWN"; before=$ANSWER
    if [ "$(kv "$before" server_pid)" = "$srv" ]; then OWN_PID_OK="yes: server_pid $srv is the serving process"
    else OWN_PID_OK="NO: server_pid $(kv "$before" server_pid), the serving process is $srv"; fi
    say begin-recovery "$OWN"; v=$ANSWER
    info "$OWN"; after=$ANSWER
    k=fail
    lim 30 "$DD" if=/dev/urandom of="/dev/ublkb$OWN" bs=4k count=4 oflag=direct conv=nocreat status=none 2>> "$OUT/control.err" \
        && lim 30 "$DD" if="/dev/ublkb$OWN" of=/dev/null bs=4k count=4 iflag=direct status=none 2>> "$OUT/control.err" && k=ok
    { if [ "$v" = busy ] && [ "$k" = ok ] && [ "$before" = "$after" ]; then r=PASS; else r=FAIL; fi; pin begin_recovery_on_live_says_busy "$r" "begin-recovery on the live $OWN answered: ${v:-nothing}; a write and a read after it: $k; info before: $before; after: $after"; }

    step "own: who holds it"
    say client-holds "$OWN"; v=$ANSWER
    if holder_start hold-excl "/dev/ublkb$OWN" excl; then
        say client-holds "$OWN"
        pin_is client_holds_exclusive_holder "with a process holding /dev/ublkb$OWN open with O_EXCL: ${ANSWER:-nothing} (with nobody: ${v:-nothing})" [ "$ANSWER" = held ]
    else
        pin client_holds_exclusive_holder ERROR "the holder could not open the device: $(head -n 1 "$H_OUT")"
    fi
    holder_end
    if holder_start hold-plain "/dev/ublkb$OWN" plain; then
        say client-holds "$OWN"
        pin_is client_holds_plain_reader_unknown "with a process holding /dev/ublkb$OWN open without O_EXCL: ${ANSWER:-nothing}" [ "$ANSWER" = unknown ]
    else
        pin client_holds_plain_reader_unknown ERROR "the holder could not open the device: $(head -n 1 "$H_OUT")"
    fi
    holder_end

    step "own: an orderly stop"
    # The driver moves the device to waiting in a work item it queues
    # when the server's char device is released, and with requests
    # fetched through an io_uring that release can come from the ring's
    # teardown after the process has gone (drivers/block/ublk_drv.c,
    # ublk_ch_release and ublk_ch_release_work_fn). So the end of the
    # process does not mean the device waits yet: it is given the kill's
    # wait, the daemon's limit for a take-over (5 s), and the time is an
    # observation.
    kill -TERM "$srv"
    if srv_end "$srv" "$srv_out" 30; then
        if wait_state "$OWN" waiting 5; then
            obs orderly_stop_live_ms "$WAITED_MS ms from the server's end until the binding's lookup said waiting"
        else
            obs orderly_stop_live_ms "after $WAITED_MS ms the lookup still said $STATE"
        fi
        { if [ "$STATE" = waiting ] && [ -b "/dev/ublkb$OWN" ]; then r=PASS; else r=FAIL; fi; pin orderly_stop_leaves_waiting "$r" "after SIGTERM the server said: ${SRV_SERVED:-nothing}; $WAITED_MS ms after its end, info: $ANSWER; /dev/ublkb$OWN $([ -b "/dev/ublkb$OWN" ] && echo 'is there' || echo 'IS GONE')"; }
    else
        pin orderly_stop_leaves_waiting ERROR "the server did not end within 30 s of SIGTERM"
        obs orderly_stop_live_ms "not run: the server did not end"
        kill -9 "$srv" 2>/dev/null
    fi

    step "own: parameters once started"
    info "$OWN"; before=$ANSWER
    say set-params "$OWN" --size $((SIZE * 2)); v=$ANSWER
    info "$OWN"
    { if [ -n "$v" ] && [ "$v" != set ] && [ "$(kv "$before" size)" = "$(kv "$ANSWER" size)" ] && [ -n "$(kv "$ANSWER" size)" ]; then r=PASS; else r=FAIL; fi; pin set_params_refused_once_started "$r" "set-params with $((SIZE * 2)) bytes on the waiting $OWN answered: ${v:-nothing}; size before $(kv "$before" size), after $(kv "$ANSWER" size)"; }

    step "own: a recovery begun and abandoned"
    say begin-recovery "$OWN"; v=$ANSWER
    info "$OWN"; before=$STATE
    if serve_bg own-2 take-over "$OWN" --file "$WORK/own.img"; then
        { if [ "$v" = begun ] && [ "$before" = waiting ]; then r=PASS; else r=FAIL; fi; pin abandoned_recovery_leaves_waiting "$r" "begin-recovery answered: ${v:-nothing}, and its process ended; the device then read $before; the next take-over: $SRV_LINE"; }
    else
        pin abandoned_recovery_leaves_waiting FAIL "begin-recovery answered: ${v:-nothing}; the device then read $before; the next take-over: ${SRV_LINE:-no answer}"
        for v in killed_server_device_waits take_over_hands_nothing_before_the_end take_over_reissues_a_request_in_flight \
            delete_returns_while_held deleted_held_is_dead_and_keeps_number delete_fails_requests \
            number_free_when_holder_lets_go dead_after_delete_is_not_taken_over; do
            pin "$v" ERROR "no take-over after the abandoned recovery: ${SRV_LINE:-no answer}"
        done
        obs killed_server_live_ms "not run: no server"
        return
    fi
    srv=$SRV_PID; srv_out=$SRV_OUT

    step "own: a kill, then a take-over with a reader waiting"
    srv_kill "$srv"
    if wait_state "$OWN" waiting 10; then
        obs killed_server_live_ms "$WAITED_MS ms from the kill until the binding's lookup said waiting"
        pin_is killed_server_device_waits "waiting $WAITED_MS ms after SIGKILL (the daemon's limit is 5000)" [ "$WAITED_MS" -le 5000 ]
    else
        obs killed_server_live_ms "after $WAITED_MS ms the lookup still said $STATE"
        pin killed_server_device_waits FAIL "$WAITED_MS ms after SIGKILL the device read $STATE"
    fi
    # A reader on the waiting device: its request is the one a server
    # must not be handed before the recovery has ended.
    ( lim 120 "$DD" if="/dev/ublkb$OWN" of=/dev/null bs=4k count=1 iflag=direct status=none 2> "$WORK/rd1.err"; echo $? > "$WORK/rd1.rc" ) &
    rd=$!
    sleep 1
    if serve_bg own-3 take-over "$OWN" --file "$WORK/own.img" --recovery-wait-ms 2000 --hold-one-request "120:read:$HOLD_A"; then
        wait "$rd"; rd_rc=$(cat "$WORK/rd1.rc" 2>/dev/null)
        k=$(handed_before "$SRV_LINE")
        { if [ "$k" = 0 ] && [ "$rd_rc" = 0 ]; then r=PASS; else r=FAIL; fi; pin take_over_hands_nothing_before_the_end "$r" "with 2 s between the queue being ready and the end of the recovery and a reader waiting: $SRV_LINE; the reader exited ${rd_rc:-?}"; }
    else
        pin take_over_hands_nothing_before_the_end ERROR "the take-over did not go live: ${SRV_LINE:-no answer}"
        for v in take_over_reissues_a_request_in_flight delete_returns_while_held deleted_held_is_dead_and_keeps_number \
            delete_fails_requests number_free_when_holder_lets_go dead_after_delete_is_not_taken_over; do
            pin "$v" ERROR "no take-over after the kill: ${SRV_LINE:-no answer}"
        done
        return
    fi
    srv=$SRV_PID; srv_out=$SRV_OUT

    step "own: a read in flight at a kill is issued again"
    ( lim 180 "$DD" if="/dev/ublkb$OWN" of="$WORK/rd2.bin" bs=4k count=1 skip=$((HOLD_A / 4096)) iflag=direct status=none 2> "$WORK/rd2.err"; echo $? > "$WORK/rd2.rc" ) &
    rd=$!
    sleep 2
    if [ -e "$WORK/rd2.rc" ]; then
        pin take_over_reissues_a_request_in_flight ERROR "the read that was to be held returned at once (exit $(cat "$WORK/rd2.rc")): the server held nothing"
    fi
    srv_kill "$srv"
    wait_state "$OWN" waiting 10
    if serve_bg own-4 take-over "$OWN" --file "$WORK/own.img"; then
        wait "$rd"; rd_rc=$(cat "$WORK/rd2.rc" 2>/dev/null)
        got=$(sha256sum < "$WORK/rd2.bin" 2>/dev/null | cut -d' ' -f1)
        grep -q '^pin_take_over_reissues_a_request_in_flight' "$R" || \
            { if [ "$rd_rc" = 0 ] && [ "$got" = "$want" ]; then r=PASS; else r=FAIL; fi; pin take_over_reissues_a_request_in_flight "$r" "the read held by the killed server exited ${rd_rc:-?} after the take-over ($SRV_LINE); its data is $([ "$got" = "$want" ] && echo 'what the file holds' || echo 'NOT what the file holds')"; }
    else
        grep -q '^pin_take_over_reissues_a_request_in_flight' "$R" || pin take_over_reissues_a_request_in_flight ERROR "the take-over did not go live: ${SRV_LINE:-no answer}"
        for v in delete_returns_while_held deleted_held_is_dead_and_keeps_number delete_fails_requests \
            number_free_when_holder_lets_go dead_after_delete_is_not_taken_over; do
            pin "$v" ERROR "no take-over after the second kill: ${SRV_LINE:-no answer}"
        done
        return
    fi
    srv=$SRV_PID; srv_out=$SRV_OUT

    # The server holds no request here. A delete while a live server
    # holds one waits for that request (run 43070686788: no answer in
    # 60 s against a hold of 120 s), which is the driver's doing and is
    # observed on its own device further down.
    step "own: a delete while a process holds the device open"
    if ! holder_start hold-del "/dev/ublkb$OWN" plain; then
        for v in delete_returns_while_held deleted_held_is_dead_and_keeps_number delete_fails_requests \
            number_free_when_holder_lets_go dead_after_delete_is_not_taken_over; do
            pin "$v" ERROR "the holder could not open the device: $(head -n 1 "$H_OUT")"
        done
        holder_end; kill -TERM "$srv" 2>/dev/null
        return
    fi
    holder_cmd "read $HOLD_B"          # answered: the device works for the holder before the delete
    held_line=$(holder_answer 2 30)
    t0=$(now_ms)
    say delete "$OWN"; v=$ANSWER
    ms=$(( $(now_ms) - t0 ))
    DELETE_HELD="held: ${v:-no answer} ($ms ms by the rig's clock)"
    { if [[ "$v" == returned\ after\ * ]] && [ "$ms" -lt 2000 ]; then r=PASS; else r=FAIL; fi; pin delete_returns_while_held "$r" "the binding's delete of the held $OWN answered: ${v:-nothing}, after $ms ms by the rig's clock"; }
    holder_cmd "read 0"
    later_line=$(holder_answer 3 30)
    { if [[ "$held_line" == read\ *\ ok\ * ]] && [[ "$later_line" == read\ *\ failed:* ]]; then r=PASS; else r=FAIL; fi; pin delete_fails_requests "$r" "the holder's read before the delete: ${held_line:-no answer in 30 s}; its read after it: ${later_line:-no answer in 30 s}"; }
    info "$OWN"; before=$ANSWER; got=$STATE
    say add "$OWN" --depth 8; v=$ANSWER
    { if [ "$got" = dead ] && [ ! -e "/dev/ublkb$OWN" ] && [ ! -e "/dev/ublkc$OWN" ] && [ "$v" = taken ]; then r=PASS; else r=FAIL; fi; pin deleted_held_is_dead_and_keeps_number "$r" "info: $before; nodes: $(nodes_of "$OWN"); an add under the number answered: ${v:-nothing}"; }
    say begin-recovery "$OWN"; v=$ANSWER
    info "$OWN"
    { if [ -n "$v" ] && [ "$v" != begun ] && [ "$ANSWER" = "$before" ]; then r=PASS; else r=FAIL; fi; pin dead_after_delete_is_not_taken_over "$r" "begin-recovery on the deleted and held $OWN answered: ${v:-nothing}; info before: $before; after: $ANSWER"; }
    srv_end "$srv" "$srv_out" 30 || kill -9 "$srv" 2>/dev/null
    echo "   the server of the deleted device: ${SRV_SERVED:-nothing}"
    holder_end
    if wait_state "$OWN" nothing 10; then
        ms=$WAITED_MS
        if free_for_add "$OWN" 5; then r=PASS; else r=FAIL; fi
        pin number_free_when_holder_lets_go "$r" "$ms ms after the holder closed the device, a lookup of $OWN says nothing is there; then $FREE_SAID"
    else
        pin number_free_when_holder_lets_go FAIL "$WAITED_MS ms after the holder closed the device, info of $OWN: $ANSWER"
    fi
}
DELETE_HELD="held: not run"; OWN_PID_OK="not run"
own_device

# A delete of a live device whose server holds a request. Observed, not
# judged until it has been measured: the design's row is about a device
# a client holds, and the first run showed that this is another matter
# (run 43070686788: the delete did not answer in 60 s while the server
# held a read for 120 s, and the read did not fail meanwhile).
#
# delete_under_a_held_request NAME NUMBER HOLD_S KILL_AFTER_S: the
# server holds one read for HOLD_S; the delete is sent 2 s into the
# hold. With KILL_AFTER_S, the server is killed that long after the
# delete was sent, while the delete waits: what the daemon's own
# failure path would meet if its server went away under a delete. The
# line says how long the delete took, what the device was a second
# after the delete was sent and at the end, and what the reader got.
delete_under_a_held_request() {
    local name=$1 n=$2 hold=$3 kill_after=${4:-} t0 t1 during after line ms v killed="" idle=$(($2 + 4)) fresh=$(($2 + 6)) k
    # While the delete waits: a lookup of this device, a lookup of a
    # free number, an add under a fresh number, and a delete of a
    # second device that nobody serves or holds. By a reading of the
    # driver's source the delete waits holding the device's own lock
    # and the lock that every add and delete takes, so the last two
    # would wait behind it, on any number: one stuck volume would hold
    # up every other volume's start and detach on the node.
    say add "$idle" --depth 8
    truncate -s "$SIZE" "$WORK/$name.img"
    say add "$n" --depth 8
    if ! serve_bg "$name" serve "$n" --file "$WORK/$name.img" --hold-one-request "$hold:read:$HOLD_B"; then
        obs "$name" "not run: the device $n was not served: ${SRV_LINE:-no answer}"; return
    fi
    if ! holder_start "hold-$name" "/dev/ublkb$n" plain; then
        obs "$name" "not run: the holder could not open the device: $(head -n 1 "$H_OUT")"
        holder_end; kill -TERM "$SRV_PID" 2>/dev/null; return
    fi
    t1=$(now_ms)
    holder_cmd "read $HOLD_B"          # held by the server: in flight at the delete
    sleep 2
    rm -f "$WORK/$name.del" "$WORK/$name.del-ms"
    t0=$(now_ms)
    # In the background: the rig must be able to look, and to kill the
    # server, while the delete waits.
    { lim 90 "$CTL" delete "$n" > "$WORK/$name.del" 2>> "$OUT/control.err"; echo $(( $(now_ms) - t0 )) > "$WORK/$name.del-ms"; } &
    sleep 1
    # Each in the background with its own clock. (In run 43073076409
    # the lookup of this device, made in line, did not come back until
    # the delete had, so the server was never killed.)
    for k in "look info $n" "free info $S_FREE" "add add $fresh --depth 8" "idle delete $idle"; do
        # shellcheck disable=SC2086  # the words of a command
        set -- $k; k=$1; shift
        rm -f "$WORK/$name.$k" "$WORK/$name.$k-ms"
        { lim 90 "$CTL" "$@" > "$WORK/$name.$k" 2>> "$OUT/control.err"; echo $(( $(now_ms) - t0 - 1000 )) > "$WORK/$name.$k-ms"; } &
    done
    if [ -n "$kill_after" ]; then
        sleep $((kill_after - 1))
        if [ -e "$WORK/$name.del-ms" ]; then killed="; the delete had answered before the server was to be killed"
        else killed="; the server was killed $(( $(now_ms) - t0 )) ms after the delete was sent"; fi
        kill -9 "$SRV_PID" 2>/dev/null
    fi
    for _ in $(seq 1 900); do [ -e "$WORK/$name.del-ms" ] && break; sleep 0.1; done
    ms=$(cat "$WORK/$name.del-ms" 2>/dev/null); v=$(head -n 1 "$WORK/$name.del" 2>/dev/null | cut -f3-)
    cat "$WORK/$name.del" >> "$OUT/steps.log" 2>/dev/null
    line=$(holder_answer 2 60)
    for _ in $(seq 1 100); do
        [ -e "$WORK/$name.look-ms" ] && [ -e "$WORK/$name.free-ms" ] && [ -e "$WORK/$name.add-ms" ] && [ -e "$WORK/$name.idle-ms" ] && break
        sleep 0.1
    done
    during=""
    for k in "look:a lookup of the device" "free:a lookup of the free number $S_FREE" "add:an add under the fresh number $fresh" "idle:a delete of the idle device $idle"; do
        during+="${k#*:}: '$(head -n 1 "$WORK/$name.${k%%:*}" 2>/dev/null | cut -f3- | sed 's/ size=.*//' | cut -c1-40)' after $(cat "$WORK/$name.${k%%:*}-ms" 2>/dev/null || echo 'more than 90000') ms; "
    done
    info "$n"; after=$STATE
    obs "$name" "the server held one read for $hold s; the delete, sent 2 s into the hold, answered: ${v:-nothing}, after ${ms:-more than 90000} ms$killed; sent 1 s after the delete: $during the device read '$after' at the end; the held read: ${line:-no answer in 60 s more}, $(( $(now_ms) - t1 )) ms after it was sent"
    lim 20 "$CTL" delete "$fresh" > /dev/null 2>&1; lim 20 "$CTL" delete "$idle" > /dev/null 2>&1
    srv_end "$SRV_PID" "$SRV_OUT" 30 || kill -9 "$SRV_PID" 2>/dev/null
    holder_end
    wait_state "$n" nothing 10 || { echo "   $n is still there: $ANSWER"; lim 20 "$CTL" delete "$n" > /dev/null 2>&1; }
}
step "a delete of a live device whose server holds a request"
delete_under_a_held_request delete_while_the_server_holds_a_request "$OWN3" 20
step "the same, and the server is killed while the delete waits"
delete_under_a_held_request delete_while_the_server_holds_a_request_then_dies "$OWN4" 60 5

# ============================================ a commit the driver refuses
# M1.3ac (#71). The server commits one request with no buffer, which the
# driver refuses as malformed before it changes anything (-EINVAL in
# ublk_drv.c at v6.11, v6.17 and v7.0): the request stays the server's,
# unanswered. The server then ends at once, as the daemon is to on a
# refused commit, and leaves the device waiting. A take-over must then
# have the kernel issue the request again, and the request must do what
# it was for (pinned): the daemon's stop on a refused commit rests on
# that, and a write is where a request not issued again loses data. A
# delete instead shows what becomes of it without a take-over (observed,
# like the other deletes under a held request). One course of each for
# a read, a write and a flush.
#
# The test program (`--refuse-one-commit read:B`, `write:B` or `flush`)
# says under the step `refuse-commit`: `sent for a read at byte B on tag
# T` (a write, or `a flush`), then the driver's answer, `refused: ...`,
# `aborted` or `taken`; or `not sent for ...: REASON` when the library
# would not send it, after which the request is answered and nothing
# here can pass. A client that ends before the server has gone shows
# that the kernel took the commit (it ended the request with the
# commit's -EIO): the program can print `taken` only when the tag is
# handed its next request, which one client may never cause, so the
# step does not wait for that line.
#
# The client of each kind, through the block device: a reader of the
# 4 KiB block at RC_AT (O_DIRECT); a writer of 4 KiB of new data there
# (O_DIRECT); an fsync of the device with nothing to write, which the
# kernel sends on as a flush (the device has a volatile write cache,
# UBLK_ATTR_VOLATILE_CACHE).
#
# refused_commit NAME N SECONDS KIND [SERVE OPTIONS]: device N served
# with the options, the client of KIND (read, write or flush), and a
# wait of SECONDS for the server to end by itself (then it is killed).
# -> RC_VERDICT: PASS when the commit was refused and nothing else
# happened (the program said it sent the commit for that request, the
# driver refused it, the program ended by the refusal, the client still
# waits, the device waits within 5 s, the daemon's limit), FAIL when
# something else did, ERROR when the rig could not set it up; RC_WHY
# says what was seen. RC_IMG, RC_OLD (the digest of the image's block at
# RC_AT before), RC_NEW (that of the writer's data), RC_RD (the stem of
# the client's files) and RC_KIND are for the step that follows.
RC_AT=$HOLD_A
refused_commit() {
    local name=$1 n=$2 secs=$3 srv out sent said served ended rd state ms want; shift 3
    RC_KIND=$1; shift
    RC_IMG=$WORK/$name.img; RC_RD=$WORK/$name.rd; RC_VERDICT=ERROR
    truncate -s "$SIZE" "$RC_IMG"
    "$DD" if=/dev/urandom of="$RC_IMG" bs=4k count=1 seek=$((RC_AT / 4096)) conv=notrunc status=none
    RC_OLD=$(rc_block "$RC_IMG")
    "$DD" if=/dev/urandom of="$RC_RD.data" bs=4k count=1 status=none
    RC_NEW=$(sha256sum < "$RC_RD.data" | cut -d' ' -f1)
    local client=()
    case $RC_KIND in
        read) client=("$DD" if="/dev/ublkb$n" of="$RC_RD.bin" bs=4k count=1 skip=$((RC_AT / 4096)) iflag=direct status=none)
              want="sent for a read at byte $RC_AT on tag " ;;
        write) client=("$DD" if="$RC_RD.data" of="/dev/ublkb$n" bs=4k count=1 seek=$((RC_AT / 4096)) oflag=direct "conv=nocreat,notrunc" status=none)
               want="sent for a write at byte $RC_AT on tag " ;;
        flush) client=("$DD" if=/dev/null of="/dev/ublkb$n" "conv=fsync,nocreat,notrunc" status=none)
               want="sent for a flush on tag " ;;
        *) RC_WHY="no client of kind $RC_KIND"; return ;;
    esac
    say add "$n" --depth 8
    if [ "$ANSWER" != added ] && [[ $ANSWER != added\ * ]]; then RC_WHY="the add under $n answered: ${ANSWER:-nothing}"; return; fi
    if ! serve_bg "$name-1" serve "$n" --file "$RC_IMG" "$@"; then
        RC_WHY="the device $n was not served: ${SRV_LINE:-no answer}"; kill -9 "$SRV_PID" 2>/dev/null; return
    fi
    srv=$SRV_PID; out=$SRV_OUT
    ( t0=$(now_ms)
      lim 180 "${client[@]}" 2> "$RC_RD.err"
      r=$?; echo "exit $r after $(( $(now_ms) - t0 )) ms$([ -s "$RC_RD.err" ] && echo ": $(head -n 1 "$RC_RD.err" | cut -c1-120)")" > "$RC_RD.rc" ) &
    for _ in $(seq 1 $((secs * 10))); do kill -0 "$srv" 2>/dev/null || break; sleep 0.1; done
    if kill -0 "$srv" 2>/dev/null; then ended="NO: it was still serving after $secs s, and was killed"; srv_kill "$srv"
    else ended=yes; wait "$srv" 2>/dev/null; fi
    cat "$out" >> "$OUT/steps.log"
    sent=$(awk -F'\t' '$1 == "refuse-commit" { print $3; exit }' "$out")
    said=$(awk -F'\t' '$1 == "refuse-commit" { if (++k == 2) { print $3; exit } }' "$out")
    served=$(awk -F'\t' '$1 == "served" { print $3 }' "$out" | tail -n 1)
    wait_state "$n" waiting 5; state=$STATE; ms=$WAITED_MS
    if [ -e "$RC_RD.rc" ]; then rd="ENDED: $(cat "$RC_RD.rc")"; else rd=waiting; fi
    RC_WHY="the program said '${sent:-nothing}', then '${said:-nothing}'; it ended by itself: $ended; its last line: '$(printf '%s' "$served" | cut -c1-160)'; the $RC_KIND client: $rd; the device read $state $ms ms after"
    if [[ $sent == "$want"* ]] && [[ $said == refused:* ]] && [ "$ended" = yes ] \
        && [[ $served == "ended by the refused commit"* ]] && [ "$rd" = waiting ] && [ "$state" = waiting ]; then
        RC_VERDICT=PASS
    else
        RC_VERDICT=FAIL
    fi
}
# rc_block PATH: the digest of the 4 KiB at RC_AT, read with O_DIRECT from a device
rc_block() {
    local direct=()
    [ -b "$1" ] && direct=(iflag=direct)
    lim 30 "$DD" if="$1" bs=4k count=1 skip=$((RC_AT / 4096)) "${direct[@]}" status=none 2>/dev/null | sha256sum | cut -d' ' -f1
}
# rc_said DIGEST: what a block holds, against the image's old block and the writer's data
rc_said() {
    case $1 in "$RC_OLD") echo "the old block" ;; "$RC_NEW") echo "the written data" ;; *) echo "NEITHER the old block nor the written data" ;; esac
}
# rc_client SECONDS: wait for refused_commit's client -> RC_CLIENT, what
# it got; 0 when it ended with 0 (and a reader read the image's block)
rc_client() {
    local got
    for _ in $(seq 1 $(($1 * 10))); do [ -e "$RC_RD.rc" ] && break; sleep 0.1; done
    if [ ! -e "$RC_RD.rc" ]; then RC_CLIENT="still waiting after $1 s more"; return 1; fi
    RC_CLIENT=$(cat "$RC_RD.rc")
    if [ "$RC_KIND" = read ]; then
        got=$(sha256sum < "$RC_RD.bin" 2>/dev/null | cut -d' ' -f1)
        RC_CLIENT+="; its data is $(rc_said "$got")"
        [[ $(cat "$RC_RD.rc") == "exit 0 "* ]] && [ "$got" = "$RC_OLD" ]
    else
        [[ $(cat "$RC_RD.rc") == "exit 0 "* ]]
    fi
}

# refused_course KIND N_TAKE N_DELETE: the take-over course and the delete course of one kind
refused_course() {
    local kind=$1 nt=$2 nd=$3 what pin r dev img v ms t0 before
    what=$kind; [ "$kind" = flush ] || what="$kind:$RC_AT"
    case $kind in                      # the pin after the take-over
        read) pin=refused_read_issued_again ;;
        write) pin=refused_write_lands_after_take_over ;;
        *) pin=refused_flush_completes_after_take_over ;;
    esac

    step "a $kind committed with no buffer, then a take-over"
    refused_commit "refused_${kind}_take_over" "$nt" 30 "$kind" --refuse-one-commit "$what"
    pin "refused_${kind}_ends_the_server" "$RC_VERDICT" "$RC_WHY"
    if [ "$RC_VERDICT" = ERROR ]; then
        pin "$pin" ERROR "$RC_WHY"
    elif serve_bg "refused_${kind}_take_over-2" take-over "$nt" --file "$RC_IMG"; then
        if rc_client 60; then r=PASS; else r=FAIL; fi
        # A write: the range read back through the device while the
        # take-over serves, then from the image once it has ended.
        [ "$kind" != write ] || dev=$(rc_block "/dev/ublkb$nt")
        kill -TERM "$SRV_PID"; srv_end "$SRV_PID" "$SRV_OUT" 30 || kill -9 "$SRV_PID" 2>/dev/null
        if [ "$kind" = write ]; then
            img=$(rc_block "$RC_IMG")
            [ "$dev" = "$RC_NEW" ] && [ "$img" = "$RC_NEW" ] || r=FAIL
            RC_CLIENT+="; the range read back through the device: $(rc_said "$dev"); from the image after the take-over ended: $(rc_said "$img")"
        fi
        pin "$pin" "$r" "after the take-over ($SRV_LINE) the $kind client held by the refused commit: $RC_CLIENT; the take-over's last line: $(printf '%s' "$SRV_SERVED" | cut -c1-160)"
    else
        pin "$pin" FAIL "the take-over of the device left by the refused commit did not go live: ${SRV_LINE:-no answer}"
        kill -9 "$SRV_PID" 2>/dev/null
    fi
    lim 20 "$CTL" delete "$nt" > /dev/null 2>&1
    rc_client 30

    step "a $kind committed with no buffer, then a delete"
    refused_commit "refused_${kind}_delete" "$nd" 30 "$kind" --refuse-one-commit "$what"
    if [ "$RC_VERDICT" = ERROR ]; then
        obs "refused_${kind}_then_delete" "not run: $RC_WHY"
    else
        t0=$(now_ms); say delete "$nd"; v=$ANSWER; ms=$(( $(now_ms) - t0 ))
        rc_client 60
        [ "$kind" != write ] || RC_CLIENT+="; the image's block afterwards: $(rc_said "$(rc_block "$RC_IMG")")"
        info "$nd"; before=$STATE
        wait_state "$nd" nothing 10
        obs "refused_${kind}_then_delete" "the delete answered: ${v:-nothing}, after $ms ms by the rig's clock; the $kind client held by the refused commit: $RC_CLIENT; the device then read $before, and $STATE after $WAITED_MS ms more. The refusal itself ($RC_VERDICT): $RC_WHY"
    fi
    lim 20 "$CTL" delete "$nd" > /dev/null 2>&1
    rc_client 30
}
refused_course read "$RC_READ" "$RC_READ_DEL"
refused_course write "$RC_WRITE" "$RC_WRITE_DEL"
refused_course flush "$RC_FLUSH" "$RC_FLUSH_DEL"

# The control: the same step on a server that refuses nothing. The
# reader is answered at once and the server goes on: the step must say
# FAIL, or a PASS above shows nothing.
step "control: the refused-commit step on a server that refuses nothing"
refused_commit refused_commit_control "$RC_CTL" 5 read
ctl refused_commit_without_a_refusal "$RC_VERDICT" "$RC_WHY"
lim 20 "$CTL" delete "$RC_CTL" > /dev/null 2>&1
rc_client 30

step "own: a delete of a waiting device nobody holds"
truncate -s "$SIZE" "$WORK/own2.img"
say add "$OWN2" --depth 8
if serve_bg own2-1 serve "$OWN2" --file "$WORK/own2.img"; then
    kill -TERM "$SRV_PID"; srv_end "$SRV_PID" "$SRV_OUT" 30 || kill -9 "$SRV_PID" 2>/dev/null
    # A device still reads live for a moment after its server's process
    # has ended (up to 136 ms seen); the step is about a waiting one.
    # In run 43074927907 one image looked too soon and judged a delete
    # of a live device.
    wait_state "$OWN2" waiting 10; before=$STATE
    t0=$(now_ms); say delete "$OWN2"; v=$ANSWER; ms=$(( $(now_ms) - t0 ))
    free=no
    if wait_state "$OWN2" nothing 10; then
        got="nothing there to a lookup after $WAITED_MS ms"; gone=$STATE
        if free_for_add "$OWN2" 5; then free=yes; fi
        got+="; then $FREE_SAID"
    else got="still: $ANSWER"; gone=$STATE; fi
    obs delete_ms "$DELETE_HELD; unheld and waiting: ${v:-no answer} ($ms ms by the rig's clock)"
    { if [ "$before" = waiting ] && [[ "$v" == returned\ after\ * ]] && [ "$gone" = nothing ] && [ "$free" = yes ] && [ ! -e "/dev/ublkb$OWN2" ]; then r=PASS; else r=FAIL; fi; pin delete_unheld_waiting_frees_number "$r" "the device read $before; the binding's delete answered: ${v:-nothing}; then: $got; /dev/ublkb$OWN2 $([ -e "/dev/ublkb$OWN2" ] && echo 'IS STILL THERE' || echo 'is gone')"; }
else
    obs delete_ms "$DELETE_HELD; unheld: not run, the device was not served"
    pin delete_unheld_waiting_frees_number ERROR "the device $OWN2 was not served: ${SRV_LINE:-no answer}"
fi

# ============================================================ strangers
step "stranger: live, served by the reference server"
REF_PID_OK="not run"
if ref_add "$S_LIVE"; then
    stranger live "$S_LIVE" live
    info "$S_LIVE"
    if [ -n "$(ref_pid "$S_LIVE")" ] && [ "$(kv "$ANSWER" server_pid)" = "$(ref_pid "$S_LIVE")" ]; then
        REF_PID_OK="yes: server_pid $(kv "$ANSWER" server_pid) is the reference server's"
    else
        REF_PID_OK="NO: server_pid $(kv "$ANSWER" server_pid), the reference tool names $(ref_pid "$S_LIVE")"
    fi
else
    strangers_error live "the reference server's device $S_LIVE did not go live: $(tail -n 1 "$OUT/stranger-$S_LIVE-add.log")"
fi
case "$OWN_PID_OK/$REF_PID_OK" in
    *"not run"*) pin info_server_pid_is_the_servers ERROR "the binding's own server: $OWN_PID_OK; the reference server: $REF_PID_OK" ;;
    yes*/yes*) pin info_server_pid_is_the_servers PASS "the binding's own server: $OWN_PID_OK; the reference server: $REF_PID_OK" ;;
    *) pin info_server_pid_is_the_servers FAIL "the binding's own server: $OWN_PID_OK; the reference server: $REF_PID_OK" ;;
esac
lim 60 ublk del -n "$S_LIVE" > /dev/null 2>&1

step "stranger: waiting, its reference server killed"
if ref_add "$S_WAITING"; then
    p=$(ref_pid "$S_WAITING"); kill -9 "$p" 2>/dev/null
    for _ in $(seq 1 100); do [ "$(ref_state "$S_WAITING")" = QUIESCED ] && break; sleep 0.1; done
    if [ "$(ref_state "$S_WAITING")" = QUIESCED ]; then
        stranger waiting "$S_WAITING" waiting
    else
        strangers_error waiting "after its server (pid ${p:-unknown}) was killed the device read $(ref_state "$S_WAITING"), not QUIESCED"
    fi
else
    strangers_error waiting "the reference server's device $S_WAITING did not go live: $(tail -n 1 "$OUT/stranger-$S_WAITING-add.log")"
fi
lim 60 ublk del -n "$S_WAITING" > /dev/null 2>&1

# The reference tool has no add without a start. The stranger here is
# a device another process added and left, with another depth than the
# add under test asks for: to the driver, and to a daemon that finds
# it, that is a device of somebody else's that was never started.
step "stranger: added by another process and never started"
say add "$S_NEVER" --depth 16
if [ "$ANSWER" = added ] || [[ $ANSWER == added\ * ]]; then
    stranger never_started "$S_NEVER" dead
else
    strangers_error never_started "another process's add under $S_NEVER answered: ${ANSWER:-nothing}"
fi
say delete "$S_NEVER"

step "stranger: deleted while a process held it open"
if ref_add "$S_DELETED" && holder_start hold-stranger "/dev/ublkb$S_DELETED" plain; then
    lim 60 ublk del --async -n "$S_DELETED" > "$OUT/stranger-$S_DELETED-del.log" 2>&1
    for _ in $(seq 1 100); do [ ! -e "/dev/ublkb$S_DELETED" ] && break; sleep 0.1; done
    if [ ! -e "/dev/ublkb$S_DELETED" ]; then
        stranger deleted_held "$S_DELETED" dead
    else
        strangers_error deleted_held "10 s after the reference tool's asynchronous delete /dev/ublkb$S_DELETED was still there"
    fi
    holder_end
else
    strangers_error deleted_held "the reference server's device $S_DELETED did not go live, or the holder could not open it"
    [ -z "${H_PID:-}" ] || holder_end
fi

# ============================================================== controls
step "control: the check for \"taken\", on a free number"
say add "$S_FREE" --depth 8
if [ "$ANSWER" = taken ]; then ctl add_on_free_number_is_taken PASS "an add under the free number $S_FREE answered taken"
elif [ -z "$ANSWER" ]; then ctl add_on_free_number_is_taken ERROR "no answer"
else ctl add_on_free_number_is_taken FAIL "an add under the free number $S_FREE answered: $ANSWER"; fi
say delete "$S_FREE"

step "control: the check for \"untouched\", with the stranger deleted between the looks"
if ref_add "$S_CTL"; then
    look "$S_CTL"; before=$LOOK
    lim 60 ublk del -n "$S_CTL" > /dev/null 2>&1
    look "$S_CTL"; after=$LOOK
    if [ "$before" = "$after" ]; then ctl stranger_deleted_between_looks PASS "the two looks are the same although the stranger was deleted: $after"
    else ctl stranger_deleted_between_looks FAIL "before: $before; after: $after"; fi
else
    ctl stranger_deleted_between_looks ERROR "the reference server's device $S_CTL did not go live"
fi

# ============================================== the library's directory
step "nothing of the binding under /run/ublksrvd"
SEEN=$OUT/ublksrvd-seen.log; : > "$SEEN"
for n in $OWN $OWN2 $OWN3 $OWN4 $RC_READ $RC_READ_DEL $RC_WRITE $RC_WRITE_DEL $RC_FLUSH $RC_FLUSH_DEL $RC_CTL $S_NEVER $S_FREE; do ublksrvd_look "$n" "$T0" "$SEEN"; done
n=$(grep -c '^entry ' "$SEEN")
pin_is no_file_under_run_ublksrvd "$n entries under /run/ublksrvd for the numbers the binding added, served, took over and deleted ($OWN $OWN2 $OWN3 $OWN4 $RC_READ $RC_READ_DEL $RC_WRITE $RC_WRITE_DEL $RC_FLUSH $RC_FLUSH_DEL $RC_CTL $S_NEVER $S_FREE), newer than the start of the rig: $(grep '^entry ' "$SEEN" | cut -d' ' -f2- | tr '\n' ' ')" [ "$n" = 0 ]
rec ublksrv_run_dir "$(find /run/ublksrvd -mindepth 1 -maxdepth 1 2>/dev/null | sort | head -n 20 | tr '\n' ' ')"

step "done"
cat "$R"
exit 0

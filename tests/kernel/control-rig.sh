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
OWN=40; OWN2=41; OWN3=42
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
    for n in $OWN $OWN2 $OWN3 $S_NEVER $S_FREE; do [ -z "$CTL" ] || lim 20 "$CTL" delete "$n" > /dev/null 2>&1; done
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
    rec ublk_server "BUILD FAILED in three tries: $(grep -m 1 -E 'Temporary failure|Could not resolve|E: ' "$OUT/ublksrv-build.log" || tail -n 1 "$OUT/ublksrv-build.log")"; SETUP_OK=""
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
lim 900 "$KTEST" --ignored --test-threads 1 --nocapture > "$OUT/kernel-tests.log" 2>&1
rec kernel_tests_exit "$?"
grep -a '^control_' "$OUT/kernel-tests.log" >> "$R"
grep -a '^control_' "$OUT/kernel-tests.log" | sed 's/^/== /'
tail -n 5 "$OUT/kernel-tests.log"

# ====================================================== its own device
own_device() {
    local a before after t0 ms srv srv_out v rd rd_rc want got held_line later_line k
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
        obs killed_server_live_ms "not run: no device"; obs flags_beyond_the_mask "not run: no device"
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
        obs killed_server_live_ms "not run: no server"
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
    kill -TERM "$srv"
    if srv_end "$srv" "$srv_out" 30; then
        info "$OWN"
        { if [ "$STATE" = waiting ] && [ -b "/dev/ublkb$OWN" ]; then r=PASS; else r=FAIL; fi; pin orderly_stop_leaves_waiting "$r" "after SIGTERM the server said: ${SRV_SERVED:-nothing}; info: $ANSWER; /dev/ublkb$OWN $([ -b "/dev/ublkb$OWN" ] && echo 'is there' || echo 'IS GONE')"; }
    else
        pin orderly_stop_leaves_waiting ERROR "the server did not end within 30 s of SIGTERM"
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
    kill -9 "$srv"; wait "$srv" 2>/dev/null
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
    kill -9 "$srv"; wait "$srv" 2>/dev/null
    wait_state "$OWN" waiting 10
    if serve_bg own-4 take-over "$OWN" --file "$WORK/own.img" --hold-one-request "120:read:$HOLD_B"; then
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

    step "own: a delete while a process holds the device, with a request in flight"
    if ! holder_start hold-del "/dev/ublkb$OWN" plain; then
        for v in delete_returns_while_held deleted_held_is_dead_and_keeps_number delete_fails_requests \
            number_free_when_holder_lets_go dead_after_delete_is_not_taken_over; do
            pin "$v" ERROR "the holder could not open the device: $(head -n 1 "$H_OUT")"
        done
        holder_end; kill -TERM "$srv" 2>/dev/null
        return
    fi
    holder_cmd "read $HOLD_B"          # held by the server: in flight at the delete
    sleep 2
    t0=$(now_ms)
    say delete "$OWN"; v=$ANSWER
    ms=$(( $(now_ms) - t0 ))
    DELETE_HELD="held: ${v:-no answer} ($ms ms by the rig's clock)"
    { if [[ "$v" == returned\ after\ * ]] && [ "$ms" -lt 2000 ]; then r=PASS; else r=FAIL; fi; pin delete_returns_while_held "$r" "the binding's delete of the held $OWN answered: ${v:-nothing}, after $ms ms by the rig's clock"; }
    held_line=$(holder_answer 2 30)
    holder_cmd "read 0"
    later_line=$(holder_answer 3 30)
    { if [[ "$held_line" == read\ *\ failed:* ]] && [[ "$later_line" == read\ *\ failed:* ]]; then r=PASS; else r=FAIL; fi; pin delete_fails_requests "$r" "the read in flight at the delete: ${held_line:-no answer in 30 s}; a read after it: ${later_line:-no answer in 30 s}"; }
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
        pin number_free_when_holder_lets_go PASS "$WAITED_MS ms after the holder closed the device, a lookup of $OWN says nothing is there"
    else
        pin number_free_when_holder_lets_go FAIL "$WAITED_MS ms after the holder closed the device, info of $OWN: $ANSWER"
    fi
}
DELETE_HELD="held: not run"; OWN_PID_OK="not run"
own_device

step "own: a delete of a waiting device nobody holds"
truncate -s "$SIZE" "$WORK/own2.img"
say add "$OWN2" --depth 8
if serve_bg own2-1 serve "$OWN2" --file "$WORK/own2.img"; then
    kill -TERM "$SRV_PID"; srv_end "$SRV_PID" "$SRV_OUT" 30 || kill -9 "$SRV_PID" 2>/dev/null
    info "$OWN2"; before=$STATE
    t0=$(now_ms); say delete "$OWN2"; v=$ANSWER; ms=$(( $(now_ms) - t0 ))
    if wait_state "$OWN2" nothing 10; then got="nothing there after $WAITED_MS ms"; else got="still: $ANSWER"; fi
    obs delete_ms "$DELETE_HELD; unheld and waiting: ${v:-no answer} ($ms ms by the rig's clock)"
    { if [ "$before" = waiting ] && [[ "$v" == returned\ after\ * ]] && [ "$STATE" = nothing ] && [ ! -e "/dev/ublkb$OWN2" ]; then r=PASS; else r=FAIL; fi; pin delete_unheld_waiting_frees_number "$r" "the device read $before; the binding's delete answered: ${v:-nothing}; then: $got; /dev/ublkb$OWN2 $([ -e "/dev/ublkb$OWN2" ] && echo 'IS STILL THERE' || echo 'is gone')"; }
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
for n in $OWN $OWN2 $S_NEVER $S_FREE; do ublksrvd_look "$n" "$T0" "$SEEN"; done
n=$(grep -c '^entry ' "$SEEN")
pin_is no_file_under_run_ublksrvd "$n entries under /run/ublksrvd for the numbers the binding added, served, took over and deleted ($OWN $OWN2 $S_NEVER $S_FREE), newer than the start of the rig: $(grep '^entry ' "$SEEN" | cut -d' ' -f2- | tr '\n' ' ')" [ "$n" = 0 ]
rec ublksrv_run_dir "$(find /run/ublksrvd -mindepth 1 -maxdepth 1 2>/dev/null | sort | head -n 20 | tr '\n' ' ')"

step "done"
cat "$R"
exit 0

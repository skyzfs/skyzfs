#!/bin/bash
# Tests of the control rig's refused-fetch watch (M1.T36): the rig's own
# refused_fetch and rf_delete, run against a made-up test program (its
# `--refuse-one-fetch` lines as #84 specifies them) and a made-up reader
# of the device. No kernel and no ublk are needed.
# Run: bash tests/kernel/test_control_fetch.sh
HERE=$(cd "$(dirname "$0")" && pwd)
RIG=$HERE/control-rig.sh
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
if ! T=$(mktemp -d) || [ -z "$T" ] || [ ! -d "$T" ]; then echo "no temporary directory"; exit 2; fi
# The made-up programs wait for this file: none outlives the test.
trap 'touch "$T/stop"; sleep 0.5; rm -rf "$T"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}
has() { # name got part
    case $2 in *"$3"*) echo "ok   $1" ;; *) echo "FAIL $1: '$3' not in '$2'"; fails=$((fails + 1)) ;; esac
}
hasnt() { # name got part
    case $2 in *"$3"*) echo "FAIL $1: '$3' in '$2'"; fails=$((fails + 1)) ;; *) echo "ok   $1" ;; esac
}

# The rig's functions, as the rig has them. A one-line function is cut
# at its own line.
for f in say info wait_state free_for_add nodes_of rf_blk rf_line rf_words rf_pin refused_fetch rf_delete; do
    eval "$(sed -n "/^$f() .*}\$/{p;q}; /^$f() {/,/^}\$/p" "$RIG")"
    declare -F "$f" > /dev/null || { echo "FAIL the rig has no function $f"; exit 2; }
done
eval "$(sed -n '/^rec() /,/^}$/p; /^kv() /p; /^now_ms() /p; /^pin() /p; /^obs() /p; /^ctl() /p' "$RIG")"

OUT=$T/out; WORK=$T/work; mkdir -p "$OUT" "$WORK"
# shellcheck disable=SC2034  # read by the rig's rec
R=$OUT/results.tsv; : > "$R"
export T
# The test program. SCENARIO says what the driver does with the fetch
# and the start. The device's state is a file per number.
CTL=$T/ctl
cat > "$CTL" <<'CTLEOF'
#!/bin/bash
step=$1; n=$2; shift 2
state() { echo "$1" > "$T/state.$n"; }
what=""
while [ $# -gt 0 ]; do
    case $1 in --file) shift ;; --refuse-one-fetch) what=$2; shift ;; esac
    shift
done
case $step in
    add) if [ -e "$T/state.$n" ] && [ "$(cat "$T/state.$n")" != nothing ]; then printf 'add\t%s\ttaken\n' "$n"; else state dead; printf 'add\t%s\tadded\n' "$n"; fi; exit 0 ;;
    info) printf 'info\t%s\tstate=%s server_pid=1\n' "$n" "$(cat "$T/state.$n" 2>/dev/null || echo nothing)"; exit 0 ;;
    delete) state nothing; printf 'delete\t%s\treturned after 5 ms\n' "$n"; exit 0 ;;
esac
# serve or take-over
c=start; [ "$what" = recovery ] && c=end-recovery
# As the real program: `aborted` comes at the stop, `taken` when a
# request reaches tag 0 (here: the reader's read).
trap '[ "$SCENARIO" != aborted ] || printf "refuse-fetch\t%s\taborted\n" "$n"; printf "served\t%s\tended by a signal; handed 0 reads 0 writes 0 flushes 0 discards 0 others 0 failed 0 held 0 first_write_copies 0 commands_aborted 0 commands_failed 1\n" "$n"; [ -e "$T/started.$n" ] && state waiting; exit 0' TERM
[ "$SCENARIO" = ignores_term ] && trap '' TERM
if [ -z "$what" ]; then
    state live; : > "$T/started.$n"; printf '%s\t%s\tlive after 3 ms; handed before the start: 0\n' "$step" "$n"
else
    if [ "$SCENARIO" = unsent ]; then printf 'refuse-fetch\t%s\tsent on tag 3\n' "$n"; else printf 'refuse-fetch\t%s\tsent on tag 0\n' "$n"; fi
    case $SCENARIO in
        aborted|taken) ;;
        unsent) printf 'refuse-fetch\t%s\trefused: errno 22 (Invalid argument)\n' "$n" ;;
        *) printf 'refuse-fetch\t%s\trefused: errno 22 (Invalid argument)\n' "$n" ;;
    esac
    case $SCENARIO in
        start_ok|aborted|taken|ignores_term) sleep 0.3; state live; : > "$T/started.$n"; printf '%s\t%s\treturned after 3 ms: ok\n' "$c" "$n" ;;
        start_errno) sleep 0.3; printf '%s\t%s\treturned after 2 ms: errno 22 (Invalid argument)\n' "$c" "$n" ;;
        start_hangs) sleep 1; printf '%s\t%s\tno answer after 10000 ms\n' "$c" "$n" ;;
        panics) sleep 0.3; printf 'thread panicked\nskyzfs-ublk-control: start: the command'"'"'s thread ended with no answer\n' >&2; exit 101 ;;
        start_late) sleep 1; printf '%s\t%s\tno answer after 10000 ms\n' "$c" "$n"; sleep 1.5; state live; : > "$T/started.$n"; printf '%s\t%s\treturned after 12500 ms: ok\n' "$c" "$n" ;;
    esac
fi
said=""
while [ ! -e "$T/stop" ]; do
    if [ "$SCENARIO" = taken ] && [ -z "$said" ] && [ -e "$T/read.$n" ]; then printf 'refuse-fetch\t%s\ttaken\n' "$n"; said=1; fi
    sleep 0.1
done
CTLEOF
chmod +x "$CTL"
# The reader of the device: it reads when the device is live, waits
# (until stopped) when it is not.
DD=$T/dd
cat > "$DD" <<'DDEOF'
#!/bin/bash
dev=""
for a in "$@"; do case $a in if=/dev/ublkb*) dev=${a#if=/dev/ublkb} ;; esac; done
[ -n "$dev" ] || exec dd "$@"
[ "$(cat "$T/state.$dev" 2>/dev/null)" = live ] && { : > "$T/read.$dev"; exit 0; }
while [ ! -e "$T/stop" ]; do sleep 0.1; done
DDEOF
chmod +x "$DD"
# The device node: there once the start has returned ok.
# shellcheck disable=SC2329  # called by the rig's refused_fetch
rf_blk() { [ -e "$T/started.${1#/dev/ublkb}" ]; }
SIZE=$((64 << 20))
export SIZE

watch() { # scenario n step [options] -> RF_VERDICT, RF_SAID
    SCENARIO=$1; export SCENARIO
    local n=$2; shift 2
    rm -f "$T/state.$n" "$T/started.$n"
    say add "$n" --depth 8 > /dev/null
    refused_fetch "rf-$SCENARIO-$n" "$n" "$@" > /dev/null 2>&1
    rf_delete "$n" > /dev/null 2>&1
}

watch start_hangs 70 serve --refuse-one-fetch start
is "a refused fetch, a start that does not return: the watch reads the refusal" "$RF_VERDICT" PASS
has "  says what the driver answered" "$RF_SAID" "the program said: 'sent on tag 0', then 'refused: errno 22 (Invalid argument)'"
has "  and that the start did not return" "$RF_SAID" "its start: 'start: no answer after 10000 ms'"
has "  nor 10 s later" "$RF_SAID" "10 s later: STILL NO ANSWER"
has "  the device's state" "$RF_SAID" "the device then read dead"
has "  no read is tried with no node" "$RF_SAID" "a read of its first block through /dev/ublkb70: not tried"
has "  how the program ended" "$RF_SAID" "the program ended by SIGTERM, its last line: 'ended by a signal"
has "  the delete" "$RF_SAID" "a delete answered: returned after 5 ms"
has "  and the number" "$RF_SAID" "the number was free: an add under 70 succeeded"

watch start_late 71 serve --refuse-one-fetch start
has "a start that returns late is seen" "$RF_SAID" "10 s later: returned after 12500 ms: ok"

watch start_ok 72 serve --refuse-one-fetch start
is "a refused fetch, a start that returned" "$RF_VERDICT" PASS
has "  says so" "$RF_SAID" "its start: 'start: returned after 3 ms: ok'"
has "  reads the device" "$RF_SAID" "through /dev/ublkb72: exit 0 after"
hasnt "  and has no later line" "$RF_SAID" "10 s later"

watch start_errno 73 serve --refuse-one-fetch start
has "a start that failed" "$RF_SAID" "its start: 'start: returned after 2 ms: errno 22 (Invalid argument)'"

watch start_ok 74 take-over --refuse-one-fetch recovery
has "at a take-over: the end of the recovery" "$RF_SAID" "its start: 'end-recovery: returned after 3 ms: ok'"
is "  read as a refusal" "$RF_VERDICT" PASS

watch aborted 75 serve --refuse-one-fetch start
is "an aborted fetch is not a refusal" "$RF_VERDICT" FAIL
has "  the word at the stop is recorded" "$RF_SAID" "by the program's end it said of the fetch: 'aborted'"
watch taken 76 serve --refuse-one-fetch start
is "a taken fetch is not a refusal" "$RF_VERDICT" FAIL
has "  the word at the probe's read is recorded" "$RF_SAID" "by the program's end it said of the fetch: 'taken'"
has "  and the whole last line" "$RF_SAID" "commands_aborted 0 commands_failed 1'"

watch start_ok 77 serve
is "the control: a plain serve" "$RF_VERDICT" FAIL
has "  says nothing of a fetch, and goes live" "$RF_SAID" "the program said: 'nothing', then 'nothing'; its start: 'serve: live after 3 ms"

# A device that is not live while its node is there: the reader waits,
# and the watch gives up on it at 15 s.
# shellcheck disable=SC2329  # called by the rig's refused_fetch
rf_blk() { :; }
watch start_hangs 78 serve --refuse-one-fetch start
has "a read that does not return is given up" "$RF_SAID" "NO ANSWER in 15 s (the reader is left behind)"
rf_blk() { [ -e "$T/started.${1#/dev/ublkb}" ]; }

watch panics 80 serve --refuse-one-fetch start
is "a program that ends by itself after the refusal" "$RF_VERDICT" PASS
has "  is not said to have ended by a signal" "$RF_SAID" "the program ended by itself, with exit 101, before the rig signalled it"
has "  and its last words are kept" "$RF_SAID" "start: the command's thread ended with no answer"
hasnt "  no signal is named" "$RF_SAID" "ended by SIGTERM"

watch ignores_term 79 serve --refuse-one-fetch start
has "a program that SIGTERM does not end is killed" "$RF_SAID" "the program ended by kill -9 (SIGTERM did not end it in 10 s)"

# pin_refused_fetch_sent_and_refused, from the two steps' words.
pin_of() { : > "$R"; rf_pin "$1" "$2" > /dev/null; awk -F'\t' '$1 == "pin_refused_fetch_sent_and_refused" { print $2 }' "$R"; }
watch start_hangs 81 serve --refuse-one-fetch start; a="$RF_VERDICT: $(rf_words)"
watch start_ok 82 take-over --refuse-one-fetch recovery; b="$RF_VERDICT: $(rf_words)"
got=$(pin_of "$a" "$b")
is "the pin: both refused" "${got%%:*}" PASS
has "  with the words" "$got" "at the start: 'sent on tag 0', then 'refused: errno 22 (Invalid argument)'"
watch start_ok 83 serve; c="$RF_VERDICT: $(rf_words)"
got=$(pin_of "$a" "$c")
is "the pin: a program that sent no bad fetch at the take-over" "${got%%:*}" FAIL
has "  says what it said" "$got" "at the take-over: '', then ''"
watch unsent 84 serve --refuse-one-fetch start; d="$RF_VERDICT: $(rf_words)"
is "the pin: a refusal of a fetch sent on another tag is not one" "$(pin_of "$d" "$b" | cut -d: -f1)" FAIL
watch taken 85 serve --refuse-one-fetch start; e="$RF_VERDICT: $(rf_words)"
is "the pin: a driver that took the fetch" "$(pin_of "$e" "$b" | cut -d: -f1)" FAIL
is "the pin: a step that did not run is the rig's error" "$(pin_of "$a" "not run" | cut -d: -f1)" ERROR

if [ "$fails" = 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi

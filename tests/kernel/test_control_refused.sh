#!/bin/bash
# Tests of the control rig's refused-commit step (M1.T33): the rig's own
# refused_commit and rc_reader, run against a made-up test program and
# a made-up reader of the device, one course for each answer the driver
# or the library can give. No kernel and no ublk are needed.
# Run: bash tests/kernel/test_control_refused.sh
HERE=$(cd "$(dirname "$0")" && pwd)
RIG=$HERE/control-rig.sh
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
if ! T=$(mktemp -d) || [ -z "$T" ] || [ ! -d "$T" ]; then echo "no temporary directory"; exit 2; fi
# The made-up readers wait for this file: none outlives the test.
trap 'touch "$T/stop"; sleep 0.3; rm -rf "$T"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}
has() { # name got part
    case $2 in *"$3"*) echo "ok   $1" ;; *) echo "FAIL $1: '$3' not in '$2'"; fails=$((fails + 1)) ;; esac
}

# The rig's functions, as the rig has them.
for f in say info wait_state serve_bg srv_kill refused_commit rc_reader; do
    eval "$(sed -n "/^$f() {\$/,/^}\$/p" "$RIG")"
    declare -F "$f" > /dev/null || { echo "FAIL the rig has no function $f"; exit 2; }
done
eval "$(sed -n '/^kv() /p; /^now_ms() /p' "$RIG")"
HOLD_A=$((32 << 20))
eval "$(sed -n '/^RC_AT=/p' "$RIG")"
is "the block the step reads" "$RC_AT" "$HOLD_A"

OUT=$T/out; WORK=$T/work; mkdir -p "$OUT" "$WORK"
# shellcheck disable=SC2034  # read by refused_commit, as the rig's own global
SIZE=$((64 << 20))
export T RC_AT
# The test program. SCENARIO says what the driver answers to the commit
# with no buffer; the device's state is a file that `serve` writes.
CTL=$T/ctl
cat > "$CTL" <<'CTLEOF'
#!/bin/bash
step=$1; n=$2
state() { echo "$1" > "$T/state.$n"; }
case $step in
    add) if [ "$SCENARIO" = add_taken ]; then printf 'add\t%s\ttaken\n' "$n"; else printf 'add\t%s\tadded\n' "$n"; fi ;;
    info) printf 'info\t%s\tstate=%s server_pid=1\n' "$n" "$(cat "$T/state.$n" 2>/dev/null || echo nothing)" ;;
    serve)
        state live
        printf 'serve\t%s\tlive after 3 ms\n' "$n"
        sleep 0.5
        sent="refuse-commit\t$n\tsent for a read at byte $RC_AT on tag 3\n"
        case $SCENARIO in
            refused|refused_reader_ended|refused_stays_live|refused_hangs|refused_other_read)
                # another read refused: one the kernel or udev sent at a start
                [ "$SCENARIO" = refused_other_read ] && sent="refuse-commit\t$n\tsent for a read at byte 0 on tag 1\n"
                printf "$sent"
                printf 'refuse-commit\t%s\trefused: errno 22 (Invalid argument)\n' "$n"
                printf 'served\t%s\tended by the refused commit; handed 1 reads 0 writes 0 flushes\n' "$n"
                [ "$SCENARIO" = refused_stays_live ] || state waiting
                [ "$SCENARIO" = refused_hangs ] || exit 0 ;;
            aborted) printf "$sent"; printf 'refuse-commit\t%s\taborted\n' "$n" ;;
            taken) printf "$sent" ;;
            not_sent) printf 'refuse-commit\t%s\tnot sent for a read at byte %s on tag 3: refused by the library\n' "$n" "$RC_AT" ;;
        esac
        # It serves on, until it is killed.
        while :; do sleep 0.1; done ;;
    *) printf '%s\t%s\tunknown\n' "$step" "$n" ;;
esac
CTLEOF
chmod +x "$CTL"
# The reader of the device, and dd for everything else.
DD=$T/dd
cat > "$DD" <<'DDEOF'
#!/bin/bash
case " $* " in
    *" if=/dev/ublkb"*)
        case $SCENARIO in
            refused|refused_stays_live|refused_hangs|refused_other_read|aborted) while [ ! -e "$T/stop" ]; do sleep 0.1; done; exit 1 ;;
            taken|refused_reader_ended) echo "dd: error reading '/dev/ublkb44': Input/output error" >&2; exit 1 ;;
            *) for a in "$@"; do case $a in of=*) head -c 4096 /dev/zero > "${a#of=}" ;; esac; done; exit 0 ;;
        esac ;;
esac
exec dd "$@"
DDEOF
chmod +x "$DD"

course() { # scenario n seconds [serve options]
    SCENARIO=$1; export SCENARIO
    local n=$2 secs=$3; shift 3
    refused_commit "rc-$SCENARIO" "$n" "$secs" "$@" > /dev/null 2>&1
}

course refused 44 30 --refuse-one-commit "read:$RC_AT"
is "a refused commit" "$RC_VERDICT" PASS
has "  says what the driver answered" "$RC_WHY" "then 'refused: errno 22 (Invalid argument)'"
has "  and that the reader waits" "$RC_WHY" "the reader: waiting"
course refused_reader_ended 45 30 --refuse-one-commit "read:$RC_AT"
is "a refusal with the reader ended" "$RC_VERDICT" FAIL
has "  says what the reader got" "$RC_WHY" "the reader: ENDED: exit 1 after"
course refused_stays_live 46 30 --refuse-one-commit "read:$RC_AT"
is "a refusal that leaves the device live" "$RC_VERDICT" FAIL
course refused_hangs 52 2 --refuse-one-commit "read:$RC_AT"
is "a refusal after which the program does not end" "$RC_VERDICT" FAIL
has "  says so" "$RC_WHY" "it ended by itself: NO: it was still serving after 2 s"
course refused_other_read 53 30 --refuse-one-commit "read:$RC_AT"
is "a refusal of another read" "$RC_VERDICT" FAIL
course taken 47 2 --refuse-one-commit "read:$RC_AT"
is "a commit the kernel took (the reader got an I/O error)" "$RC_VERDICT" FAIL
has "  the server was killed" "$RC_WHY" "it ended by itself: NO: it was still serving after 2 s, and was killed"
has "  the reader's error" "$RC_WHY" "Input/output error"
course aborted 48 2 --refuse-one-commit "read:$RC_AT"
is "an aborted commit" "$RC_VERDICT" FAIL
course not_sent 49 2 --refuse-one-commit "read:$RC_AT"
is "a commit the library did not send" "$RC_VERDICT" FAIL
has "  says so" "$RC_WHY" "the program said 'not sent for a read at byte $RC_AT on tag 3: refused by the library'"
course none 50 2
is "the control: no refusal" "$RC_VERDICT" FAIL
course add_taken 51 2
is "an add that failed is the rig's error" "$RC_VERDICT" ERROR
has "  says why" "$RC_WHY" "the add under 51 answered: taken"

# rc_reader: what the reader got, against the file.
SCENARIO=refused; RC_RD=$T/rd
# shellcheck disable=SC2034  # read by rc_reader
RC_WANT=$(head -c 4096 /dev/zero | sha256sum | cut -d' ' -f1)
head -c 4096 /dev/zero > "$RC_RD.bin"; echo "exit 0 after 12 ms" > "$RC_RD.rc"
if rc_reader 1; then r=0; else r=1; fi
is "the file's data, no error" "$r $RC_READ" "0 exit 0 after 12 ms; its data is what the file holds"
head -c 4096 /dev/urandom > "$RC_RD.bin"
if rc_reader 1; then r=0; else r=1; fi
is "other data" "$r $RC_READ" "1 exit 0 after 12 ms; its data is NOT what the file holds"
head -c 4096 /dev/zero > "$RC_RD.bin"; echo "exit 1 after 12 ms: dd: error reading" > "$RC_RD.rc"
if rc_reader 1; then r=0; else r=1; fi
is "an error" "$r" 1
rm -f "$RC_RD.rc"
if rc_reader 1; then r=0; else r=1; fi
is "a reader still waiting" "$r $RC_READ" "1 still waiting after 1 s more"

if [ "$fails" = 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi

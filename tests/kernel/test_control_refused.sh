#!/bin/bash
# Tests of the control rig's refused-commit step (M1.T33): the rig's own
# refused_commit, rc_client and refused_course, run against a made-up
# test program and a made-up client of the device, one course for each
# answer the driver or the library can give, and the whole course of a
# read, a write and a flush. No kernel and no ublk are needed.
# Run: bash tests/kernel/test_control_refused.sh
HERE=$(cd "$(dirname "$0")" && pwd)
RIG=$HERE/control-rig.sh
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
if ! T=$(mktemp -d) || [ -z "$T" ] || [ ! -d "$T" ]; then echo "no temporary directory"; exit 2; fi
# The made-up clients and servers wait for this file: none outlives the test.
trap 'touch "$T/stop"; sleep 0.5; rm -rf "$T"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}
has() { # name got part
    case $2 in *"$3"*) echo "ok   $1" ;; *) echo "FAIL $1: '$3' not in '$2'"; fails=$((fails + 1)) ;; esac
}

# The rig's functions, as the rig has them.
for f in rec say info wait_state serve_bg srv_kill srv_end refused_commit rc_block rc_said rc_client refused_course; do
    eval "$(sed -n "/^$f() {/,/^}\$/p" "$RIG")"
    declare -F "$f" > /dev/null || { echo "FAIL the rig has no function $f"; exit 2; }
done
eval "$(sed -n '/^kv() /p; /^now_ms() /p; /^pin() /p; /^obs() /p; /^ctl() /p; /^step() /p' "$RIG")"
HOLD_A=$((32 << 20))
eval "$(sed -n '/^RC_AT=/p' "$RIG")"
is "the block the step reads" "$RC_AT" "$HOLD_A"

OUT=$T/out; WORK=$T/work; mkdir -p "$OUT" "$WORK"
R=$OUT/results.tsv; : > "$R"
# shellcheck disable=SC2034  # read by refused_commit, as the rig's own global
SIZE=$((64 << 20))
export T RC_AT
# The test program. SCENARIO says what the driver answers to the commit
# with no buffer. The device's state is a file; `take-over` and
# `delete` leave a file that the made-up client waits for.
CTL=$T/ctl
cat > "$CTL" <<'CTLEOF'
#!/bin/bash
step=$1; n=$2; shift 2
state() { echo "$1" > "$T/state.$n"; }
file=""; what=""
while [ $# -gt 0 ]; do
    case $1 in --file) file=$2; shift ;; --refuse-one-commit) what=$2; shift ;; esac
    shift
done
case $what in read:*) req="a read at byte ${what#read:}" ;; write:*) req="a write at byte ${what#write:}" ;; *) req="a flush" ;; esac
case $step in
    add) if [ "$SCENARIO" = add_taken ]; then printf 'add\t%s\ttaken\n' "$n"; else printf 'add\t%s\tadded\n' "$n"; fi ;;
    info) printf 'info\t%s\tstate=%s server_pid=1\n' "$n" "$(cat "$T/state.$n" 2>/dev/null || echo nothing)" ;;
    delete) state nothing; : > "$T/deleted.$n"; printf 'delete\t%s\treturned after 5 ms\n' "$n" ;;
    take-over)
        state live; echo "$file" > "$T/img.$n"
        printf 'take-over\t%s\tlive after 2 ms; handed before the start: 0; busy answers: 0\n' "$n"
        : > "$T/served.$n"
        # The test program's queue thread panicking at the reissued request.
        if [ "$SCENARIO" = take_over_dies ]; then rm -f "$T/served.$n"; state waiting; echo "the queue's thread panicked" >&2; exit 1; fi
        trap 'printf "served\t%s\tended by a signal; handed 1 reads\n" "$n"; state waiting; exit 0' TERM
        while [ ! -e "$T/stop" ]; do sleep 0.1; done ;;
    serve)
        state live; echo "$file" > "$T/img.$n"
        printf 'serve\t%s\tlive after 3 ms\n' "$n"
        sleep 0.5
        sent="refuse-commit\t$n\tsent for $req on tag 3\n"
        case $SCENARIO in
            refused|refused_reader_ended|refused_stays_live|refused_hangs|refused_other_read|lost_write|stale_device|image_lost|writer_fails|take_over_dies)
                # another read refused: one the kernel or udev sent at a start
                [ "$SCENARIO" = refused_other_read ] && sent="refuse-commit\t$n\tsent for a read at byte 0 on tag 1\n"
                printf "$sent"
                printf 'refuse-commit\t%s\trefused: errno 22 (Invalid argument)\n' "$n"
                printf 'served\t%s\tended by the refused commit; handed 1 reads 0 writes 0 flushes\n' "$n"
                [ "$SCENARIO" = refused_stays_live ] || state waiting
                [ "$SCENARIO" = refused_hangs ] || exit 0 ;;
            # The driver's answer and the program's end, each without the other.
            refused_crashes) printf "$sent"; printf 'refuse-commit\t%s\trefused: errno 22 (Invalid argument)\n' "$n"; state waiting; exit 101 ;;
            aborted_then_ended) printf "$sent"; printf 'refuse-commit\t%s\taborted\n' "$n"
                printf 'served\t%s\tended by the refused commit; handed 1 reads\n' "$n"; state waiting; exit 0 ;;
            aborted) printf "$sent"; printf 'refuse-commit\t%s\taborted\n' "$n" ;;
            taken) printf "$sent" ;;
            not_sent) printf 'refuse-commit\t%s\tnot sent for %s on tag 3: refused by the library\n' "$n" "$req" ;;
        esac
        # It serves on, until it is killed.
        while [ ! -e "$T/stop" ]; do sleep 0.1; done ;;
    *) printf '%s\t%s\tunknown\n' "$step" "$n" ;;
esac
CTLEOF
chmod +x "$CTL"
# A client of the device, and dd for everything else. A held request
# waits until the device is taken over (then a read reads the image, a
# write lands in it, and a flush ends) or deleted (an I/O error). After
# the take-over: SCENARIO=lost_write loses the write; stale_device
# answers a read of the device with zeros; image_lost keeps the write
# where a read of the device finds it and not in the image;
# writer_fails lands the write and tells the writer it failed.
DD=$T/dd
cat > "$DD" <<'DDEOF'
#!/bin/bash
dev=""; in=""; of=""; kind=""
for a in "$@"; do
    case $a in
        if=/dev/ublkb*) dev=${a#if=/dev/ublkb}; kind=read ;;
        of=/dev/ublkb*) dev=${a#of=/dev/ublkb}; kind=write ;;
        if=*) in=${a#if=} ;;
        of=*) of=${a#of=} ;;
    esac
done
[ -n "$dev" ] || exec dd "$@"
[ "$kind" = write ] && [ "$in" = /dev/null ] && kind=flush
eio() { echo "dd: error on '/dev/ublkb$dev': Input/output error" >&2; exit 1; }
case $SCENARIO in
    taken|refused_reader_ended) eio ;;
    none|not_sent) [ "$kind" = read ] && head -c 4096 /dev/zero > "${of:-/dev/stdout}"; exit 0 ;;
esac
while [ ! -e "$T/served.$dev" ] && [ ! -e "$T/deleted.$dev" ]; do
    [ -e "$T/stop" ] && exit 1
    sleep 0.1
done
[ -e "$T/deleted.$dev" ] && eio
img=$(cat "$T/img.$dev")
case $kind in
    read)
        if [ -z "$of" ] && [ "$SCENARIO" = stale_device ]; then head -c 4096 /dev/zero; exit 0; fi
        if [ -z "$of" ] && [ "$SCENARIO" = image_lost ] && [ -e "$T/cache.$dev" ]; then cat "$T/cache.$dev"; exit 0; fi
        dd if="$img" of="${of:-/dev/stdout}" bs=4k count=1 skip=$((RC_AT / 4096)) status=none ;;
    write)
        case $SCENARIO in
            lost_write) ;;
            image_lost) cp "$in" "$T/cache.$dev" ;;
            *) dd if="$in" of="$img" bs=4k count=1 seek=$((RC_AT / 4096)) conv=notrunc status=none ;;
        esac
        [ "$SCENARIO" = writer_fails ] && eio ;;
esac
exit 0
DDEOF
chmod +x "$DD"

course() { # scenario n seconds kind [serve options]
    SCENARIO=$1; export SCENARIO
    local n=$2 secs=$3; shift 3
    refused_commit "rc-$SCENARIO-$1" "$n" "$secs" "$@" > /dev/null 2>&1
}

course refused 44 30 read --refuse-one-commit "read:$RC_AT"
is "a refused commit" "$RC_VERDICT" PASS
has "  says what the driver answered" "$RC_WHY" "then 'refused: errno 22 (Invalid argument)'"
has "  and that the client waits" "$RC_WHY" "the read client: waiting"
course refused 60 30 write --refuse-one-commit "write:$RC_AT"
is "a refused write" "$RC_VERDICT" PASS
course refused 61 30 flush --refuse-one-commit flush
is "a refused flush" "$RC_VERDICT" PASS
course refused 62 30 write --refuse-one-commit "read:$RC_AT"
is "a refused read where a write was to be refused" "$RC_VERDICT" FAIL
course refused 63 30 flush --refuse-one-commit "read:$RC_AT"
is "a refused read where a flush was to be refused" "$RC_VERDICT" FAIL
course refused_reader_ended 45 30 read --refuse-one-commit "read:$RC_AT"
is "a refusal with the reader ended" "$RC_VERDICT" FAIL
has "  says what the reader got" "$RC_WHY" "the read client: ENDED: exit 1 after"
course refused_stays_live 46 30 read --refuse-one-commit "read:$RC_AT"
is "a refusal that leaves the device live" "$RC_VERDICT" FAIL
course refused_hangs 52 2 read --refuse-one-commit "read:$RC_AT"
is "a refusal after which the program does not end" "$RC_VERDICT" FAIL
has "  says so" "$RC_WHY" "it ended by itself: NO: it was still serving after 2 s"
course refused_other_read 53 30 read --refuse-one-commit "read:$RC_AT"
is "a refusal of another read" "$RC_VERDICT" FAIL
course taken 47 2 read --refuse-one-commit "read:$RC_AT"
is "a commit the kernel took (the reader got an I/O error)" "$RC_VERDICT" FAIL
has "  the server was killed" "$RC_WHY" "it ended by itself: NO: it was still serving after 2 s, and was killed"
has "  the reader's error" "$RC_WHY" "Input/output error"
course aborted 48 2 read --refuse-one-commit "read:$RC_AT"
is "an aborted commit" "$RC_VERDICT" FAIL
course refused_crashes 54 30 read --refuse-one-commit "read:$RC_AT"
is "a refusal after which the program crashed" "$RC_VERDICT" FAIL
has "  says it has no last line of its own" "$RC_WHY" "it ended by itself: yes; its last line: ''"
course aborted_then_ended 55 30 read --refuse-one-commit "read:$RC_AT"
is "a program that ended by a refusal the driver did not make" "$RC_VERDICT" FAIL
has "  says what the driver answered" "$RC_WHY" "then 'aborted'"
course not_sent 49 2 read --refuse-one-commit "read:$RC_AT"
is "a commit the library did not send" "$RC_VERDICT" FAIL
has "  says so" "$RC_WHY" "the program said 'not sent for a read at byte $RC_AT on tag 3: refused by the library'"
course none 50 2 read
is "the control: no refusal" "$RC_VERDICT" FAIL
course add_taken 51 2 read
is "an add that failed is the rig's error" "$RC_VERDICT" ERROR
has "  says why" "$RC_WHY" "the add under 51 answered: taken"

# rc_client: what a reader got, against the image's block.
# Read by rc_client and rc_said, as the rig's globals.
export RC_KIND='read' RC_NEW='new' RC_OLD
RC_RD=$T/rd
RC_OLD=$(head -c 4096 /dev/zero | sha256sum | cut -d' ' -f1)
head -c 4096 /dev/zero > "$RC_RD.bin"; echo "exit 0 after 12 ms" > "$RC_RD.rc"
if rc_client 1; then r=0; else r=1; fi
is "the file's data, no error" "$r $RC_CLIENT" "0 exit 0 after 12 ms; its data is the old block"
head -c 4096 /dev/urandom > "$RC_RD.bin"
if rc_client 1; then r=0; else r=1; fi
is "other data" "$r $RC_CLIENT" "1 exit 0 after 12 ms; its data is NEITHER the old block nor the written data"
head -c 4096 /dev/zero > "$RC_RD.bin"; echo "exit 1 after 12 ms: dd: error reading" > "$RC_RD.rc"
if rc_client 1; then r=0; else r=1; fi
is "an error" "$r" 1
RC_KIND='write'; echo "exit 0 after 12 ms" > "$RC_RD.rc"
if rc_client 1; then r=0; else r=1; fi
is "a writer that succeeded" "$r $RC_CLIENT" "0 exit 0 after 12 ms"
rm -f "$RC_RD.rc"
if rc_client 1; then r=0; else r=1; fi
is "a client still waiting" "$r $RC_CLIENT" "1 still waiting after 1 s more"

# The whole course of each kind, through the rig's refused_course: the
# take-over and its pin, then the delete and its observation.
SCENARIO=refused; export SCENARIO
: > "$R"
refused_course read 70 71 > /dev/null 2>&1
refused_course write 72 73 > /dev/null 2>&1
refused_course flush 74 75 > /dev/null 2>&1
line() { awk -F'\t' -v k="$1" '$1 == k { print $2 }' "$R" | tail -n 1; }
for kind in read write flush; do
    has "$kind: the server ends by the refusal" "$(line "pin_refused_${kind}_ends_the_server")" "PASS: the program said 'sent for a"
done
has "read: issued again, with the file's data" "$(line pin_refused_read_issued_again)" "PASS: after the take-over (live after 2 ms"
has "  and says so" "$(line pin_refused_read_issued_again)" "exit 0 after"
has "  with its data" "$(line pin_refused_read_issued_again)" "its data is the old block"
has "write: lands after the take-over" "$(line pin_refused_write_lands_after_take_over)" "PASS: "
has "  through the device and in the image" "$(line pin_refused_write_lands_after_take_over)" "the range read back through the device: the written data; from the image after the take-over ended: the written data"
has "flush: completes after the take-over" "$(line pin_refused_flush_completes_after_take_over)" "PASS: "
for kind in read write flush; do
    has "$kind, deleted: the client gets an I/O error" "$(line "obs_refused_${kind}_then_delete")" "client held by the refused commit: exit 1 after"
    has "  and the number is free" "$(line "obs_refused_${kind}_then_delete")" "the device then read nothing"
done
has "write, deleted: the image keeps the old block" "$(line obs_refused_write_then_delete)" "the image's block afterwards: the old block"
# A take-over after which the writer is told it succeeded and the data
# is not there.
SCENARIO=lost_write; : > "$R"
refused_course write 76 77 > /dev/null 2>&1
has "a write lost at the take-over fails" "$(line pin_refused_write_lands_after_take_over)" "FAIL: "
has "  says where it is missing" "$(line pin_refused_write_lands_after_take_over)" "the range read back through the device: the old block; from the image after the take-over ended: the old block"
SCENARIO=take_over_dies; : > "$R"
refused_course flush 84 85 > /dev/null 2>&1
has "a take-over that ended by itself fails" "$(line pin_refused_flush_completes_after_take_over)" "FAIL: "
has "  and is named, with its error" "$(line pin_refused_flush_completes_after_take_over)" "THE TAKE-OVER HAD ENDED BY ITSELF, with exit 1"
has "  and the program's own error" "$(line pin_refused_flush_completes_after_take_over)" "the queue's thread panicked"
SCENARIO=stale_device; : > "$R"
refused_course write 78 79 > /dev/null 2>&1
has "a write the device does not read back fails" "$(line pin_refused_write_lands_after_take_over)" "FAIL: "
has "  says so" "$(line pin_refused_write_lands_after_take_over)" "through the device: NEITHER the old block nor the written data; from the image after the take-over ended: the written data"
SCENARIO=image_lost; : > "$R"
refused_course write 80 81 > /dev/null 2>&1
has "a write that is not in the image fails" "$(line pin_refused_write_lands_after_take_over)" "FAIL: "
has "  says so" "$(line pin_refused_write_lands_after_take_over)" "through the device: the written data; from the image after the take-over ended: the old block"
SCENARIO=writer_fails; : > "$R"
refused_course write 82 83 > /dev/null 2>&1
has "a write that landed and was answered with an error fails" "$(line pin_refused_write_lands_after_take_over)" "FAIL: "
has "  says so" "$(line pin_refused_write_lands_after_take_over)" "Input/output error"

if [ "$fails" = 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi

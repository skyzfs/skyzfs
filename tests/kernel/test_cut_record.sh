#!/bin/bash
# Tests of cut-record.sh's handling of the holder's end, with stand-in
# holders: no kernel, ZFS or daemon. The recording runs under set -e, so a
# holder's status that is not taken with || ends the recording before the
# check, as the rig's error (the review of #113, D1).
# Run: bash tests/kernel/test_cut_record.sh
# The stand-in holders' scripts are single-quoted: they expand in their own shell.
# shellcheck disable=SC2016
HERE=$(cd "$(dirname "$0")" && pwd)
SCRIPT=$HERE/cut-record.sh
if ! T=$(mktemp -d) || [ -z "$T" ] || [ ! -d "$T" ]; then echo "no temporary directory"; exit 2; fi
trap 'rm -rf "$T"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}

# The functions as the script has them.
fn=$(sed -n '/^holder_ended() {$/,/^}$/p' "$SCRIPT")
is "holder_ended is in the script" "$([ -n "$fn" ] && echo yes)" yes
wrote=$(sed -n '/^holder_wrote_one() {$/,/^}$/p' "$SCRIPT")
is "holder_wrote_one is in the script" "$([ -n "$wrote" ] && echo yes)" yes
gone=$(sed -n '/^holder_gone() {$/,/^}$/p' "$SCRIPT")
is "holder_gone is in the script" "$([ -n "$gone" ] && echo yes)" yes
idle=$(sed -n '/^hold_idle() {$/,/^}$/p' "$SCRIPT")
is "hold_idle is in the script" "$([ -n "$idle" ] && echo yes)" yes

# One recording's end: a stand-in holder that writes LOG and ends with
# STATUS, then holder_ended under set -e as the recording calls it.
# -> "went on" when the recording reaches the check, else what ended it.
recording() { # status log
    # The names are the function's, which eval brings in.
    # shellcheck disable=SC2034,SC2329
    (
        set -euo pipefail
        RDIR=$T/r; NAME=rec; mkdir -p "$RDIR"; printf '%b' "$2" > "$RDIR/holder.log"
        log() { :; }
        rig_error() { echo "rig error: $*" > "$T/why"; exit 2; }
        eval "$fn"
        bash -c "exit $1" & HOLD_PID=$!
        holder_ended "at the stop"
        echo "went on" > "$T/why"
    )
    echo "$? $(cat "$T/why" 2>/dev/null)"
    rm -f "$T/why"
}

is "a holder that ends with 0" "$(recording 0 '1\tdrained: 2 cuts written\n')" "0 went on"
is "a holder that ends with its verdict goes on to the check" \
    "$(recording 1 '1\tVERDICT cut 1 is still being uploaded after its write was refused\n')" "0 went on"
is "1 without a VERDICT line is the rig's error" "$(recording 1 '1\tleft\n')" \
    "2 rig error: the holder ended with 1 at the stop: 1	left"
is "the holder's own error is the rig's error" "$(recording 2 '1\tERROR HolderError: x\n')" \
    "2 rig error: the holder ended with 2 at the stop: 1	ERROR HolderError: x"

# The wait for the holder's first cut, as rec_takeover and rec_failed call
# it: a stand-in holder that runs SCRIPT. -> "wrote one", "ended early"
# (the recording ends there and goes to the check), or what ended it.
first_cut() { # holder-script
    # shellcheck disable=SC2034,SC2329
    (
        set -euo pipefail
        RDIR=$T/w; NAME=rec; rm -rf "$RDIR"; mkdir -p "$RDIR"; : > "$RDIR/holder.log"
        log() { :; }
        rig_error() { echo "rig error: $*" > "$T/why"; exit 2; }
        eval "$fn"
        eval "$gone"
        eval "$wrote"
        RDIR=$RDIR bash -c "$1" & HOLD_PID=$!
        if holder_wrote_one; then echo "wrote one" > "$T/why"; else echo "ended early" > "$T/why"; fi
        [ -z "$HOLD_PID" ] || kill "$HOLD_PID" 2>/dev/null || true
    )
    echo "$? $(cat "$T/why" 2>/dev/null)"
    rm -f "$T/why"
}
is "a holder that wrote a cut" "$(first_cut 'printf "1\t5\titself\t2\t1\t2\n" > "$RDIR/held.tsv"; sleep 30')" "0 wrote one"
is "a verdict before the first cut ends the recording for the check" \
    "$(first_cut 'printf "1\tVERDICT cut 1 is still being uploaded\n" >> "$RDIR/holder.log"; exit 1')" "0 ended early"
is "a holder that ended with 0 before a cut is the rig's error" "$(first_cut 'exit 0')" \
    "2 rig error: the holder ended with 0 before it wrote a cut: "
is "a holder that ended with its error before a cut is the rig's error" \
    "$(first_cut 'printf "1\tERROR HolderError: x\n" >> "$RDIR/holder.log"; exit 2')" \
    "2 rig error: the holder ended with 2 before it wrote a cut: 1	ERROR HolderError: x"
is "the recordings end at a verdict before the first cut" \
    "$(grep -c '^    holder_wrote_one || { export_pool; return 0; }$' "$SCRIPT")" 2

# The wait for the holder to pause with nothing held, as rec_edges calls
# it: a stand-in holder that runs SCRIPT, with WORK/hold.pause the rig's
# ask. -> "idle", "ended early" (the recording ends there and goes to the
# check), or what ended it.
paused() { # holder-script
    # shellcheck disable=SC2034,SC2329
    (
        set -euo pipefail
        RDIR=$T/p; WORK=$T/pw; NAME=rec; POOL=p; rm -rf "$RDIR" "$WORK"; mkdir -p "$RDIR" "$WORK"
        : > "$RDIR/holder.log"
        log() { :; }
        z() { :; }
        rig_error() { echo "rig error: $*" > "$T/why"; exit 2; }
        eval "$fn"
        eval "$gone"
        eval "$idle"
        RDIR=$RDIR WORK=$WORK bash -c "$1" & HOLD_PID=$!
        if hold_idle; then echo "idle" > "$T/why"; else echo "ended early" > "$T/why"; fi
        [ -z "$HOLD_PID" ] || kill "$HOLD_PID" 2>/dev/null || true
    )
    echo "$? $(cat "$T/why" 2>/dev/null)"
    rm -f "$T/why"
}
is "a holder that paused with nothing held" \
    "$(paused 'until [ -e "$WORK/hold.pause" ]; do sleep 0.1; done; : > "$WORK/hold.pause.held"; sleep 30')" "0 idle"
is "a verdict while the holder is asked to pause ends the recording for the check" \
    "$(paused 'printf "1\tVERDICT cut 1 is still being uploaded\n" >> "$RDIR/holder.log"; exit 1')" "0 ended early"
is "a holder that ended with 0 while asked to pause is the rig's error" "$(paused 'exit 0')" \
    "2 rig error: the holder ended with 0 while it was asked to pause: "
is "a holder that ended with its error while asked to pause is the rig's error" \
    "$(paused 'printf "1\tERROR HolderError: x\n" >> "$RDIR/holder.log"; exit 2')" \
    "2 rig error: the holder ended with 2 while it was asked to pause: 1	ERROR HolderError: x"
is "edges ends at a verdict while the holder is asked to pause" \
    "$(grep -c '^    hold_idle || { export_pool; return 0; }$' "$SCRIPT")" 1
is "no wait for the holder without a look at whether it lives" \
    "$(grep -c 'kill -0 "$HOLD_PID" 2>/dev/null || { holder_gone "' "$SCRIPT")" 2

# Every wait for the holder takes its status with ||, outside cleanup
# (which runs with set +e): a bare one ends the recording at its status.
bare=$(awk '/^cleanup\(\) \{$/ { c = 1 } c && /^}$/ { c = 0; next } !c && /wait "\$HOLD_PID"/ && !/wait "\$HOLD_PID" 2>\/dev\/null \|\|/ { print NR": "$0 }' "$SCRIPT")
is "no bare wait for the holder" "$bare" ""
n=$(grep -c 'holder_ended "' "$SCRIPT")
is "the holder's leave, its stop and an early end all go through holder_ended" "$n" 3

# The daemon's start as unostore has it: PRE_SERVE pauses the watcher
# (WORK/watch.pause) and the watcher must start paused. A stand-in
# watcher says whether the pause was there when it started. -> "paused"
# or "not paused".
begin=$(sed -n '/^begin_daemon() {$/,/^}$/p' "$SCRIPT")
wstart=$(sed -n '/^watcher_start() {$/,/^}$/p' "$SCRIPT")
is "begin_daemon and watcher_start are in the script" "$([ -n "$begin" ] && [ -n "$wstart" ] && echo yes)" yes
started() { # pre-serve-body
    # shellcheck disable=SC2034,SC2329
    (
        set -euo pipefail
        RDIR=$T/s; WORK=$T/sw; HERE=$T/sh; RUN=$T/sr; VOL=$T/sv; NAME=rec
        rm -rf "$RDIR" "$WORK" "$HERE"; mkdir -p "$RDIR" "$WORK" "$HERE"
        printf 'import os, sys\nopen(sys.argv[1] + "/seen", "w").write("paused" if os.path.exists(os.environ["CUT_WATCH_PAUSE"]) else "not paused")\n' \
            > "$HERE/cut_watch.py"
        UPLOAD=1; HOLD=0; VDEV_MB=1; DEV=d; SKYZFSD=true; NODEB=true; S3_ENDPOINT=e; S3_BUCKET=b
        log() { :; }
        nodeb() { :; }
        serve_start() { :; }
        rig_error() { echo "rig error: $*" > "$T/why"; exit 2; }
        eval "pre() { $1; }"
        PRE_SERVE=pre
        touch "$WORK/watch.pause"     # a pause left by the recording before
        eval "$wstart"
        eval "$begin"
        begin_daemon
        wait "$WATCH_PID"
        cat "$RDIR/seen" > "$T/why"
    )
    echo "$? $(cat "$T/why" 2>/dev/null)"
    rm -f "$T/why"
}
is "a pause PRE_SERVE asked for holds past the watcher's start" "$(started 'touch "$WORK/watch.pause"')" "0 paused"
is "a pause left from the recording before does not" "$(started ':')" "0 not paused"

[ "$fails" = 0 ] && echo "all passed"
exit $((fails > 0))

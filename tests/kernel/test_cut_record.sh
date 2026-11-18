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

# The other writer's act (umoved, urow): a row that moved between the
# look and the act (node B's exit 6) means look again and act again, up
# to five times. A stand-in look writes a row whose number counts the
# looks, on group 40; the stand-in act ends with 6 where the row it was
# given is not the last look's, or until the number reaches MOVES+1, and
# ignores the row it was given where UNGUARDED is 1. Each call on a row
# that is not the last look's is a control. -> "acted on N after K
# agains, C controls on groups G, L logged", C counting the calls, G the
# groups they named, L the lines of guard-controls.txt; or what ended
# it.
oact=$(sed -n '/^other_act() {$/,/^}$/p' "$SCRIPT")
rcut=$(grep '^rowcut() ' "$SCRIPT")
rskey=$(grep '^skey() ' "$SCRIPT")
is "other_act and rowcut are in the script" "$([ -n "$oact" ] && [ -n "$rcut" ] && echo yes)" yes
acted() { # moves [unguarded]
    # shellcheck disable=SC2034,SC2329
    (
        set -euo pipefail
        RDIR=$T/a; NAME=rec; MOVES=$1; UNGUARDED=${2:-0}; rm -rf "$RDIR"; mkdir -p "$RDIR"; : > "$RDIR/looks"
        log() { :; }
        rig_error() { echo "rig error: $*" > "$T/why"; exit 2; }
        eval "$rcut"
        eval "$oact"
        before_other() { echo x >> "$RDIR/looks"; printf 'row\tab\t%s\t40\n' "$(wc -l < "$RDIR/looks")" > "$RDIR/$1-row.txt"; }
        act() {
            local cur
            cur=$(wc -l < "$RDIR/looks")
            { [ "$2" = "$cur" ] && [ "$3" = 40 ]; } || echo "$3" >> "$RDIR/control-calls"
            [ "$UNGUARDED" = 1 ] || { [ "$2" = "$cur" ] && [ "$3" = 40 ]; } || return 6
            [ "$cur" -gt "$MOVES" ] || return 6
            echo "$1 $2 $3" > "$RDIR/acted"
        }
        other_act step act
        echo "acted on $(cat "$RDIR/acted") after $(cat "$RDIR/acts-again.txt" 2>/dev/null | wc -l) agains," \
            "$(wc -l < "$RDIR/control-calls") controls on groups $(sort -u "$RDIR/control-calls" | paste -sd, -)," \
            "$(wc -l < "$RDIR/guard-controls.txt") logged" > "$T/why"
    )
    echo "$? $(cat "$T/why" 2>/dev/null)"
    rm -f "$T/why"
}
is "an act on the row as read" "$(acted 0)" "0 acted on ab 1 40 after 0 agains, 1 controls on groups 0, 1 logged"
is "a row that moved twice: the third look's row" "$(acted 2)" "0 acted on ab 3 40 after 2 agains, 3 controls on groups 0, 3 logged"
is "a row that moved five times is the rig's error" "$(acted 5)" \
    "2 rig error: node B's act at step found the row moved five times in a row"
is "an act that does not check the row it was given is the rig's error" "$(acted 0 1)" \
    "2 rig error: node B's act at step on the row ab:1:0, which it did not read, ended with 0, not 6: the guard is not wired"

# The look before the other writer's act: where the daemon cuts after
# the look, the watcher cannot fetch that cut under that head, and the
# look is taken again (run 44280164949, urow). A stand-in node B's row
# names cut L at look L, and its later read names cut L+1 while L is at
# most MOVES; the stand-in watcher fetches the cut a look names once L
# is past MOVES. The stand-in drain's status says cut group 39, one
# before the row's, at its first STALE looks (F1), then AFTER (40, the
# row's, unless given). -> "fetched
# N after K looks again, synced S", or what ended it.
bother=$(sed -n '/^before_other() {$/,/^}$/p' "$SCRIPT")
is "before_other is in the script" "$([ -n "$bother" ] && echo yes)" yes
looked() { # moves [pool imported: 1 or 0] [stale looks] [the status's group after them]
    # shellcheck disable=SC2034,SC2329
    (
        set -euo pipefail
        RDIR=$T/b; NAME=rec; POOL=p; MOVES=$1; UP=${2:-1}; STALE=${3:-0}; AFTER=${4:-40}; rm -rf "$RDIR"; mkdir -p "$RDIR"; : > "$RDIR/looks"
        log() { :; }
        sleep() { :; }
        rig_error() { echo "rig error: $*" > "$T/why"; exit 2; }
        zpool() { [ "$UP" = 1 ]; }
        z() { shift; echo x >> "$RDIR/syncs"; }
        drain() {
            echo x >> "$RDIR/drains"
            local cg=$AFTER
            [ "$(wc -l < "$RDIR/drains")" -gt "$STALE" ] || cg=39
            printf 'seconds 0\nupload_state idle\ncuts_held 0\ncut_group %s\n' "$cg" > "$RDIR/$1"
        }
        eval "$rcut"
        eval "$rskey"
        eval "$bother"
        nbrow() {
            local k
            case $1 in
            *-row.txt)
                echo x >> "$RDIR/looks"; k=$(wc -l < "$RDIR/looks")
                [ "$k" -le "$MOVES" ] || printf '%s\t40\tst\thd\n' "$k" >> "$RDIR/uploaded.tsv" ;;
            *) k=$(wc -l < "$RDIR/looks"); [ "$k" -gt "$MOVES" ] || k=$((k + 1)) ;;
            esac
            printf 'head\thd\t4\nrow\tst\t%s\t40\n' "$k" > "$RDIR/$1"
        }
        before_other step
        echo "fetched $(rowcut step-row.txt) after $(cat "$RDIR/looks-again.txt" 2>/dev/null | wc -l) looks again," \
            "synced $(cat "$RDIR/syncs" 2>/dev/null | wc -l)" > "$T/why"
    )
    echo "$? $(cat "$T/why" 2>/dev/null)"
    rm -f "$T/why"
}
is "a cut the watcher fetched" "$(looked 0)" "0 fetched st 1 40 after 0 looks again, synced 1"
is "a daemon that cut after two looks: the third look's cut" "$(looked 2)" "0 fetched st 3 40 after 2 looks again, synced 3"
is "a pool not imported is not synced" "$(looked 0 0)" "0 fetched st 1 40 after 0 looks again, synced 0"
is "a row that moved on five times is the rig's error" "$(looked 5)" \
    "2 rig error: the row or the status moved on after the look before step five times in a row"
is "a status one upload older than its row: the look again" "$(looked 0 1 1)" \
    "0 fetched st 2 40 after 1 looks again, synced 2"
is "a status ahead of its row (a cut held, or abandoned): the judge's, no look again" "$(looked 0 1 0 41)" \
    "0 fetched st 1 40 after 0 looks again, synced 1"
is "a status older than its row five times is the rig's error" "$(looked 0 1 5)" \
    "2 rig error: the row or the status moved on after the look before step five times in a row"

[ "$fails" = 0 ] && echo "all passed"
exit $((fails > 0))

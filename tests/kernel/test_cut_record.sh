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

# bound's volume is written through at its create (--zero-fill), so that
# it has no holes and its first writes are copied; boundsparse's is not
# (M2.4e). The course's settings, as record() has them, then the create.
course=$(sed -n '/^record() {$/,/^}$/p' "$SCRIPT" | sed -n '/^    case \$1 in$/,/^    esac$/p')
is "record() sets the courses' settings" "$(grep -c 'CREATE_ARGS' <<< "$course")" 1
args_for() { ( CREATE_ARGS=(); set -- "$1"; eval "$course"; echo "${CREATE_ARGS[*]}" ) 2>&1; }
is "bound: --zero-fill" "$(args_for bound)" "--zero-fill"
is "boundsparse: sparse" "$(args_for boundsparse)" ""
is "life: sparse" "$(args_for life)" ""
is "the create takes the course's arguments" \
    "$(grep -c '"$SKYZFSD" create --dir "$VOL" .*"${CREATE_ARGS\[@\]}"' "$SCRIPT")" 1
is "boundsparse is a course of the held cuts, and does what bound does" \
    "$(grep -cx 'HELD_ONLY="bound boundsparse edges takeover failed"' "$SCRIPT")$(grep -cx 'rec_boundsparse() { rec_bound; }' "$SCRIPT")" 11

# rec_bound reads the status once the ten writes are synced and while
# the holder is still paused, the first cut held (status-bound-held.txt,
# cut_copied_held in cut_daemon_check.py), and lets it go after.
rb=$(sed -n '/^rec_bound() {$/,/^}$/p' "$SCRIPT")
# shellcheck disable=SC2034,SC2317,SC2329  # the stubs and names are rec_bound's, run through eval
bound_trace() { (
    WORK=$(mktemp -d); MNT=/nonexistent; POOL=p
    create_pool() { echo "create paused=$([ -e "$WORK/hold.pause" ] && echo yes || echo no)"; }
    dd() { echo dd; }
    sleep() { echo "sleep $1"; }
    z() { echo "z $*"; }
    ustatus() { echo "status $1 paused=$([ -e "$WORK/hold.pause" ] && echo yes || echo no)"; }
    load() { echo "load $1"; }
    export_pool() { echo export; }
    eval "$rb"; rec_bound; rm -rf "${WORK:?}"
) 2>&1; }
is "rec_bound: paused before the create, the writes, a wait, then the status while paused, then the let-go" \
    "$(bound_trace | grep -c '^dd$') $(bound_trace | head -n 1) $(bound_trace | tail -n 5 | tr '\n' ';')" \
    "10 create paused=yes z 60 zpool sync p;sleep 0.3;status status-bound-held.txt paused=yes;load 2;export;"

# status_until reads the status again, with nothing done to the pool,
# until a count the daemon posts on its own shows (run 45529175986: the
# peer refused, and control_peer 0 in the status read right after it).
su_fn=$(sed -n '/^status_until() {$/,/^}$/p' "$SCRIPT")
is "status_until is in the script" "$([ -n "$su_fn" ] && echo yes)" yes
# shellcheck disable=SC2034,SC2317,SC2329  # the stubs and names are status_until's, run through eval
until_trace() { ( # the count each look reads, the limit
    RDIR=$(mktemp -d); NAME=pchannel; looks=0
    read -r -a counts <<< "$1"
    ustatus() { echo "control_peer ${counts[$looks]}" > "$RDIR/$1"; looks=$((looks + 1)); }
    skey() { sed -n "s/^$2 //p" "$RDIR/$1" | head -n 1; }
    sleep() { :; }
    log() { echo "log $*"; }
    eval "$su_fn"
    status_until s.txt control_peer 1 "$2"; rc=$?
    echo "$rc looks=$looks file=$(cat "$RDIR/s.txt")"; rm -rf "${RDIR:?}"
) 2>&1; }
is "status_until: the count shows at the third look" "$(until_trace "0 0 1 1" 10)" "0 looks=3 file=control_peer 1"
is "status_until: a count already there is one look" "$(until_trace "2" 10)" "0 looks=1 file=control_peer 2"
is "status_until: a count that never shows is 1, said, after the last look" \
    "$(until_trace "0 0" 0)" "log pchannel: the status's control_peer was 0, not at least 1, 0 s on
1 looks=1 file=control_peer 0"
is "pchannel waits for the peer's count before it reads the channel's status" \
    "$(sed -n '/^rec_pchannel() {$/,/^}$/p' "$SCRIPT" | grep -A3 '^    ppeer$' | grep -c '^    status_until channel-status.txt control_peer 1 10 || true$')" 1

# row_past reads node B's row until it names a cut counted after C of
# group G or later (the premises of prewind and pkill, #139's B2 and B3),
# nudging the pool between looks only when asked to.
rp_fn=$(sed -n '/^row_past() {$/,/^}$/p' "$SCRIPT")
is "row_past is in the script" "$([ -n "$rp_fn" ] && echo yes)" yes
# shellcheck disable=SC2034,SC2317,SC2329  # the stubs and names are row_past's, run through eval
rowpast_trace() { ( # the rows each look reads ("N G" or "-"), C, G, the limit, nudge
    RDIR=$(mktemp -d); NAME=course; MNT=$RDIR; POOL=p; looks=0
    read -r -a rows <<< "$1"
    nbrow() { local r=${rows[$looks]}; looks=$((looks + 1)); [ "$r" = - ] && : > "$RDIR/$1" ||
        printf 'row\tS\t%s\t%s\n' "${r%,*}" "${r#*,}" > "$RDIR/$1"; }
    rowcut() { awk -F '\t' '$1 == "row" && NF == 4 { print $2, $3, $4 }' "$RDIR/$1"; }
    dd() { echo dd; }
    z() { echo "z $*"; }
    sleep() { :; }
    log() { echo "log $*"; }
    eval "$rp_fn"
    row_past r.txt "$2" "$3" "$4" ${5:+"$5"}; rc=$?
    echo "$rc looks=$looks"; rm -rf "${RDIR:?}"
) 2>&1; }
is "row_past: the third row is past C and G" "$(rowpast_trace "7,50 8,49 8,50" 7 50 10)" "0 looks=3"
is "row_past: nudges between looks when asked, not after the last" \
    "$(rowpast_trace "7,50 8,50" 7 50 10 nudge)" "dd
z 60 zpool sync p
0 looks=2"
is "row_past: a cut at C is not past it, and the limit is 1, said" \
    "$(rowpast_trace "7,60" 7 50 0)" "log course: node B's row names cut 7 of group 60 (S), not one after 7 of group 50 or later, 0 s on
1 looks=1"
is "row_past: no row at all is said as none" \
    "$(rowpast_trace "-" 7 50 0)" "log course: node B's row names cut none of group none, not one after 7 of group 50 or later, 0 s on
1 looks=1"

# end_row reads node B's row at a course's end once the uploader is at
# rest, until two reads agree (run 45690023761, uv1: a read across a
# commit named the head of one and the row of the other).
er_fn=$(sed -n '/^end_row() {$/,/^}$/p' "$SCRIPT")
is "end_row is in the script" "$([ -n "$er_fn" ] && echo yes)" yes
# shellcheck disable=SC2034,SC2317,SC2329  # the stubs are end_row's, run through eval
endrow_trace() { ( # the reads, each "HEAD:COMMIT:EXPIRY"
    RDIR=$(mktemp -d); looks=0
    read -r -a reads <<< "$1"
    drain() { echo "drain $1"; }
    nbrow() { local r=${reads[$looks]}; looks=$((looks + 1)); IFS=: read -r h c e <<< "$r"
        printf 'head\t%s\t3\nrow\tS\t8\t14\nrow_commit\t%s\nlease\tme\t1\t%s\n' "$h" "$c" "$e" > "$RDIR/$1"; }
    sleep() { :; }
    rig_error() { echo "rig_error $*"; exit 2; }
    eval "$er_fn"
    end_row; rc=$?
    echo "$rc looks=$looks head=$(awk -F '\t' '$1 == "head" { print $2 }' "$RDIR/end-row.txt") reads=$(grep -c '^read ' "$RDIR/end-rows.txt")"
    rm -rf "${RDIR:?}"
) 2>&1; }
is "end_row: a torn first read, then two that agree: drained first, the last kept" \
    "$(endrow_trace "a:b:1 b:b:2 b:b:3")" "drain end-drain.txt
0 looks=3 head=b reads=3"
is "end_row: the lease's expiry is not compared" "$(endrow_trace "b:b:1 b:b:2")" "drain end-drain.txt
0 looks=2 head=b reads=2"
is "end_row: ten reads that never agree are the rig's error" \
    "$(endrow_trace "a:a:1 b:b:1 a:a:1 b:b:1 a:a:1 b:b:1 a:a:1 b:b:1 a:a:1 b:b:1")" "drain end-drain.txt
rig_error node B's row at the end did not read the same twice in a row in ten reads (end-rows.txt)"
for c in umoved urow uv1 uv1moved ulost; do
    is "$c: ends with end_row after the export" "$(sed -n "/^rec_$c() {\$/,/^}\$/p" "$SCRIPT" | grep -E '^    (export_pool|end_row|nbrow end-row)' | tail -n 2 | tr -d ' ' | tr '\n' ' ')" "export_pool end_row "
done
is "no course reads the end row but through end_row" "$(grep -c 'nbrow end-row.txt' "$SCRIPT")" 1

# asked waits for the agent's publish of T to have its answer.
ak_fn=$(sed -n '/^asked() {$/,/^}$/p' "$SCRIPT")
is "asked is in the script" "$([ -n "$ak_fn" ] && echo yes)" yes
# shellcheck disable=SC2034,SC2317,SC2329  # run through eval
asked_trace() { ( # agent.tsv's text, T
    RDIR=$(mktemp -d); printf '%b' "$1" > "$RDIR/agent.tsv"
    sleep() { :; }
    eval "$ak_fn"
    asked "$2" 0; echo "rc=$?"; rm -rf "${RDIR:?}"
) 2>&1; }
is "asked: an answered publish of r1" "$(asked_trace '1\t2\tpublish --name r1 --attachment a\t0\taccepted\n' r1)" "rc=0"
is "asked: a publish of r1 with no answer yet" "$(asked_trace '1\t2\tpublish --name r1 --attachment a\t0\t\n' r1)" "rc=1"
is "asked: another name's, or r1's look, is not it" \
    "$(asked_trace '1\t2\tpublish --name r12 --a\t0\taccepted\n1\t2\tprogress --name r1\t0\tpending\n' r1)" "rc=1"

# panew asks T anew with a count read now, and T's last G and manifest.
pn_fn=$(sed -n '/^panew() {$/,/^}$/p;/^preq() /p' "$SCRIPT")
is "panew is in the script" "$([ -n "$pn_fn" ] && echo yes)" yes
# shellcheck disable=SC2034,SC2317,SC2329  # run through eval
panew_trace() { ( # the cuts answer, record or not
    RDIR=$(mktemp -d)
    printf 'r1\t50\taa\tbb\t7\t/m\t48\n' > "$RDIR/published.tsv"
    pagent() { echo "$1 $*" >> "$RDIR/calls"; [ "$2" = cuts ] && echo "$CUTS"; return 0; }
    pask() { echo "pask $*"; }
    rig_error() { echo "rig_error $*"; exit 2; }
    CUTS=$1
    eval "$pn_fn"
    panew r1-anew r1 "$2"
    tail -n 1 "$RDIR/published.tsv"; rm -rf "${RDIR:?}"
) 2>&1; }
A32=$(printf 'a%.0s' $(seq 32)); S32=$(printf 'b%.0s' $(seq 32))
is "panew: asked with the count read now, G and the manifest of the last request, and recorded" \
    "$(panew_trace "cuts attachment=$A32 state=$S32 count=19 group=33 opens=itself" record)" \
    "pask r1-anew publish r1 $A32 $S32 19 50 /m
r1	50	$A32	$S32	19	/m	48"
is "panew: not recorded without record" \
    "$(panew_trace "cuts attachment=$A32 state=$S32 count=19 group=33 opens=itself" '')" \
    "pask r1-anew publish r1 $A32 $S32 19 50 /m
r1	50	aa	bb	7	/m	48"
is "panew: a cuts answer with no state is the rig's error" \
    "$(panew_trace "cuts attachment=$A32 state=none count=0 group=none opens=none" record)" \
    "rig_error a cuts answer the rig cannot read: cuts attachment=$A32 state=none count=0 group=none opens=none"

# past_top syncs a group at a time until the newest cut opens as itself.
pt_fn=$(sed -n '/^past_top() {$/,/^}$/p' "$SCRIPT")
is "past_top is in the script" "$([ -n "$pt_fn" ] && echo yes)" yes
# shellcheck disable=SC2034,SC2317,SC2329  # run through eval
past_top_trace() { ( # the opens of each cuts answer in turn, the limit
    RDIR=$(mktemp -d); MNT=$RDIR; NAME=course
    # shellcheck disable=SC2086  # one answer a word
    printf '%s\n' $1 > "$RDIR/opens"
    pagent() { local o; o=$(head -n 1 "$RDIR/opens"); sed -i 1d "$RDIR/opens"; echo "cuts count=1 opens=${o/:/ }"; }
    z() { echo sync >> "$RDIR/syncs"; }
    dd() { :; }; sleep() { :; }
    log() { echo "log $*"; }
    rig_error() { echo "rig_error $*"; exit 2; }
    eval "$pt_fn"
    past_top "$2"; echo "rc=$? syncs=$(wc -l < "$RDIR/syncs" 2>/dev/null || echo 0)"; rm -rf "${RDIR:?}"
) 2>&1; }
is "past_top: rewind, rewind, itself: two syncs, then done" \
    "$(past_top_trace "rewind rewind itself" 60)" "log course: the newest cut opens as itself: cuts count=1 opens=itself
rc=0 syncs=2"
is "past_top: a limit passed with no cut opening as itself is the rig's error" \
    "$(past_top_trace "rewind rewind" 0)" "rig_error no cut opened as itself 0 s on: cuts count=1 opens=rewind"
# Protocol 3 (M2.7b-1): the answer ends with rewound=R.
is "past_top: protocol 3's rewind, then itself with rewound=: one sync, then done" \
    "$(past_top_trace "rewind:rewound=95 itself:rewound=95" 60)" \
    "log course: the newest cut opens as itself: cuts count=1 opens=itself rewound=95
rc=0 syncs=1"
is "past_top: protocol 3's rewind alone is never itself" \
    "$(past_top_trace "rewind:rewound=95" 0)" "rig_error no cut opened as itself 0 s on: cuts count=1 opens=rewind rewound=95"
is "past_top: opens=itselfx is not itself" \
    "$(past_top_trace "itselfx" 0)" "rig_error no cut opened as itself 0 s on: cuts count=1 opens=itselfx"

# The order the premises need (#139's B2 and B3).
order_of() { sed -n "/^$1() {\$/,/^}\$/p" "$SCRIPT" | grep -nE "$2" | cut -d: -f2- | sed 's/^ *//; s/ .*//' | tr '\n' ' '; }
is "prewind: asks (refused rewound), brings the store back, waits for the rewind cut's commit, passes the abandoned top, then asks anew" \
    "$(order_of rec_prewind '^    (pflow|asked|bash -c "\$S3_UP"|row_past|rm -f "\$WORK/watch.pause"|past_top|panew|pwait)')" \
    "pflow bash row_past rm past_top panew pwait "
is "pkill: tk asked again as it was, then anew with a count read then" \
    "$(order_of rec_pkill '^    (pask tk-after|pagain|panew|pwait tk-end)')" \
    "pask pagain panew pwait "
is "prewind: the agent does not sync groups itself" \
    "$(sed -n '/^rec_prewind() {$/,/^}$/p' "$SCRIPT" | grep -c 'pflow r1 .*--sync')" 0
is "pkill: withdraws, waits for a commit past tw's G, reads the status, then asks tk and kills" \
    "$(order_of rec_pkill '^    (pask tw-withdraw|row_past|ustatus|pflow tk|kill_serve)')" \
    "pask row_past ustatus pflow kill_serve "
is "pchannel: records the socket's mode before the rig widens it" \
    "$(sed -n '/^ppeer() {$/,/^}$/p' "$SCRIPT" | grep -nE '^    (stat -c %a "\$sock"|chmod 0666)' | cut -d: -f2 | sed 's/^ *//; s/ .*//' | tr '\n' ' ')" \
    "stat chmod "

is "prewind: reads the abandoned top after the export and before the rewind" \
    "$(order_of rec_prewind '^    (export_pool|out=\$\(pagent ask cuts|echo "\$top"|import_pool -T)')" \
    "export_pool out=\$(pagent echo import_pool export_pool "
top_line=$(sed -n '/^rec_prewind() {$/,/^}$/p' "$SCRIPT" | grep -m1 '^    top=')
is "prewind's top line is in the script" "$([ -n "$top_line" ] && echo yes)" yes
# shellcheck disable=SC2034,SC2154  # out and top are the top line's, run through eval
is "prewind's top is the group of the cuts answer" \
    "$(out='cuts attachment=a state=b count=19 group=43 opens=itself'; eval "$top_line"; echo "$top")" 43
# shellcheck disable=SC2034,SC2154  # out and top are the top line's, run through eval
is "prewind's top before any cut is empty (the rig's error)" \
    "$(out='cuts attachment=a state=b count=0 group=none opens=none'; eval "$top_line"; echo "[$top]")" "[]"

# rec_pkillat's steps, and which requests asked anew are the ones judged
# (#140's review, N5): r1, tk and m1 by the request asked anew, p1 by its
# first, since the publish found was made for it.
pk_fn=$(sed -n '/^rec_pkillat() {$/,/^}$/p' "$SCRIPT")
is "pkillat: after the take-over, asks how T stands, asks again, then anew, waits, and looks at p1" \
    "$(grep -nE '^ +(pask "\$t-after"|pagain "\$t-again"|if \[ "\$t" = p1 \]|panew "\$t-anew"|pwait "\$t-end"|pask "\$t-look" look|else)' <<< "$pk_fn" | cut -d: -f2- | sed 's/^ *//; s/ .*//' | tr '\n' ' ')" \
    "pask pagain if panew pwait pask else panew pwait "
is "pkillat: p1 asked anew is not recorded, the others are" \
    "$(sed -n '/^    if \[ "\$t" = p1 \]; then$/,/^    fi$/p' <<< "$pk_fn" | grep -E '^ +panew ' | sed 's/^ *//')" \
    'panew "$t-anew" "$t"
panew "$t-anew" "$t" record'
is "every panew of the script, with its arguments" \
    "$(grep -E '^ +panew ' "$SCRIPT" | sed 's/^ *//')" \
    'panew r1-anew r1 record
panew tk-anew tk record
panew "$t-anew" "$t"
panew "$t-anew" "$t" record'
is "pkillat: asking again records nothing (the take-over's request is refused)" \
    "$(grep -cE '^ +pagain (tk-again|"\$t-again") .* record$' "$SCRIPT")" 0

# prewindf: f1 pends, then the pool is exported, its newest root block
# zeroed and imported with -F or else -FX; f1's end is read after that, with the
# pool synced between looks (the lead's addition to M2.6a's rig).
is "prewindf: f1 pends, export, the root block zeroed, import -F (or -FX), then f1's end" \
    "$(order_of rec_prewindf '^    (pflow f1|export_pool|zero_newest_root|import_rewind_f|import_pool|pwait f1-end)')" \
    "pflow export_pool zero_newest_root import_rewind_f pwait export_pool "
is "prewindf: f1 is asked with G far above, so that it waits, and lives 900 s (its premise's expiry)" \
    "$(sed -n '/^rec_prewindf() {$/,/^}$/p' "$SCRIPT" | grep -c '^    pflow f1 f1 --group-plus 1000000 --expiry-s 900 ')" 1
# The workflow runs every publish course by default (#141's review).
WF=${SCRIPT%/*}/../../.github/workflows/skyzfs-publish-rig.yml
is "the publish rig's default courses are the script's publish courses" \
    "$(sed -n 's/.*\${COURSES:-\([a-z ]*\)}.*/\1/p' "$WF")" \
    "$(sed -n 's/^PUBLISH_NAMED="\(.*\)"$/\1/p' "$SCRIPT")"
is "prewindf: f1's end is read with the pool synced between looks" \
    "$(sed -n '/^rec_prewindf() {$/,/^}$/p' "$SCRIPT" | grep -c '^    pwait f1-end f1 [0-9]* sync$')" 1
is "rewindf zeroes the root block and imports as prewindf does" \
    "$(order_of rec_rewindf '^    (export_pool|zero_newest_root|import_rewind_f|committed)')" \
    "export_pool zero_newest_root import_rewind_f committed export_pool "
ir_fn=$(sed -n '/^import_rewind_f() {$/,/^}$/p' "$SCRIPT")
is "import_rewind_f tries -F, then -FX, and says which imported" \
    "$(grep -cE '^    for opt in -F -FX; do$|^    echo "imported with \$how" >> "\$RDIR/rewind-f.txt"$' <<< "$ir_fn")" 2
# prewindc (#141's review, S1): prewindf's control, with nothing zeroed
# and a plain import; c1 is read after a minute of syncs, withdrawn and
# looked at.
is "prewindc: c1 pends, export, a plain import, then c1's end, withdrawal and look" \
    "$(order_of rec_prewindc '^    (pflow c1|export_pool|zero_newest_root|import_rewind_f|import_pool|pwait c1-end|pask c1-withdraw|pask c1-look)')" \
    "pflow export_pool import_pool pwait pask pask export_pool "
is "prewindc: c1 is asked as f1 is, and read with the pool synced between looks" \
    "$(sed -n '/^rec_prewindc() {$/,/^}$/p' "$SCRIPT" | grep -cE '^    pflow c1 c1 --group-plus 1000000 --expiry-s 900 |^    pwait c1-end c1 60 sync$|^    pask c1-withdraw withdraw c1$|^    pask c1-look look c1$')" 4
is "prewindc is a publish course by name" \
    "$(grep -c '^PUBLISH_NAMED=".* prewindc .*"$' "$SCRIPT")" 1
# Looks at names that must be absent: f1 once ended, tw once withdrawn
# and a cut past its G committed.
is "prewindf: f1 is looked at after its end" \
    "$(order_of rec_prewindf '^    (pwait f1-end|pask f1-look look f1$|load 1$)')" "pwait pask load "
is "pkill: tw is looked at after its status, before tk is asked" \
    "$(order_of rec_pkill '^    (ustatus tw-status.txt|pask tw-look look tw$|pflow tk)')" "ustatus pask pflow "
is "prewindf is a publish course by name" \
    "$(grep -c '^PUBLISH_NAMED=".* prewindf .*"$' "$SCRIPT")" 1

# look (M2.6a): at t1 after it was done, at t5 after it expired and at t3,
# never asked validly; at r1 with the store down, before it comes back.
is "pplain: looks at t1 once done, at t5 once expired, and at t3" \
    "$(order_of rec_pplain '^    (pwait t1-again-end|pask t1-look look t1$|pflow t5|pask t5-look look t5$|pask t3-look look t3$)')" \
    "pwait pask pflow pask pask "
is "prewind: looks at r1 with the store down, before it comes back" \
    "$(order_of rec_prewind '^    (bash -c "\$S3_DOWN"|pflow r1|pask r1-look look r1$|bash -c "\$S3_UP")')" \
    "bash pflow pask bash "

# passist (M2.T5's stage 0): an idle pool, a1 carried only by the assist,
# then a minute of assists for a2, which nothing carries, between two
# statuses and before the kstat is kept; then a2 withdrawn and looked at.
is "passist: idle, a1 assisted, idle, a2 assisted between statuses, the kstat kept, then a2 withdrawn and looked at" \
    "$(order_of rec_passist '^    (create_pool|load|pidle|pflow|ustatus|cat "/proc/spl/kstat/zfs/\$POOL/txgs"|pask|export_pool)')" \
    "create_pool load pidle pflow pidle ustatus pflow ustatus cat pask pask export_pool "
is "passist: a1 five groups ahead and a2 far ahead, both assisted" \
    "$(sed -n '/^rec_passist() {$/,/^}$/p' "$SCRIPT" | grep -cE '^    pflow a1 a1 --group-plus 5 --assist |^    pflow a2 a2 --group-plus 1000000 --expiry-s 900 --assist |^    pask a2-withdraw withdraw a2$|^    pask a2-look look a2$|^    cat "/proc/spl/kstat/zfs/\$POOL/txgs" > "\$RDIR/a2-txgs.txt"$')" 5
is "passist is a publish course by name" \
    "$(grep -cE '^PUBLISH_NAMED=".* passist( .*)?"$' "$SCRIPT")" 1

# pretire and pkillret (M2.7b-1): retire a published snapshot, and the
# daemon killed once a retire's transaction landed.
is "pretire and pkillret are publish courses by name" \
    "$(grep -cE '^PUBLISH_NAMED=".* pretire pkillret"$' "$SCRIPT")" 1
is "pkillret serves with the kill after the first retire" \
    "$(grep -c '^        pkillret) SERVE_ARGS=(--kill-at-upload retire:1) ;;$' "$SCRIPT")" 1
steps_of() { sed -n "/^$1() {\$/,/^}\$/p" "$SCRIPT" | grep -E "$2" | sed 's/^ *//; s/ *\\$//'; }
is "pretire: e1 and e2 published; e1 retired with e2's commit, looked at, progress, retired, looked at, progress of e1 and e2, listed, retired again; e9; e3 pending, retired, withdrawn" \
    "$(steps_of rec_pretire '^    (pflow|pask|nodeb)')" \
    'pflow e1 e1 --sync --limit-s 300
pflow e2 e2 --sync --limit-s 300
pask e1-other retire e1 "$(pcommit e2)"
pask e1-kept look e1
pask e1-progress progress e1
pask e1-retire retire e1 "$(pcommit e1)"
pask e1-look look e1
pask e1-progress-after progress e1
pask e2-progress progress e2
nodeb refs > "$RDIR/refs-after.txt" 2> "$RDIR/refs-after.err"
pask e1-again retire e1 "$(pcommit e1)"
pask e9 retire e9 "$(pcommit e1)"
pflow e3 e3 --group-plus 1000000 --expiry-s 900 --limit-s 3
pask e3-retire retire e3 "$(pcommit e1)"
pask e3-withdraw withdraw e3'
is "pkillret: k1 published, node B before, retired (the daemon dies), node B between, the take-over, retired again, looked at" \
    "$(steps_of rec_pkillret '^    (pflow|h=|out=|pend|serve_end|printf|nodeb|SERVE_ARGS|serve_start|pask)')" \
    'pflow k1 k1 --sync --limit-s 300
h=$(pcommit k1)
nodeb snap-row --snapshot k1 > "$RDIR/k1-before.txt" 2> "$RDIR/k1-before.err"
out=$(pagent ask retire k1 "$h" 2>> "$RDIR/agent.err") || rc=$?
pend k1-retire "${out:-none (exit $rc)}"
serve_end 5 || rig_error "the daemon'"'"'s pid went away and serve_end did not see it end"
printf '"'"'killed_rc\t%s\n'"'"' "$SERVE_RC" > "$RDIR/kill.tsv"
nodeb snap-row --snapshot k1 > "$RDIR/k1-between.txt" 2> "$RDIR/k1-between.err"
SERVE_ARGS=()
serve_start 2
pask k1-again retire k1 "$h"
pask k1-look look k1'
pc_fn=$(sed -n '/^pcommit() {$/,/^}$/p' "$SCRIPT")
is "pcommit is in the script" "$([ -n "$pc_fn" ] && echo yes)" yes
# shellcheck disable=SC2034  # RDIR is pcommit's, run through eval
pcommit_of() { ( # ends.tsv's text, the label
    RDIR=$(mktemp -d)
    printf '%b' "$1" > "$RDIR/ends.tsv"
    eval "$pc_fn"
    pcommit "$2"; rm -rf "${RDIR:?}"
) 2>&1; }
h1=$(printf 'a%.0s' $(seq 1 64)); h2=$(printf 'b%.0s' $(seq 1 64)); z=$(printf '0%.0s' $(seq 1 64))
is "pcommit: the commit of the label's done" "$(pcommit_of "e1\tdone commit=$h1\ne2\tdone commit=$h2\n" e2)" "$h2"
is "pcommit: the label's last line" "$(pcommit_of "e1\tdone commit=$h1\ne1\tdone commit=$h2\n" e1)" "$h2"
is "pcommit: an end that is not done is 64 zeros" "$(pcommit_of "e1\tpending after 3\n" e1)" "$z"
is "pcommit: no such label is 64 zeros" "$(pcommit_of "e1\tdone commit=$h1\n" e2)" "$z"
is "pcommit: a commit not of 64 hex digits is 64 zeros" "$(pcommit_of "e1\tdone commit=${h1}a\n" e1)" "$z"
is "pcommit: a label that ends like another is not it" "$(pcommit_of "xe1\tdone commit=$h1\n" e1)" "$z"
pidle_fn=$(sed -n '/^pidle() {$/,/^}$/p' "$SCRIPT")
is "pidle counts only the groups that wrote (ZFS 2.4 commits empty ones), and an empty kstat is the rig's error" \
    "$(grep -cE "\\\$3 == \"C\" .*\\\$6 \+ 0 > 0|rig_error \"no group that wrote in" <<< "$pidle_fn")" 2

# pidle's wait (#145's review, G3: plants S2 and S3), on a stand-in kstat
# and a stand-in sleep that moves the clock on at once. The clock is
# bash's SECONDS unset, so a plain variable that only the stand-in moves:
# set, SECONDS also counts the wall clock's seconds, and a second that
# turns during the run adds one (the next look's B2: 3 to 8 runs in 100
# answered off by one). A pidle that never sleeps would then loop for
# ever, so the run is a bash of its own under a limit. -> "idle after N s,
# K sleeps", "error after N s, K sleeps", "no end in 20 s" for the
# limit's 124, or "ended with N" for any other failure of the inner bash
# (#145's third look, N2: a pidle the sed did not find read as a
# time-out).
pidle_body='
    unset SECONDS
    mkdir -p "$T/kstat/p"
    printf "txg birth state ndirty nread nwritten reads writes\n40 1 C 0 0 4096 0 0\n41 1 C 0 0 0 0 0\n" \
        > "$T/kstat/p/txgs"
    g=42 slept=0
    sleep() {
        SECONDS=$((SECONDS + $1)) slept=$((slept + 1))
        if [ "$GROW" ]; then echo "$g 1 C 0 0 512 0 0" >> "$T/kstat/p/txgs"; g=$((g + 1)); fi
    }
    log() { :; }
    rig_error() { echo "error after $SECONDS s, $slept sleeps"; exit 0; }
    POOL=p NAME=t
    eval "${PIDLE_FN//\/proc\/spl\/kstat\/zfs/$T/kstat}"
    SECONDS=0
    pidle "$1" || exit $?
    echo "idle after $SECONDS s, $slept sleeps"'
pidle_run() { # LIMIT [grow]: with grow, a group that wrote at every sleep
    rm -rf "${T:?}/kstat"
    local rc=0
    T=$T PIDLE_FN=$pidle_fn GROW=${2:-} timeout 20 bash -c "$pidle_body" pidle_run "$1" || rc=$?
    case $rc in 0) ;; 124) echo "no end in 20 s" ;; *) echo "ended with $rc" ;; esac
}
is "pidle: a pool whose newest group that wrote stays put is idle after 12 s, not before" "$(pidle_run 120)" "idle after 12 s, 12 sleeps"
is "pidle: a pool that writes a group every second is the rig's error at its limit" "$(pidle_run 30 grow)" \
    "error after 30 s, 30 sleeps"
# The diagnosis itself (N2): a pidle that is not there ends 127, not 124.
is "pidle_run says how the inner bash ended" "$(pidle_fn='' pidle_run 120 2> /dev/null)" "ended with 127"

# Protocol 3 or 2 (M2.7b-1): pchannel learns the daemon's version from
# the greeting it answers `version` with, and sends the others in it.
ch_fn=$(sed -n '/^channel_hello() {$/,/^}$/p' "$SCRIPT")
is "channel_hello is in the script" "$([ -n "$ch_fn" ] && echo yes)" yes
ch_of() { ( # the greeting file's text, or "-" for no file
    eval "$ch_fn"
    d=$(mktemp -d)
    [ "$1" = - ] || printf '%b' "$1" > "$d/g.txt"
    channel_hello "$d/g.txt"; rm -rf "${d:?}"
) 2>&1; }
is "channel_hello: version 3" "$(ch_of 'skyzfs-control 3\n')" "skyzfs-control 3"
is "channel_hello: version 2" "$(ch_of 'skyzfs-control 2\n')" "skyzfs-control 2"
is "channel_hello: another version offers" "$(ch_of 'skyzfs-control 4\n')" "skyzfs-control ?"
is "channel_hello: a version 3 with more after it offers" "$(ch_of 'skyzfs-control 30\n')" "skyzfs-control ?"
is "channel_hello: an empty file offers" "$(ch_of '')" "skyzfs-control ?"
is "channel_hello: no file offers" "$(ch_of -)" "skyzfs-control ?"
is "channel_hello: only the first line is read" "$(ch_of 'skyzfs-control 9\nskyzfs-control 3\n')" "skyzfs-control ?"
praw_fn=$(sed -n '/^praw() {$/,/^}$/p' "$SCRIPT")
# shellcheck disable=SC2034,SC2317,SC2329  # the stubs and names are praw's, run through eval
praw_args() { ( # what praw hands publish_agent.py, with and without a greeting file
    HERE=/h; RUN=/r; RDIR=$(mktemp -d)
    python3() { shift; echo "args: $*" >&2; cat > /dev/null; }
    pend() { :; }; rig_error() { echo "error $*"; }
    eval "$praw_fn"
    { praw "$@" 2>&1; cat "$RDIR/agent.err"; } | sed "s|$RDIR|RDIR|g"; rm -rf "${RDIR:?}"
) }
is "praw: no greeting file, no fourth argument" "$(praw_args cuts 'x\n')" "args: /h/publish_agent.py raw /r RDIR/agent.tsv cuts"
is "praw: the greeting file is the fourth argument" "$(praw_args version 'x\n' /g/greeting.txt)" \
    "args: /h/publish_agent.py raw /r RDIR/agent.tsv version /g/greeting.txt"
is "pchannel learns the greeting from version, before the others" \
    "$(sed -n '/^rec_pchannel() {$/,/^}$/p' "$SCRIPT" | grep -E '^    (praw|PCHANNEL_HELLO=|ppeer)' | sed 's/ *$//')" \
    "    praw version 'skyzfs-control 9\\ncuts\\n' \"\$RDIR/channel-greeting.txt\"
    PCHANNEL_HELLO=\$(channel_hello \"\$RDIR/channel-greeting.txt\")
    praw malformed \"\$PCHANNEL_HELLO\\ncuts extra=1\\n\"
    praw name \"\$PCHANNEL_HELLO\\nprogress name=a/b\\n\"
    praw cuts \"\$PCHANNEL_HELLO\\ncuts\\n\"
    ppeer"
is "the peer sends in the learned greeting" \
    "$(sed -n '/^ppeer() {$/,/^}$/p' "$SCRIPT" | grep -c "printf '%s\\\\ncuts\\\\n' \"\$PCHANNEL_HELLO\"")" 1

[ "$fails" = 0 ] && echo "all passed"
exit $((fails > 0))

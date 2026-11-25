#!/usr/bin/env bash
# Recorded outputs of the ZFS commands the agent's ZFS adapter runs
# (skyzfs-agent, crates/skyzfs-agent/src/zfs/cli.rs and zfs/cli/*.lua),
# for its fixtures: the M2.6 plan's Q10. It judges nothing.
#
#   zfs-cli-record.sh OUT LUA_DIR [WORK]
#
# LUA_DIR holds snapshot.lua, members.lua and tagged.lua, the programs
# as the adapter pipes them to `zfs program`. Every command runs with
# LC_ALL=C, as the adapter runs its commands (src/host.rs), unless its
# record says otherwise. Each command is one record, OUT/NAME/ with:
#
#   cmd     the command line, quoted for bash (printf %q: a comma is
#           written "\,", a byte that is not UTF-8 as $'\377'), with
#           "< FILE" for stdin
#   argv    the same words, each ended by a NUL byte, unquoted
#   stdout  what it wrote to stdout
#   stderr  what it wrote to stderr. When the command was killed at the
#           limit, the shell's "Killed" line, which names this script,
#           is in it: that line is not ZFS's
#   rc      its exit code (124: it did not end within the limit)
#   secs    its wall time in seconds: the command under lim, with lim's
#           own steps (two mktemp, a fork, the copy of the outputs) and
#           its poll of 0.1 s; for a catch, the try that was kept
#   note    only where there is something to say: a state looked for
#           and whether it was caught, or an expected message and
#           whether it was seen
#
# and one line "NAME<TAB>rc<TAB>note<TAB>cmd" of OUT/index.tsv, in the
# order each record ended (a record run in the background, mirror-online
# or mirror-rebuild, can end before or after the records that watched
# it). NAME is
# "<pool>-<what>" with pool
#
#   rig     skzq: the data on /dev/ublkb0 of the reference ublk server,
#           with a log device
#   mirror  skzqm: a mirror of two files, with a special and a dedup
#           mirror, a cache device and a spare, so that each class's
#           header is in its status. Its files scan in seconds, so its
#           scrubs, resilver and rebuild run with a delay of 10 ms
#           injected on every I/O of m0, and of m1 as well during the
#           rebuild (zinject -D 10:1, from the package zfs-test;
#           about.txt says whether it took, zinject.txt why not, and
#           zinject-rebuild.txt what was injected for the rebuild): the
#           state's text is the same, it lasts long enough to be seen
#   remove  skzqr: two files, one of them removed (the removal's states)
#   none    skznone, a pool that does not exist
#   scale-N skzqs, made only when ZCR_SCALE names N: N file systems
#           under the root (made with -u, not mounted; the root is),
#           and on it, each with its wall time in secs:
#             snapshot          the snapshot program at @t
#             snapshot-cli      zfs snapshot -r P@t, only if the program
#                               did not take the whole @t (as above);
#                               about.txt then says how many it took
#             members, hold, tagged, release
#                               at @t, hold and release with -r
#             destroy           zfs destroy -r P@t
#             take-cli          zfs snapshot -r P@c, the take by the CLI
#             take-cli-members  members.lua at @c
#           about.txt gives the members that members and
#           take-cli-members counted, which is what hold, tagged,
#           release and destroy ran on.
#           The snapshot program is not run at a count above
#           ZCR_SCALE_LUA_MAX, when set; snapshot-cli takes @t there.
#           Beside the two takes (snapshot, take-cli), from 2 s before
#           the take to 2 s after (writing, below): a writer of 4 KiB
#           O_DSYNC writes, a writer of new data past the pool's dirty
#           data limit, and the pool's transaction groups and timed
#           `zpool sync` runs. The take's record gets their logs
#           (writer.tsv, fill.tsv, txg.tsv), a line for each (stall,
#           fill, txg) and the window they were read on (window); about.txt
#           the same lines, and the dirty data limit and txg history.
#           The takes' secs are under that load, and so are the lines:
#           txg's before_sync and before_sync_longest are the load's own
#           before the take. A bounded zpool sync after each take
#           syncs what the writer of new data left. At most
#           three counts, each at most 100000 and each once. Each
#           command's limit is ZCR_SCALE_LIMIT seconds (default 1800),
#           or what is left of ZCR_SCALE_BUDGET (default 4800 s, for all
#           counts together) if less; once the budget is spent no
#           step is started, and about.txt says where it stopped.
#           Steps that are not counts' commands (the pool's create and
#           destroy, the writers' margins and stops) can run past it by
#           some minutes. about.txt also has df
#           of WORK before each pool and how long the file systems took
#           to make
#
# The adapter's tag is skyzfs-publish (HOLD_TAG, src/zfs.rs) and its
# property skyzfs:assist (ASSIST_PROPERTY). Each live pool gets a child
# file system, a volume, skyzfs:assist set on its root and one user
# property set locally on the child (skyzfs:note), before anything is
# recorded on it. In order, on rig and mirror:
#
#   status, status-g       zpool status -P [-g], idle
#   get                    zpool get -H -p -o property,value guid,
#                          load_guid,multihost,readonly,checkpoint
#   assist-set, assist-sync
#                          the assist: zfs set skyzfs:assist=V POOL, then
#                          zpool sync POOL
#   snapshot-1             snapshot.lua at @t
#   snapshot-cli           only if snapshot-1 did not take the whole
#                          @t: zfs snapshot -r, so that what follows has
#                          its @t (cli_snapshot: where the program failed
#                          part way, what it took is destroyed first,
#                          snapshot-cli-destroy-partial)
#   snapshot-2             snapshot.lua at @t again: EEXIST (expected)
#   members, members-absent, tagged
#                          members.lua at t and at a t that is absent;
#                          tagged.lua before any hold
#   hold-1, holds, holds-r, hold-2, tagged-held, release-1,
#   holds-r-released, release-2
#                          the holds, and hold and release twice for the
#                          second's message (expected)
#   holds-absent, hold-absent, release-absent
#                          on a snapshot that is not there, on a live pool
#   hold-root, hold-r-partial, holds-r-partial
#                          a hold on the root's @t alone, then `hold -r`,
#                          then what holds it: whether the children get
#                          the tag when the root has it already
#   hold-child, release-r-partial, holds-r-after-partial
#                          a hold on the child's @t alone, then
#                          `release -r` (the root lacks the tag), then
#                          what holds it. Holds left are released, with
#                          no record
#   set-utf8, set-raw, get-odd
#                          two user properties set locally on the child,
#                          one with a value in UTF-8 beyond ASCII and one
#                          whose value is not UTF-8, and `zfs get` of them
#   snapshot-odd, snapshot-odd-utf8
#                          snapshot.lua with those properties, at @o0 with
#                          LC_ALL=C as the adapter, and at @o2 with
#                          LC_ALL=C.UTF-8
#   exists-child-pre, snapshot-exists-child, exists-child-after
#                          the child has @p and the root has not, then
#                          snapshot.lua at @p, which refuses at its check
#                          before taking anything (EEXIST, expected), then
#                          the snapshots that are there
#   destroy, destroy-after, destroy-absent
#                          what the adapter runs when it cannot read the
#                          snapshot program's output: zfs destroy -r P@T,
#                          on @o0 (the program took it), then the
#                          snapshots there, then on a snapshot that is
#                          not there ("could not find any snapshots to
#                          destroy", expected)
#   destroy-held-hold, destroy-held, destroy-held-after
#                          the same on @o2 with a hold on the child's
#                          @o2 alone, then the snapshots there
#   checkpoint, get-checkpoint, checkpoint-discard
#                          the same zpool get with a checkpoint
#   scrub, scrub-status, scrub-status-g
#                          during a scrub, if caught (the note says)
#   scrub-again, scrub-pause, scrub-paused, scrub-paused-g, scrub-stop,
#   scrub-canceled, scrub-canceled-g
#                          zpool status -P [-g] after `zpool scrub -p`,
#                          and after `zpool scrub -s`
#
# then, since they change the pools:
#
#   rig-attach, rig-resilver-status[-g], rig-resilvered[-g]
#                          /dev/ublkb1 attached to the rig's device
#   mirror-offline, mirror-online, mirror-resilver-status[-g],
#   mirror-resilvered[-g]  the mirror's second file brought back online
#                          after writes
#   mirror-rebuild, mirror-rebuild-status[-g], mirror-rebuilt[-g]
#                          a third file attached with -s (a sequential
#                          rebuild). Its status in progress reads
#                          "resilver (mirror-0) in progress". Until
#                          run 45665823917 it was not caught: the delay
#                          was on m0 alone, the rebuild read m1 and was
#                          over in seconds, and `zpool status` returned
#                          only when the attach did. Both sides are now
#                          slowed for it
#
# `zpool online` and `zpool attach -s` run in the background (with a
# limit of ZCR_CHANGE_LIMIT seconds, default 900) while the status is
# watched, so that a command that waits for its scan cannot hide the
# scan. In the runs so far `online` returned seconds after its resilver
# began, and `attach -s` only after its rebuild had ended; whether
# `attach -s` waits for the rebuild is not known.
#   remove-status, remove-status-g, remove, remove-in-progress[-g],
#   remove-done[-g]        skzqr's second file removed
#
# and once: `nocacheflush` (the module parameter), `version` and
# `locale`. On the pool that does not exist, every command the adapter
# runs: status, status-g, get, assist-set, assist-sync, the three
# programs, holds, holds-r, hold, release, destroy.
#
# Not recorded: the snapshot program's output that cannot be read (the
# case that makes the adapter run destroy -r), which cannot be made to
# happen from outside, and a checkpoint whose size reads 0.
#
# Meant for a throwaway machine. Exit 0 once set up, whatever the
# commands answered; 2 when it could not set up.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=tests/lib/gnu-dd.sh
. "$HERE/../lib/gnu-dd.sh"
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"

# Every limit is generous: the pools are healthy, and a command that
# does not end is recorded as 124 rather than holding the run.
LIMIT=${ZCR_LIMIT:-120}
# The program's limits, as the adapter passes them.
PROGRAM_LIMITS=(-t 100000000 -m 104857600)
# The adapter's hold tag and assist property.
TAG=skyzfs-publish
ASSIST=skyzfs:assist
# A sequential rebuild under way, and not the scrub that follows it
# ("scan: resilvered (mirror-0) ..." over "scrub in progress").
REBUILD_PATTERN='resilver \([^)]*\) in progress'

# The adapter's locale (src/host.rs): every command with LC_ALL=C.
adapter_env() {
    export LC_ALL=C
}

# secs_between T0 T1: T1 - T0 in seconds, to the millisecond.
secs_between() {
    awk -v a="$1" -v b="$2" 'BEGIN { printf "%.3f\n", b - a }'
}

# rec NAME [--in FILE] [--expect PATTERN] COMMAND...: run COMMAND once
# and keep it as the record NAME. With --expect, the note says whether
# its stdout or stderr matched PATTERN (grep -E).
rec() {
    local name=$1 in=/dev/null expect="" shown d rc=0 note="" t0 t1
    shift
    while :; do
        case ${1:-} in
            --in) in=$2; shift 2 ;;
            --expect) expect=$2; shift 2 ;;
            *) break ;;
        esac
    done
    d=$OUT/$name
    if [ -e "$d" ]; then
        echo "zfs-cli-record: the record $name twice" >&2
        return 70
    fi
    mkdir -p "$d"
    shown=$(printf '%q ' "$@")
    shown=${shown% }
    [ "$in" = /dev/null ] || shown="$shown < $(basename "$in")"
    printf '%s\n' "$shown" > "$d/cmd"
    printf '%s\0' "$@" > "$d/argv"
    t0=$(date +%s.%N)
    lim "$LIMIT" "$@" < "$in" > "$d/stdout" 2> "$d/stderr" || rc=$?
    t1=$(date +%s.%N)
    echo "$rc" > "$d/rc"
    secs_between "$t0" "$t1" > "$d/secs"
    if [ -n "$expect" ]; then
        if cat "$d/stdout" "$d/stderr" | grep -Eq -- "$expect"; then
            note="expected /$expect/: seen"
        else
            note="expected /$expect/: NOT SEEN"
        fi
        printf '%s\n' "$note" > "$d/note"
    fi
    printf '%s\t%s\t%s\t%s\n' "$name" "$rc" "$note" "$shown" >> "$OUT/index.tsv"
    printf '== %s: rc %s%s\n' "$name" "$rc" "${note:+ ($note)}"
}

# catch NAME PATTERN TRIES COMMAND...: run COMMAND up to TRIES times,
# 0.1 s apart, and keep the first output whose stdout matches PATTERN
# (grep -E) as the record NAME. If none does, the last is kept, with a
# note that says so; a try that did not end within the limit is the
# last. Returns 0 when caught and kept.
catch() {
    local name=$1 pattern=$2 tries=$3 i tmp rc caught="" note t0 t1
    shift 3
    tmp=$OUT/.catch
    for ((i = 1; i <= tries; i++)); do
        rm -rf "${tmp:?}"
        mkdir -p "$tmp"
        rc=0
        t0=$(date +%s.%N)
        lim "$LIMIT" "$@" < /dev/null > "$tmp/stdout" 2> "$tmp/stderr" || rc=$?
        t1=$(date +%s.%N)
        if grep -Eq -- "$pattern" "$tmp/stdout"; then
            caught=1
            break
        fi
        [ "$rc" = 124 ] && break
        [ "$i" -lt "$tries" ] && sleep 0.1
    done
    if [ -n "$caught" ]; then
        note="caught at try $i"
    elif [ "$rc" = 124 ]; then
        note="not caught: try $i did not end within ${LIMIT} s; this is it"
    else
        note="not caught: no output of $tries matched /$pattern/; this is the last"
    fi
    keep "$name" "$tmp" "$rc" "$note" "$(secs_between "$t0" "$t1")" "$@" || { rm -rf "${tmp:?}"; return 70; }
    [ -n "$caught" ]
}

# keep NAME DIR RC NOTE SECS COMMAND...: DIR's stdout and stderr as the
# record NAME.
keep() {
    local name=$1 tmp=$2 rc=$3 note=$4 secs=$5 d shown
    shift 5
    d=$OUT/$name
    if [ -e "$d" ]; then
        echo "zfs-cli-record: the record $name twice" >&2
        return 70
    fi
    mv "$tmp" "$d"
    shown=$(printf '%q ' "$@")
    shown=${shown% }
    printf '%s\n' "$shown" > "$d/cmd"
    printf '%s\0' "$@" > "$d/argv"
    echo "$rc" > "$d/rc"
    printf '%s\n' "$secs" > "$d/secs"
    printf '%s\n' "$note" > "$d/note"
    printf '%s\t%s\t%s\t%s\n' "$name" "$rc" "$note" "$shown" >> "$OUT/index.tsv"
    printf '== %s: rc %s (%s)\n' "$name" "$rc" "$note"
}

# program NAME P LUA [--locale L] [--expect PATTERN] [-n] ARGS...: zfs
# program -j as the adapter runs it, with LC_ALL=L when L is given.
program() {
    local name=$1 p=$2 lua=$3 ro=() env=() expect=()
    shift 3
    while :; do
        case ${1:-} in
            --locale) env=(env "LC_ALL=$2"); shift 2 ;;
            --expect) expect=(--expect "$2"); shift 2 ;;
            *) break ;;
        esac
    done
    if [ "${1:-}" = -n ]; then
        ro=(-n)
        shift
    fi
    rec "$name" --in "$LUA_DIR/$lua" "${expect[@]}" "${env[@]}" zfs program "${ro[@]}" -j "${PROGRAM_LIMITS[@]}" "$p" - "$@"
}

# partial RECORD: the snapshot program's RECORD says it failed ("failed"
# in its output), so it may have taken some members and not all.
partial() { [ -s "$OUT/$1/stdout" ] && grep -q '"failed"' "$OUT/$1/stdout"; }

# cli_snapshot NAME RECORD POOL: POOL@t by zfs snapshot -r (the record
# NAME) when the program's RECORD did not take all of it: where it
# failed, what it took is destroyed first (NAME-destroy-partial) and
# about.txt says how many it had taken; where it did not fail, only
# when POOL has no @t.
cli_snapshot() {
    local name=$1 r=$2 p=$3 taken
    if partial "$r"; then
        taken=$(grep -o '"taken": *"[0-9]*"' "$OUT/$r/stdout" | grep -o '[0-9][0-9]*' | head -n 1)
        echo "$name: $r failed after taking ${taken:-an unknown number of} snapshots: destroyed, and zfs snapshot -r takes @t" >> "$OUT/about.txt"
        rec "$name-destroy-partial" zfs destroy -r "$p@t"
    elif lim 120 zfs list -H -o name "$p@t" > /dev/null 2>&1; then
        return 0
    fi
    rec "$name" zfs snapshot -r "$p@t"
}

# members_line RECORD: how many members the members program's RECORD
# listed, in about.txt.
members_line() {
    local k
    k=$(python3 -I -c 'import json, sys
try:
    print(len(json.load(open(sys.argv[1]))["return"]["members"]))
except Exception:
    print("not read")' "$OUT/$1/stdout" 2> /dev/null)
    echo "scale: $1: ${k:-not read} members" >> "$OUT/about.txt"
}

# status NAME POOL: zpool status -P and -P -g, the adapter's pair.
status() {
    rec "$1" zpool status -P "$2"
    rec "$1-g" zpool status -P -g "$2"
}

# The adapter's commands on pool P that need no snapshot, records named
# PREFIX-...
agent_commands() { # prefix pool
    local x=$1 p=$2
    status "$x-status" "$p"
    rec "$x-get" zpool get -H -p -o property,value guid,load_guid,multihost,readonly,checkpoint "$p"
    rec "$x-assist-set" zfs set "$ASSIST=1700000000001" "$p"
    rec "$x-assist-sync" zpool sync "$p"
    program "$x-snapshot-1" "$p" snapshot.lua "$p" t "$ASSIST"
}

# The mirror's scans slowed, or as they are: DELAY names a vdev of the
# pool DELAY_POOL to inject a delay on, unless the vdevs are given.
DELAY_POOL="" DELAY="" ZINJECT=""
slow_on() { # pool [vdev...]
    [ "$1" = "$DELAY_POOL" ] && [ -n "$DELAY" ] && [ -n "$ZINJECT" ] || return 0
    local p=$1 v
    shift
    for v in "${@:-$DELAY}"; do
        lim 30 "$ZINJECT" -d "$v" -D 10:1 "$p" > /dev/null 2>&1
    done
}
slow_off() { # pool
    [ "$1" = "$DELAY_POOL" ] && [ -n "$DELAY" ] && [ -n "$ZINJECT" ] || return 0
    lim 30 "$ZINJECT" -c all > /dev/null 2>&1
}

# Release every hold of TAG under P@T that is left, with no record.
release_quietly() { # pool t
    local s
    zfs list -H -o name -t snapshot -r "$1" 2> /dev/null | grep "@$2\$" | while read -r s; do
        zfs release "$TAG" "$s" > /dev/null 2>&1
    done
}

# Everything recorded on a live pool before the changes of its devices.
record_pool() { # prefix pool
    local x=$1 p=$2
    agent_commands "$x" "$p"
    # Without the program, a snapshot for what follows all the same.
    cli_snapshot "$x-snapshot-cli" "$x-snapshot-1" "$p"
    program "$x-snapshot-2" "$p" snapshot.lua --expect '"errno": *"17"' "$p" t "$ASSIST"
    program "$x-members" "$p" members.lua -n "$p" t
    program "$x-members-absent" "$p" members.lua --expect '"absent"' -n "$p" absent
    program "$x-tagged" "$p" tagged.lua -n "$p" "$TAG"
    rec "$x-hold-1" zfs hold -r "$TAG" "$p@t"
    rec "$x-holds" zfs holds -H "$p@t"
    rec "$x-holds-r" zfs holds -H -r "$p@t"
    rec "$x-hold-2" --expect 'tag already exists' zfs hold -r "$TAG" "$p@t"
    program "$x-tagged-held" "$p" tagged.lua -n "$p" "$TAG"
    rec "$x-release-1" zfs release -r "$TAG" "$p@t"
    rec "$x-holds-r-released" zfs holds -H -r "$p@t"
    rec "$x-release-2" --expect 'no such tag' zfs release -r "$TAG" "$p@t"
    rec "$x-holds-absent" zfs holds -H "$p@absent"
    rec "$x-hold-absent" zfs hold -r "$TAG" "$p@absent"
    rec "$x-release-absent" zfs release -r "$TAG" "$p@absent"
    # A hold that some members have and some have not.
    rec "$x-hold-root" zfs hold "$TAG" "$p@t"
    rec "$x-hold-r-partial" zfs hold -r "$TAG" "$p@t"
    rec "$x-holds-r-partial" zfs holds -H -r "$p@t"
    release_quietly "$p" t
    rec "$x-hold-child" zfs hold "$TAG" "$p/child@t"
    rec "$x-release-r-partial" zfs release -r "$TAG" "$p@t"
    rec "$x-holds-r-after-partial" zfs holds -H -r "$p@t"
    release_quietly "$p" t
    # Values beyond ASCII, as the snapshot program reads them.
    rec "$x-set-utf8" zfs set "skyzfs:utf8=naïve, 日本, ∅" "$p/child"
    rec "$x-set-raw" zfs set "skyzfs:raw=a$(printf '\xff\xfe')b$(printf '\xc3')" "$p/child"
    rec "$x-get-odd" zfs get -H -p -o name,property,value,source skyzfs:utf8,skyzfs:raw "$p/child"
    program "$x-snapshot-odd" "$p" snapshot.lua "$p" o0 "$ASSIST"
    program "$x-snapshot-odd-utf8" "$p" snapshot.lua --locale C.UTF-8 "$p" o2 "$ASSIST"
    # The check refuses before anything is taken: the child has @p.
    rec "$x-exists-child-pre" zfs snapshot "$p/child@p"
    program "$x-snapshot-exists-child" "$p" snapshot.lua --expect '"errno": *"17"' "$p" p "$ASSIST"
    rec "$x-exists-child-after" zfs list -H -t snapshot -o name -r "$p"
    # The adapter's destroy -r of a take it cannot read.
    rec "$x-destroy" zfs destroy -r "$p@o0"
    rec "$x-destroy-after" zfs list -H -t snapshot -o name -r "$p"
    rec "$x-destroy-absent" --expect 'could not find any snapshots to destroy' zfs destroy -r "$p@absent"
    rec "$x-destroy-held-hold" zfs hold "$TAG" "$p/child@o2"
    rec "$x-destroy-held" zfs destroy -r "$p@o2"
    rec "$x-destroy-held-after" zfs list -H -t snapshot -o name -r "$p"
    release_quietly "$p" o2
    rec "$x-checkpoint" zpool checkpoint "$p"
    rec "$x-get-checkpoint" zpool get -H -p -o property,value guid,load_guid,multihost,readonly,checkpoint "$p"
    rec "$x-checkpoint-discard" zpool checkpoint -d -w "$p"
    slow_on "$p"
    rec "$x-scrub" zpool scrub "$p"
    catch_status "$x-scrub-status" 'scrub in progress' "$p"
    lim 600 zpool wait -t scrub "$p" > /dev/null 2>&1
    rec "$x-scrub-again" zpool scrub "$p"
    rec "$x-scrub-pause" zpool scrub -p "$p"
    status "$x-scrub-paused" "$p"
    rec "$x-scrub-stop" zpool scrub -s "$p"
    status "$x-scrub-canceled" "$p"
    slow_off "$p"
}

# The pool that does not exist: every command the adapter runs.
record_none() { # prefix pool
    local x=$1 p=$2
    agent_commands "$x" "$p"
    program "$x-members" "$p" members.lua -n "$p" t
    program "$x-tagged" "$p" tagged.lua -n "$p" "$TAG"
    rec "$x-holds" zfs holds -H "$p@t"
    rec "$x-holds-r" zfs holds -H -r "$p@t"
    rec "$x-hold" zfs hold -r "$TAG" "$p@t"
    rec "$x-release" zfs release -r "$TAG" "$p@t"
    rec "$x-destroy" zfs destroy -r "$p@t"
}

# scale_all COUNT...: scale for each count, at most three, each at most
# 100000 and each once, all within ZCR_SCALE_BUDGET seconds.
SCALE_END=0
scale_all() {
    local n seen=" " k=0
    SCALE_END=$(($(date +%s) + ${ZCR_SCALE_BUDGET:-4800}))
    ZCR_SCALE_LUA_MAX=${ZCR_SCALE_LUA_MAX:-}
    case $ZCR_SCALE_LUA_MAX in
        *[!0-9]* | 0?*)
            echo "scale: ZCR_SCALE_LUA_MAX is not a count: $ZCR_SCALE_LUA_MAX; the program runs at every count" >> "$OUT/about.txt"
            ZCR_SCALE_LUA_MAX=""
            ;;
    esac
    [ "${#ZCR_SCALE_LUA_MAX}" -gt 6 ] && ZCR_SCALE_LUA_MAX=999999
    for n in "$@"; do
        case $n in *[!0-9]* | 0*) echo "scale: not a count: $n" >> "$OUT/about.txt"; continue ;; esac
        if [ "${#n}" -gt 6 ] || [ "$n" -gt 100000 ]; then
            echo "scale: $n is more than 100000; not run" >> "$OUT/about.txt"
            continue
        fi
        case $seen in *" $n "*) echo "scale: $n twice; run once" >> "$OUT/about.txt"; continue ;; esac
        if [ "$k" -ge 3 ]; then
            echo "scale: more than three counts; $n not run" >> "$OUT/about.txt"
            continue
        fi
        seen="$seen$n "
        k=$((k + 1))
        scale "$n"
    done
}

# scale_limit: LIMIT for scale's next command, ZCR_SCALE_LIMIT or what
# is left of the budget if less. Fails when nothing is left.
scale_limit() {
    local left=$((SCALE_END - $(date +%s)))
    [ "$left" -gt 0 ] || return 1
    LIMIT=${ZCR_SCALE_LIMIT:-1800}
    [ "$left" -lt "$LIMIT" ] && LIMIT=$left
    return 0
}

# stop_bg PID: SIGTERM, then up to ZCR_STOP_TENTHS tenths of a second
# (600) for PID to end; BG_RC is its exit status. Fails, after a SIGKILL
# that is not waited for (it may be in the kernel's wait), when it did
# not end; BG_RC is then empty.
BG_RC=""
stop_bg() {
    local i
    BG_RC=""
    kill -TERM "$1" 2> /dev/null
    for ((i = 0; i < ${ZCR_STOP_TENTHS:-600}; i++)); do
        kill -0 "$1" 2> /dev/null || break
        sleep 0.1
    done
    if kill -0 "$1" 2> /dev/null; then
        kill -9 "$1" 2> /dev/null
        return 1
    fi
    wait "$1"
    BG_RC=$?
    return 0
}

# writing NAME DIR POOL COMMAND...: the record NAME, made by COMMAND (rec
# or program), with three things beside it, from ZCR_WRITER_MARGIN
# seconds (default 2) before COMMAND to as long after:
#   a writer of 4 KiB O_DSYNC writes to DIR/writer.dat (dsync_writer.py
#     write): the record's writer.tsv, and stall, its longest write that
#     overlapped COMMAND;
#   a writer of new data, 1 MiB buffered writes over FILL_SPAN bytes of
#     DIR/fill.dat (dsync_writer.py fill), only when FILL_SPAN is set:
#     fill.tsv and fill, the same line for it (writes are MiB);
#   POOL's transaction groups, read once a second from its txgs kstat
#     under ZCR_KSTAT_DIR (/proc/spl/kstat/zfs), and `zpool sync POOL`
#     every 2 s, timed (txg_probe.py probe): txg.tsv and txg, the
#     groups committed in COMMAND's window, the longest sync, the
#     longest time between two commits, and the syncs' longest.
# about.txt gets the three lines. With DIR empty, COMMAND alone.
writing() {
    local name=$1 dir=$2 pool=$3 margin=${ZCR_WRITER_MARGIN:-2} wpid fpid="" qpid t0 t1 log line fline pline s0 rc
    shift 3
    if [ -z "$dir" ]; then
        "$@"
        return
    fi
    log=$OUT/.writer-$name
    # Their stdout is not the script's: one left behind after a SIGKILL
    # must not hold the caller's pipe.
    python3 "$HERE/dsync_writer.py" write "$dir/writer.dat" "$log.tsv" > /dev/null 2> "$log.err" &
    wpid=$!
    if [ -n "${FILL_SPAN:-}" ]; then
        python3 "$HERE/dsync_writer.py" fill "$dir/fill.dat" "$log.fill.tsv" "$FILL_SPAN" > /dev/null 2> "$log.fill.err" &
        fpid=$!
    fi
    python3 "$HERE/txg_probe.py" probe "${ZCR_KSTAT_DIR:-/proc/spl/kstat/zfs}/$pool/txgs" "$pool" "$log.txg.tsv" > /dev/null 2> "$log.txg.err" &
    qpid=$!
    sleep "$margin"
    t0=$(date +%s.%N)
    "$@"
    t1=$(date +%s.%N)
    sleep "$margin"
    # Not in a command substitution: stop_bg waits for its child.
    stopped "$wpid" "the writer" dsync_writer.py stall "$log.tsv" "$t0" "$t1"
    line=$STOPPED
    fline="no writer of new data: zfs_dirty_data_max was not read"
    if [ -n "$fpid" ]; then
        stopped "$fpid" "the writer of new data" dsync_writer.py stall "$log.fill.tsv" "$t0" "$t1"
        fline=$STOPPED
    fi
    stopped "$qpid" "the probe" txg_probe.py report "$log.txg.tsv" "$t0" "$t1"
    pline=$STOPPED
    # What the writer of new data left is synced here, not under the
    # next record.
    if [ -n "$fpid" ]; then
        s0=$(date +%s.%N)
        rc=0
        lim 120 zpool sync "$pool" > /dev/null 2>&1 || rc=$?
        fline="$fline settled=$(secs_between "$s0" "$(date +%s.%N)") settle_rc=$rc"
    fi
    mkdir -p "$OUT/$name"
    mv "$log.tsv" "$OUT/$name/writer.tsv" 2> /dev/null
    mv "$log.err" "$OUT/$name/writer.err" 2> /dev/null
    mv "$log.fill.tsv" "$OUT/$name/fill.tsv" 2> /dev/null
    mv "$log.fill.err" "$OUT/$name/fill.err" 2> /dev/null
    mv "$log.txg.tsv" "$OUT/$name/txg.tsv" 2> /dev/null
    mv "$log.txg.err" "$OUT/$name/txg.err" 2> /dev/null
    printf '%s\n' "$line" > "$OUT/$name/stall"
    printf '%s\n' "$fline" > "$OUT/$name/fill"
    printf '%s\n' "$pline" > "$OUT/$name/txg"
    printf '%s\t%s\n' "$t0" "$t1" > "$OUT/$name/window"
    {
        echo "scale: writer beside $name: $line"
        echo "scale: new data beside $name: $fline"
        echo "scale: txgs beside $name: $pline"
    } >> "$OUT/about.txt"
}

# stopped PID WHAT SCRIPT COMMAND LOG T0 T1: stop_bg PID, then STOPPED
# is SCRIPT's line on LOG for the window, with the exit status where it
# is not 0 (" exit=N"): a process that died reads as such, not as a
# quiet window.
STOPPED=""
stopped() {
    local pid=$1 what=$2 script=$3
    shift 3
    if ! stop_bg "$pid"; then
        STOPPED="$what did not stop within $((${ZCR_STOP_TENTHS:-600} / 10)) s"
        return
    fi
    STOPPED=$(python3 "$HERE/$script" "$@" 2>&1)
    [ "$BG_RC" = 0 ] || STOPPED="$STOPPED exit=$BG_RC"
}

# fill_span N: FILL_SPAN, twice zfs_dirty_data_max (under ZCR_ZFS_PARAMS,
# /sys/module/zfs/parameters) and at most 4 GiB, or empty when it cannot
# be read; and the txg history on, as about.txt says.
fill_span() {
    local n=$1 params=${ZCR_ZFS_PARAMS:-/sys/module/zfs/parameters} dirty hist
    FILL_SPAN=""
    dirty=$(cat "$params/zfs_dirty_data_max" 2> /dev/null)
    case $dirty in
        '' | *[!0-9]*) echo "scale $n: zfs_dirty_data_max not read; no writer of new data" >> "$OUT/about.txt" ;;
        *)
            FILL_SPAN=$((dirty * 2))
            [ "$FILL_SPAN" -gt 4294967296 ] && FILL_SPAN=4294967296
            echo "scale $n: zfs_dirty_data_max $dirty; the writer of new data writes over $FILL_SPAN bytes" >> "$OUT/about.txt"
            ;;
    esac
    hist=$(cat "$params/zfs_txg_history" 2> /dev/null)
    if [ "$hist" = 0 ]; then
        echo 100 > "$params/zfs_txg_history" 2> /dev/null
        hist="0, set to $(cat "$params/zfs_txg_history" 2> /dev/null)"
    fi
    echo "scale $n: zfs_txg_history ${hist:-not read}" >> "$OUT/about.txt"
}

# scale N: a pool with N file systems under its root, and the adapter's
# programs and holds on it, each timed (the secs of its record).
scale() {
    local n=$1 p=skzqs t0 made step cut="" rc wdir
    local LIMIT FILL_SPAN=""
    if ! scale_limit; then
        echo "scale $n: not run, the budget was spent" >> "$OUT/about.txt"
        return 0
    fi
    df -h "$WORK" 2>&1 | sed "s/^/scale $n: df: /" >> "$OUT/about.txt"
    truncate -s 16G "$WORK/scale.img"
    if ! lim 180 zpool create -f -o ashift=12 -o cachefile=none -O mountpoint="$WORK/mnt/$p" \
        -O compression=off "$p" "$WORK/scale.img" > "$OUT/create-$p-$n.log" 2>&1; then
        echo "scale $n: zpool create failed (create-$p-$n.log)" >> "$OUT/about.txt"
        rm -f "$WORK/scale.img"
        return 0
    fi
    zfs set "$ASSIST=1700000000000" "$p" >> "$OUT/create-$p-$n.log" 2>&1
    # The writer's file is in the root, and only if the root is mounted.
    wdir=""
    if mountpoint -q "$WORK/mnt/$p"; then
        wdir=$WORK/mnt/$p
        fill_span "$n"
    else
        echo "scale $n: the root is not mounted; no writer" >> "$OUT/about.txt"
    fi
    t0=$(date +%s)
    if scale_limit; then
        rc=0
        seq 1 "$n" | lim "$LIMIT" xargs -P 8 -I{} zfs create -u "$p/d{}" >> "$OUT/create-$p-$n.log" 2>&1 || rc=$?
        [ "$rc" = 124 ] && cut=" (stopped at the limit of $LIMIT s)"
        made=$(lim 120 zfs list -H -o name -r "$p" 2> /dev/null | wc -l)
        echo "scale $n: $made datasets (the root among them) made in $(($(date +%s) - t0)) s$cut" >> "$OUT/about.txt"
    fi
    for step in snapshot snapshot-cli members hold tagged release destroy take-cli take-cli-members; do
        if ! scale_limit; then
            echo "scale $n: the budget was spent before $step; it and the rest were not run" >> "$OUT/about.txt"
            break
        fi
        case $step in
            snapshot)
                if [ -n "${ZCR_SCALE_LUA_MAX:-}" ] && [ "$n" -gt "$ZCR_SCALE_LUA_MAX" ]; then
                    echo "scale $n: the snapshot program not run: $n is more than ZCR_SCALE_LUA_MAX ($ZCR_SCALE_LUA_MAX)" >> "$OUT/about.txt"
                else
                    writing "scale-$n-snapshot" "$wdir" "$p" program "scale-$n-snapshot" "$p" snapshot.lua "$p" t "$ASSIST"
                fi
                ;;
            # Without the program's whole @t, the CLI's, so that the rest
            # has one.
            snapshot-cli) cli_snapshot "scale-$n-snapshot-cli" "scale-$n-snapshot" "$p" ;;
            members)
                program "scale-$n-members" "$p" members.lua -n "$p" t
                members_line "scale-$n-members"
                ;;
            hold) rec "scale-$n-hold" zfs hold -r "$TAG" "$p@t" ;;
            tagged) program "scale-$n-tagged" "$p" tagged.lua -n "$p" "$TAG" ;;
            release) rec "scale-$n-release" zfs release -r "$TAG" "$p@t" ;;
            destroy) rec "scale-$n-destroy" zfs destroy -r "$p@t" ;;
            take-cli) writing "scale-$n-take-cli" "$wdir" "$p" rec "scale-$n-take-cli" zfs snapshot -r "$p@c" ;;
            take-cli-members)
                program "scale-$n-take-cli-members" "$p" members.lua -n "$p" c
                members_line "scale-$n-take-cli-members"
                ;;
        esac
    done
    lim 600 zpool destroy -f "$p" > /dev/null 2>&1
    rm -f "$WORK/scale.img"
}

# catch_status NAME PATTERN POOL: the pair during a state, if caught.
catch_status() {
    catch "$1" "$2" 100 zpool status -P "$3"
    catch "$1-g" "$2" 100 zpool status -P -g "$3"
}

# during NAME STATUS PATTERN POOL COMMAND...: COMMAND as the record NAME,
# in the background with a limit of ZCR_CHANGE_LIMIT seconds (default
# 900), while catch_status STATUS PATTERN POOL watches; returns when
# both have ended. It waits for that record alone: not for a status that
# lim gave up on.
during() {
    local name=$1 st=$2 pattern=$3 p=$4 pid
    shift 4
    (
        LIMIT=${ZCR_CHANGE_LIMIT:-900}
        rec "$name" "$@"
    ) &
    pid=$!
    catch_status "$st" "$pattern" "$p"
    wait "$pid"
}

main() {
    OUT=${1:?usage: zfs-cli-record.sh OUT LUA_DIR [WORK]}
    LUA_DIR=${2:?usage: zfs-cli-record.sh OUT LUA_DIR [WORK]}
    WORK=${3:-/mnt/skz-cli-record}
    [ "$(id -u)" = 0 ] || { echo "needs root" >&2; exit 64; }
    for f in snapshot members tagged; do
        [ -r "$LUA_DIR/$f.lua" ] || { echo "no $f.lua in $LUA_DIR" >&2; exit 64; }
    done
    mkdir -p "$OUT" "$WORK"
    OUT=$(cd "$OUT" && pwd)
    LUA_DIR=$(cd "$LUA_DIR" && pwd)
    : > "$OUT/index.tsv"
    adapter_env
    export DEBIAN_FRONTEND=noninteractive
    LOOPS=()
    # shellcheck disable=SC2317,SC2329  # reached through the trap
    cleanup() {
        set +e
        local p l
        [ -n "$ZINJECT" ] && lim 30 "$ZINJECT" -c all > /dev/null 2>&1
        for p in skzq skzqm skzqr skzqs; do lim 60 zpool destroy -f "$p" > /dev/null 2>&1; done
        command -v ublk > /dev/null && lim 60 ublk del -a > /dev/null 2>&1
        for l in "${LOOPS[@]}"; do losetup -d "$l" 2> /dev/null; done
        rm -rf "${OUT:?}/.catch"
        chmod -R a+rX "$OUT"
    }
    trap cleanup EXIT

    setup_failed() { echo "setup: $*" | tee "$OUT/setup-failed.txt" >&2; exit 2; }
    loop() { # image size: a loop device on a new image, in LOOP (not in a
        # subshell: LOOPS and setup_failed must reach this shell)
        truncate -s "$2" "$WORK/$1.img"
        LOOP=$(losetup --find --show "$WORK/$1.img") || setup_failed "losetup of $1"
        LOOPS+=("$LOOP")
    }

    apt-get update > /dev/null 2>&1
    apt-get install -y --no-install-recommends zfsutils-linux > "$OUT/apt.log" 2>&1
    [ -e /dev/ublk-control ] || modprobe ublk_drv 2>> "$OUT/apt.log" || {
        apt-get install -y --no-install-recommends "linux-modules-extra-$(uname -r)" >> "$OUT/apt.log" 2>&1
        modprobe ublk_drv 2>> "$OUT/apt.log"
    }
    modprobe zfs 2>> "$OUT/apt.log"
    # zinject, to slow the mirror's scans: not in zfsutils-linux.
    command -v zinject > /dev/null || apt-get install -y --no-install-recommends zfs-test >> "$OUT/apt.log" 2>&1
    ZINJECT=$(command -v zinject ||
        find /usr/sbin /usr/libexec /usr/lib/zfs-linux /usr/share/zfs -name zinject -type f 2> /dev/null | head -n 1)
    # Cloning needs the network, and the runners' name resolution fails
    # for minutes at a time: three tries, a minute apart (as the control
    # rig does).
    local built="" try
    for try in 1 2 3; do
        "$HERE/build-ublksrv.sh" "${UBLKSRV_REF:-6d0d922614ec18e6f2b5d476af8d540a1d9c1de8}" > "$OUT/ublksrv.ver" \
            2> "$OUT/ublksrv-build.log" && { built=1; break; }
        cp "$OUT/ublksrv-build.log" "$OUT/ublksrv-build-try$try.log"
        [ "$try" = 3 ] || { sleep 60; apt-get update > /dev/null 2>&1; }
    done
    [ -n "$built" ] || setup_failed "the reference ublk server did not build in three tries (ublksrv-build-try*.log)"
    [ -e /dev/ublk-control ] || setup_failed "no /dev/ublk-control (apt.log)"
    [ -r /sys/module/zfs/version ] || setup_failed "no zfs module (apt.log)"
    {
        echo "kernel $(uname -r) $(uname -m)"
        # shellcheck disable=SC1091
        echo "os $(. /etc/os-release && echo "$PRETTY_NAME")"
        echo "zfs module $(cat /sys/module/zfs/version)"
        echo "zfs userland $(zfs version 2> /dev/null | head -n 1)"
        echo "lua from ${LUA_REF:-$LUA_DIR}"
        tail -n 1 "$OUT/ublksrv.ver"
    } > "$OUT/about.txt"
    cp "$LUA_DIR"/*.lua "$OUT/"

    rec version zfs version
    rec nocacheflush cat /sys/module/zfs/parameters/zfs_nocacheflush
    rec locale locale

    # The rig pool: data on /dev/ublkb0, a log on a loop device; ublkb1 for
    # the resilver.
    local n slog cache
    for n in 0 1; do
        truncate -s 4G "$WORK/dev$n.img"
        lim 60 ublk add -t loop -n "$n" -f "$WORK/dev$n.img" -q 1 -d 64 > "$OUT/ublk-add-$n.log" 2>&1 ||
            setup_failed "ublk add $n (ublk-add-$n.log)"
    done
    for _ in $(seq 1 40); do [ -b /dev/ublkb0 ] && [ -b /dev/ublkb1 ] && break; sleep 0.25; done
    [ -b /dev/ublkb0 ] && [ -b /dev/ublkb1 ] || setup_failed "no /dev/ublkb0 or ublkb1"
    loop slog 512M
    slog=$LOOP
    loop cache 512M
    cache=$LOOP
    truncate -s 4G "$WORK/m0.img" "$WORK/m1.img" "$WORK/m2.img" "$WORK/spare.img"
    truncate -s 1G "$WORK/s0.img" "$WORK/s1.img" "$WORK/d0.img" "$WORK/d1.img"
    truncate -s 2G "$WORK/r0.img" "$WORK/r1.img"

    make_pool() { # pool MiB vdevs...: the pool, its datasets and MiB of data
        local p=$1 mib=$2
        shift 2
        lim 180 zpool create -f -o ashift=12 -o cachefile=none -O mountpoint="$WORK/mnt/$p" -O compression=off \
            "$p" "$@" > "$OUT/create-$p.log" 2>&1 || setup_failed "zpool create $p (create-$p.log)"
        {
            zfs create "$p/child" && zfs create -V 256M "$p/vol" && zfs set "$ASSIST=1700000000000" "$p" &&
                zfs set skyzfs:note=local "$p/child" &&
                # Enough data that a scrub and a resilver take a moment.
                "$DD" if=/dev/urandom of="$WORK/mnt/$p/child/data" bs=1M count="$mib" conv=fsync status=none &&
                zpool sync "$p"
        } >> "$OUT/create-$p.log" 2>&1 || setup_failed "the datasets of $p (create-$p.log)"
    }
    make_pool skzq 1024 /dev/ublkb0 log "$slog"
    make_pool skzqm 1024 mirror "$WORK/m0.img" "$WORK/m1.img" special mirror "$WORK/s0.img" "$WORK/s1.img" \
        dedup mirror "$WORK/d0.img" "$WORK/d1.img" cache "$cache" spare "$WORK/spare.img"
    make_pool skzqr 768 "$WORK/r0.img" "$WORK/r1.img"
    DELAY_POOL=skzqm DELAY=$WORK/m0.img
    if [ -z "$ZINJECT" ]; then
        echo "mirror's scans: no zinject (apt.log); they run as they are" >> "$OUT/about.txt"
    elif lim 30 "$ZINJECT" -d "$DELAY" -D 10:1 skzqm > "$OUT/zinject.txt" 2>&1 &&
        lim 30 "$ZINJECT" >> "$OUT/zinject.txt" 2>&1; then
        echo "mirror's scans: a delay of 10 ms on every I/O of m0 ($ZINJECT -D 10:1)" >> "$OUT/about.txt"
        slow_off skzqm
    else
        echo "mirror's scans: the delay was refused (zinject.txt); they run as they are" >> "$OUT/about.txt"
        ZINJECT=""
    fi
    echo "set up" >> "$OUT/about.txt"

    record_pool rig skzq
    record_pool mirror skzqm

    # Then what changes the pools' devices.
    rec rig-attach zpool attach -f skzq /dev/ublkb0 /dev/ublkb1
    catch_status rig-resilver-status 'resilver in progress' skzq
    lim 600 zpool wait -t resilver skzq > /dev/null 2>&1
    status rig-resilvered skzq

    rec mirror-offline zpool offline skzqm "$WORK/m1.img"
    "$DD" if=/dev/urandom of="$WORK/mnt/skzqm/child/more" bs=1M count=1024 conv=fsync status=none
    zpool sync skzqm
    slow_on skzqm
    during mirror-online mirror-resilver-status 'resilver in progress' skzqm zpool online skzqm "$WORK/m1.img"
    lim 600 zpool wait -t resilver skzqm > /dev/null 2>&1
    status mirror-resilvered skzqm

    # The rebuild reads either side: both slowed.
    slow_on skzqm "$WORK/m1.img"
    [ -n "$ZINJECT" ] && lim 30 "$ZINJECT" > "$OUT/zinject-rebuild.txt" 2>&1
    during mirror-rebuild mirror-rebuild-status "$REBUILD_PATTERN" skzqm \
        zpool attach -s skzqm "$WORK/m0.img" "$WORK/m2.img"
    lim 900 zpool wait -t resilver skzqm > /dev/null 2>&1
    # The scrub that follows a rebuild, at full speed.
    slow_off skzqm
    lim 600 zpool wait -t scrub skzqm > /dev/null 2>&1
    status mirror-rebuilt skzqm

    status remove-status skzqr
    rec remove zpool remove skzqr "$WORK/r1.img"
    catch_status remove-in-progress 'in progress' skzqr
    lim 600 zpool wait -t remove skzqr > /dev/null 2>&1
    status remove-done skzqr

    record_none none skznone

    # The programs' times on many file systems, when asked.
    local counts
    read -ra counts <<< "${ZCR_SCALE:-}"
    [ "${#counts[@]}" = 0 ] || scale_all "${counts[@]}"
    exit 0
}

# Sourced with ZCR_LIB=1 (the unit test), only the functions above.
[ "${ZCR_LIB:-}" = 1 ] || main "$@"

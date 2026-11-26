#!/usr/bin/env bash
# Recorded outputs of the ZFS commands the agent's ZFS adapter runs
# (skyzfs-agent, crates/skyzfs-agent/src/zfs/cli.rs and zfs/cli/*.lua),
# for its fixtures: the M2.6 plan's Q10. It judges nothing.
#
#   zfs-cli-record.sh OUT LUA_DIR [WORK]
#
# LUA_DIR holds members.lua and tagged.lua, and snapshot.lua where the
# adapter still has it, the programs as the adapter pipes them to `zfs
# program`. Without snapshot.lua (the take is `zfs snapshot -r` since
# M2.6c-2b), the records that run it are not made, and about.txt says
# so. Every command runs with
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
#           and on it, each with its wall time in secs (with an entry
#           N:K, scale-N-sK: each dataset also has K retained snapshots,
#           @r1 to @rK, taken before the steps below, so that the reads
#           that list every snapshot read N+1 times K more, on a pool
#           of 32 GiB, as many as leave 8 GiB free; #157's R1):
#             count             zfs list -r -H -o name -t
#                               filesystem,volume P, the adapter's count
#             count-snapshots   zfs list -r -H -o name -t snapshot P,
#                               the adapter's count of the snapshots,
#                               for the list's limit (#160); about.txt
#                               gives its lines
#             snapshot          the snapshot program at @t (only with
#                               snapshot.lua)
#             snapshot-cli      zfs snapshot -r P@t, only if the program
#                               did not take the whole @t (as above);
#                               about.txt then says how many it took
#             members, hold, tagged, release
#                               at @t, hold and release with -r
#             holds             zfs holds -H -r P@t while @t is held,
#                               the agent's read of the holds in each
#                               round; about.txt gives its lines
#             destroy           zfs destroy -r P@t
#             take-cli          zfs snapshot -r P@c, the take by the CLI
#             take-cli-members  members.lua at @c
#             take-cli-get      zfs get -r -H -p -t filesystem,volume -s
#                               local -o name,property,value all P: the
#                               locally set properties of every file
#                               system and volume, read by the CLI
#             take-cli-get-pairs
#                               the same with -o name,property: the
#                               agent reads the pairs, then the values
#             take-cli-get-json the same with -j in place of -H (ZFS
#                               2.3 on)
#             take-cli-list     zfs list -r -H -p -t
#                               filesystem,volume,snapshot -o
#                               name,guid,type,createtxg P
#             take-cli-full     members.lua in its full mode at @c, the
#                               manifest's read after the take (only
#                               where members.lua has that mode, and
#                               not at a count above ZCR_SCALE_FULL_MAX
#                               when set: 0 runs it at no count)
#           Each file system is made with one user property set
#           locally (skyzfs:note), so that take-cli-get has a line for
#           each. about.txt gives the members that members and
#           take-cli-members counted, which is what hold, tagged,
#           release and destroy ran on, the datasets that take-cli-full
#           listed and the lines of take-cli-get and take-cli-list.
#           The snapshot program is not run at a count above
#           ZCR_SCALE_LUA_MAX, when set; snapshot-cli takes @t there.
#           Beside the two takes (snapshot, take-cli), beside the reads
#           after the take (take-cli-members, take-cli-get and
#           take-cli-list) at every count, and beside take-cli-full at
#           the counts in ZCR_SCALE_FULL_BESIDE (default 1000; empty:
#           none), from 2 s before the command to
#           2 s after (writing, below): a writer of 4 KiB
#           O_DSYNC writes, a writer of new data past the pool's dirty
#           data limit, and the pool's transaction groups and timed
#           `zpool sync` runs. The take's record gets their logs
#           (writer.tsv, fill.tsv, txg.tsv), a line for each (stall,
#           fill, txg) and the window they were read on (window); about.txt
#           the same lines, and the dirty data limit and txg history.
#           The secs of every record with the load beside it are
#           under that load, and so are the lines: txg's before_sync and
#           before_sync_longest are the load's own before the command.
#           A bounded zpool sync after each syncs what the writer of new
#           data left. At most
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
#   count                  zfs list -r -H -o name -t filesystem,volume
#                          POOL: the adapter's count of the pool's
#                          file systems and volumes
#   snapshot-1             snapshot.lua at @t (only with snapshot.lua)
#   snapshot-cli           only if snapshot-1 did not take the whole
#                          @t: zfs snapshot -r, so that what follows has
#                          its @t (cli_snapshot: where the program failed
#                          part way, what it took is destroyed first,
#                          snapshot-cli-destroy-partial)
#   snapshot-2             snapshot.lua at @t again: EEXIST (expected;
#                          only with snapshot.lua)
#   members, members-absent, tagged
#                          members.lua at t and at a t that is absent;
#                          tagged.lua before any hold
#   members-full, members-full-absent
#                          members.lua in its full mode (`POOL T full
#                          ASSIST`) at t and at a t that is absent. Every
#                          members-full record is made only where
#                          members.lua has that mode (#153 on), and
#                          about.txt says when it has not
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
#                          LC_ALL=C.UTF-8; without snapshot.lua, take-o0
#                          and take-o2, zfs snapshot -r at each
#   members-full-odd, members-full-odd-utf8
#                          members.lua in its full mode at o0 with
#                          LC_ALL=C and at o2 with LC_ALL=C.UTF-8: the
#                          properties as the manifest reads them
#   exists-child-pre, snapshot-exists-child, exists-child-after
#                          the child has @p and the root has not, then
#                          snapshot.lua at @p, which refuses at its check
#                          before taking anything (EEXIST, expected; only
#                          with snapshot.lua), then the snapshots that
#                          are there
#   members-full-partial   members.lua in its full mode at p, which the
#                          child has and the root has not (after
#                          exists-child-pre)
#   cli-exists-child, cli-exists-child-after, cli-exists-root
#                          zfs snapshot -r, the take, where it fails: at
#                          @p, which the child has ("dataset already
#                          exists", expected), then the snapshots there;
#                          at @t, which the root has (expected)
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
#   cli-limit-set, cli-limit, cli-limit-after
#                          snapshot_limit=0 on the child, then zfs
#                          snapshot -r at @q, then the snapshots there:
#                          whether a limit refuses the take run as root
#                          (the limit is then lifted and @q destroyed,
#                          with no record)
#   odd-set-K              a file system P/odd-K for each kind K of
#                          value below (made with no record), and zfs set
#                          skyzfs:v=VALUE on it: nl, a newline inside;
#                          tab, a tab inside; utf8, bytes above 0x7f in
#                          UTF-8; raw, bytes that are not UTF-8; forge,
#                          a newline followed by what reads as one more
#                          line of odd-get-local (P/child, skyzfs:forged,
#                          yes); empty, no byte; dash, "-" (what the
#                          get prints for no value); cr, a carriage
#                          return inside; spaces, a space at each end;
#                          bslash, a backslash and an n (not a
#                          newline); len4095, len4096, len8191 and
#                          len8192, that many bytes. A set that is
#                          refused is recorded as such
#   odd-get-local, odd-get-local-header, odd-get-local-json
#                          the locally set properties of every file
#                          system and volume, as take-cli-get reads them
#                          at scale (-H -p), with its header (-p alone),
#                          and in JSON (-j -p; ZFS 2.3 on)
#   odd-value-K, odd-json-K
#                          zfs get -H -p -o value skyzfs:v P/odd-K: one
#                          value alone, whose stdout is the value and a
#                          newline; and the same file system's value in
#                          JSON (-j -p -o name,property,value), apart
#                          from the others' values
#   odd-take, odd-recv, odd-get-received
#                          zfs snapshot -r P@g, then zfs send -p of
#                          P/odd-nl@g received as P/odd-recv (its
#                          skyzfs:v is then received, not local), then
#                          the get with -s local,received and the source
#   odd-list, odd-full     zfs list -r -H -p -t filesystem,volume,snapshot
#                          -o name,guid,type,createtxg P, and
#                          members.lua's full mode at g: the same
#                          properties as the program reads them. @g,
#                          P/odd-recv and the odd file systems are then
#                          destroyed, with no record
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
# and `zpool list -H -o name`, by which the adapter tells a pool that is
# not there from a snapshot that went (#160): pools-none before any pool
# of the recording is made, pools-one with skzq alone, pools-three with
# skzq, skzqm and skzqr, and none-pools-named of skznone, a pool that
# does not exist (`zpool list -H -o name skznone`);
#
# and, on a pool of their own (skzrd, a file), the manifest's read by the
# CLI as the adapter runs it (M2.6c-5): read-S-list (zfs list -r -H -p -t
# filesystem,volume,snapshot -o name,guid,type,createtxg P),
# read-S-get-names and read-S-get-values (zfs get -r -H -p -s local -t
# filesystem,volume -o name,property[,value] all P), in each state S:
#
#   tree     P, P/child (skyzfs:note local) and the volume P/vol with
#            @t, skyzfs:assist on P, and P/late made after @t, without
#            it; then read-value-set, read-value-unset and
#            read-value-missing: zfs get -H -p -o value of skyzfs:note on
#            P/child, of skyzfs:unset (never set) on it, and of
#            skyzfs:note on P/missing (no such dataset)
#   not      the same with no @t
#   odd      P/odd-K with skyzfs:v of each kind K of READ_KINDS
#            (read-odd-set-K, odd_value's), all with @t; and
#            read-odd-value-K, zfs get -H -p -o value skyzfs:v P/odd-K
#   counts   filesystem_limit and snapshot_limit set on P/child
#            (read-counts-set), so that filesystem_count and
#            snapshot_count are tracked, with @t; read-counts-fs and
#            read-counts-snap the two counts' values on P/child
#   none     P destroyed: the three reads and read-none-value
#
# and once: `nocacheflush` (the module parameter), `version` and
# `locale`. On the pool that does not exist, every command the adapter
# runs: status, status-g, get, assist-set, assist-sync, the three
# programs, holds, holds-r, hold, release, destroy, and the take by
# zfs snapshot -r (snapshot-cli).
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
# Whether LUA_DIR has the snapshot program (main): 0 once the take is
# `zfs snapshot -r` alone.
SNAPSHOT_LUA=1
# Whether members.lua has its full mode (main): 0 for a members.lua from
# before #153, whose answer to `... T full ASSIST` is the plain mode's,
# so that no record is named for a mode that did not run.
MEMBERS_FULL=1
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

# has_full FILE: whether the members program FILE has its full mode:
# the word "full" quoted, which the program before #153 has nowhere.
has_full() {
    grep -q '"full"' "$1"
}

# full NAME POOL [--locale L] T: members.lua in its full mode at T, as
# the adapter reads the manifest; nothing without that mode.
full() {
    [ "$MEMBERS_FULL" = 1 ] || return 0
    local name=$1 p=$2 locale=()
    shift 2
    if [ "$1" = --locale ]; then
        locale=(--locale "$2")
        shift 2
    fi
    program "$name" "$p" members.lua "${locale[@]}" -n "$p" "$1" full "$ASSIST"
}

# members_line RECORD: how many members (datasets, in the full mode)
# the members program's RECORD listed, in about.txt.
members_line() {
    local k
    k=$(python3 -I -c 'import json, sys
try:
    r = json.load(open(sys.argv[1]))["return"]
    k = "datasets" if "datasets" in r else "members"
    print(len(r[k]), k)
except Exception:
    print("not read")' "$OUT/$1/stdout" 2> /dev/null)
    echo "scale: $1: ${k:-not read}" >> "$OUT/about.txt"
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
    rec "$x-count" zfs list -r -H -o name -t filesystem,volume "$p"
    if [ "$SNAPSHOT_LUA" = 1 ]; then
        program "$x-snapshot-1" "$p" snapshot.lua "$p" t "$ASSIST"
    fi
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

# The kinds of value of odd-set-K, and each value (odd_value K POOL, on
# stdout, with no newline at its end).
ODD_KINDS="nl tab utf8 raw forge empty dash cr spaces bslash len4095 len4096 len8191 len8192"
odd_value() {
    case $1 in
        nl) printf 'one\ntwo' ;;
        tab) printf 'one\ttwo' ;;
        utf8) printf 'naïve, 日本, ∅' ;;
        raw) printf 'a\xff\xfeb\xc3' ;;
        forge) printf 'one\n%s/child\tskyzfs:forged\tyes' "$2" ;;
        empty) ;;
        dash) printf -- - ;;
        cr) printf 'one\rtwo' ;;
        spaces) printf ' one two ' ;;
        bslash) printf 'one\\ntwo' ;;
        len*) head -c "${1#len}" /dev/zero | tr '\0' v ;;
    esac
}

# record_odd PREFIX POOL: the odd values, and how the CLI and the full
# mode read them (the header's odd-* records); nothing of them is left.
record_odd() {
    local x=$1 p=$2 k
    for k in $ODD_KINDS; do
        lim 120 zfs create -u "$p/odd-$k" > /dev/null 2>&1
        rec "$x-odd-set-$k" zfs set "skyzfs:v=$(odd_value "$k" "$p")" "$p/odd-$k"
    done
    rec "$x-odd-get-local" zfs get -r -H -p -t filesystem,volume -s local -o name,property,value all "$p"
    rec "$x-odd-get-local-header" zfs get -r -p -t filesystem,volume -s local -o name,property,value all "$p"
    rec "$x-odd-get-local-json" zfs get -j -r -p -t filesystem,volume -s local -o name,property,value all "$p"
    for k in $ODD_KINDS; do
        rec "$x-odd-value-$k" zfs get -H -p -o value skyzfs:v "$p/odd-$k"
        rec "$x-odd-json-$k" zfs get -j -p -o name,property,value skyzfs:v "$p/odd-$k"
    done
    rec "$x-odd-take" zfs snapshot -r "$p@g"
    # shellcheck disable=SC2016 # bash -c's own arguments
    rec "$x-odd-recv" bash -c 'zfs send -p "$1" | zfs recv -u "$2"' odd-recv "$p/odd-nl@g" "$p/odd-recv"
    rec "$x-odd-get-received" zfs get -r -H -p -t filesystem,volume -s local,received -o name,property,value,source all "$p"
    rec "$x-odd-list" zfs list -r -H -p -t filesystem,volume,snapshot -o name,guid,type,createtxg "$p"
    full "$x-odd-full" "$p" g
    lim 120 zfs destroy -r "$p/odd-recv" > /dev/null 2>&1
    lim 120 zfs destroy -r "$p@g" > /dev/null 2>&1
    for k in $ODD_KINDS; do
        lim 120 zfs destroy -r "$p/odd-$k" > /dev/null 2>&1
    done
}

# The manifest's read by the CLI (M2.6c-5), as the adapter runs it: the
# list of every file system, volume and snapshot, and the locally set
# properties, names alone and with their values.
READ_LIST="zfs list -r -H -p -t filesystem,volume,snapshot -o name,guid,type,createtxg"
READ_NAMES="zfs get -r -H -p -s local -t filesystem,volume -o name,property all"
READ_VALUES="zfs get -r -H -p -s local -t filesystem,volume -o name,property,value all"
# The odd values coding asked for (odd_value's kinds).
READ_KINDS="nl tab utf8 raw forge empty dash cr spaces bslash len8191"
# read_pass NAME POOL: the three reads, NAME-list, NAME-get-names and
# NAME-get-values.
read_pass() {
    # shellcheck disable=SC2086 # the commands' words
    rec "$1-list" $READ_LIST "$2"
    # shellcheck disable=SC2086
    rec "$1-get-names" $READ_NAMES "$2"
    # shellcheck disable=SC2086
    rec "$1-get-values" $READ_VALUES "$2"
}
# record_reads PREFIX POOL IMAGE: the read records (the header's
# PREFIX-*), on a pool of their own on IMAGE, then on that pool once
# destroyed (a pool that does not exist). Setup steps that fail are said
# in about.txt.
record_reads() {
    local x=$1 p=$2 img=$3 k
    truncate -s 1G "$img"
    if ! lim 180 zpool create -f -o ashift=12 -o cachefile=none -O mountpoint="$WORK/mnt/$p" -O compression=off \
        "$p" "$img" > "$OUT/create-$p.log" 2>&1; then
        echo "$x: zpool create $p failed (create-$p.log): no read records but none's" >> "$OUT/about.txt"
    else
        {
            lim 120 zfs create "$p/child" && lim 120 zfs create -V 64M "$p/vol" &&
                lim 120 zfs set "$ASSIST=1700000000000" "$p" && lim 120 zfs set skyzfs:note=local "$p/child" &&
                lim 120 zfs snapshot -r "$p@t" && lim 120 zfs create "$p/late"
        } >> "$OUT/create-$p.log" 2>&1 || echo "$x: the tree of $p was not all made (create-$p.log)" >> "$OUT/about.txt"
        read_pass "$x-tree" "$p"
        rec "$x-value-set" zfs get -H -p -o value skyzfs:note "$p/child"
        rec "$x-value-unset" zfs get -H -p -o value skyzfs:unset "$p/child"
        rec "$x-value-missing" zfs get -H -p -o value skyzfs:note "$p/missing"
        lim 120 zfs destroy -r "$p@t" >> "$OUT/create-$p.log" 2>&1 || echo "$x: destroy -r $p@t failed" >> "$OUT/about.txt"
        read_pass "$x-not" "$p"
        for k in $READ_KINDS; do
            lim 120 zfs create -u "$p/odd-$k" >> "$OUT/create-$p.log" 2>&1
            rec "$x-odd-set-$k" zfs set "skyzfs:v=$(odd_value "$k" "$p")" "$p/odd-$k"
        done
        lim 120 zfs snapshot -r "$p@t" >> "$OUT/create-$p.log" 2>&1 || echo "$x: the odd pool's @t failed" >> "$OUT/about.txt"
        read_pass "$x-odd" "$p"
        for k in $READ_KINDS; do
            rec "$x-odd-value-$k" zfs get -H -p -o value skyzfs:v "$p/odd-$k"
        done
        lim 120 zfs destroy -r "$p@t" >> "$OUT/create-$p.log" 2>&1
        for k in $READ_KINDS; do
            lim 120 zfs destroy -r "$p/odd-$k" >> "$OUT/create-$p.log" 2>&1
        done
        rec "$x-counts-set" zfs set filesystem_limit=100 snapshot_limit=100 "$p/child"
        lim 120 zfs snapshot -r "$p@t" >> "$OUT/create-$p.log" 2>&1 || echo "$x: the counts' @t failed" >> "$OUT/about.txt"
        read_pass "$x-counts" "$p"
        rec "$x-counts-fs" zfs get -H -p -o value filesystem_count "$p/child"
        rec "$x-counts-snap" zfs get -H -p -o value snapshot_count "$p/child"
        lim 180 zpool destroy -f "$p" >> "$OUT/create-$p.log" 2>&1 || echo "$x: zpool destroy $p failed" >> "$OUT/about.txt"
    fi
    read_pass "$x-none" "$p"
    rec "$x-none-value" zfs get -H -p -o value skyzfs:note "$p/child"
}

# lines_line RECORD: how many lines RECORD's stdout has, in about.txt.
lines_line() {
    echo "scale: $1: $(wc -l < "$OUT/$1/stdout" 2> /dev/null || echo no) lines" >> "$OUT/about.txt"
}

# Everything recorded on a live pool before the changes of its devices.
record_pool() { # prefix pool
    local x=$1 p=$2
    agent_commands "$x" "$p"
    # Without the program, a snapshot for what follows all the same.
    cli_snapshot "$x-snapshot-cli" "$x-snapshot-1" "$p"
    if [ "$SNAPSHOT_LUA" = 1 ]; then
        program "$x-snapshot-2" "$p" snapshot.lua --expect '"errno": *"17"' "$p" t "$ASSIST"
    fi
    program "$x-members" "$p" members.lua -n "$p" t
    program "$x-members-absent" "$p" members.lua --expect '"absent"' -n "$p" absent
    full "$x-members-full" "$p" t
    full "$x-members-full-absent" "$p" absent
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
    if [ "$SNAPSHOT_LUA" = 1 ]; then
        program "$x-snapshot-odd" "$p" snapshot.lua "$p" o0 "$ASSIST"
        program "$x-snapshot-odd-utf8" "$p" snapshot.lua --locale C.UTF-8 "$p" o2 "$ASSIST"
    else
        rec "$x-take-o0" zfs snapshot -r "$p@o0"
        rec "$x-take-o2" zfs snapshot -r "$p@o2"
    fi
    # The manifest's reading of those properties, in each locale.
    full "$x-members-full-odd" "$p" o0
    full "$x-members-full-odd-utf8" "$p" --locale C.UTF-8 o2
    # The check refuses before anything is taken: the child has @p.
    rec "$x-exists-child-pre" zfs snapshot "$p/child@p"
    # A state that some members have: the child's @p alone.
    full "$x-members-full-partial" "$p" p
    if [ "$SNAPSHOT_LUA" = 1 ]; then
        program "$x-snapshot-exists-child" "$p" snapshot.lua --expect '"errno": *"17"' "$p" p "$ASSIST"
    fi
    rec "$x-exists-child-after" zfs list -H -t snapshot -o name -r "$p"
    # zfs snapshot -r, the take since e6788b0, where it fails: the
    # child's @p is there, and the root's @t is there.
    rec "$x-cli-exists-child" --expect 'dataset already exists' zfs snapshot -r "$p@p"
    rec "$x-cli-exists-child-after" zfs list -H -t snapshot -o name -r "$p"
    rec "$x-cli-exists-root" --expect 'dataset already exists' zfs snapshot -r "$p@t"
    # The adapter's destroy -r of a take it cannot read.
    rec "$x-destroy" zfs destroy -r "$p@o0"
    rec "$x-destroy-after" zfs list -H -t snapshot -o name -r "$p"
    rec "$x-destroy-absent" --expect 'could not find any snapshots to destroy' zfs destroy -r "$p@absent"
    rec "$x-destroy-held-hold" zfs hold "$TAG" "$p/child@o2"
    rec "$x-destroy-held" zfs destroy -r "$p@o2"
    rec "$x-destroy-held-after" zfs list -H -t snapshot -o name -r "$p"
    release_quietly "$p" o2
    # A limit on the child's snapshots: root is not held to it where it
    # may change it, so this records whether the take is refused.
    rec "$x-cli-limit-set" zfs set snapshot_limit=0 "$p/child"
    rec "$x-cli-limit" zfs snapshot -r "$p@q"
    rec "$x-cli-limit-after" zfs list -H -t snapshot -o name -r "$p"
    lim 120 zfs set snapshot_limit=none "$p/child" > /dev/null 2>&1
    lim 120 zfs destroy -r "$p@q" > /dev/null 2>&1
    record_odd "$x" "$p"
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
    full "$x-members-full" "$p" t
    program "$x-tagged" "$p" tagged.lua -n "$p" "$TAG"
    rec "$x-holds" zfs holds -H "$p@t"
    rec "$x-holds-r" zfs holds -H -r "$p@t"
    rec "$x-hold" zfs hold -r "$TAG" "$p@t"
    rec "$x-release" zfs release -r "$TAG" "$p@t"
    rec "$x-destroy" zfs destroy -r "$p@t"
    rec "$x-snapshot-cli" zfs snapshot -r "$p@t"
    rec "$x-pools-named" zpool list -H -o name "$p"
}

# scale_all ENTRY...: scale for each entry, at most three, each once,
# all within ZCR_SCALE_BUDGET seconds. An entry is a count N of file
# systems, at most 100000, or N:K with K retained snapshots of each, K
# at most 20000 (one call each, about 0.12 s a call with two datasets in
# run 45774416615: some 40 minutes at most), at most 200000 snapshots in
# all ((N+1)*K).
SCALE_END=0
scale_all() {
    local e n r seen=" " k=0
    SCALE_END=$(($(date +%s) + ${ZCR_SCALE_BUDGET:-4800}))
    ZCR_SCALE_LUA_MAX=${ZCR_SCALE_LUA_MAX:-}
    case $ZCR_SCALE_LUA_MAX in
        *[!0-9]* | 0?*)
            echo "scale: ZCR_SCALE_LUA_MAX is not a count: $ZCR_SCALE_LUA_MAX; the program runs at every count" >> "$OUT/about.txt"
            ZCR_SCALE_LUA_MAX=""
            ;;
    esac
    [ "${#ZCR_SCALE_LUA_MAX}" -gt 6 ] && ZCR_SCALE_LUA_MAX=999999
    ZCR_SCALE_FULL_MAX=${ZCR_SCALE_FULL_MAX:-}
    case $ZCR_SCALE_FULL_MAX in
        *[!0-9]* | 0?*)
            echo "scale: ZCR_SCALE_FULL_MAX is not a count: $ZCR_SCALE_FULL_MAX; the full read runs at every count" >> "$OUT/about.txt"
            ZCR_SCALE_FULL_MAX=""
            ;;
    esac
    [ "${#ZCR_SCALE_FULL_MAX}" -gt 6 ] && ZCR_SCALE_FULL_MAX=999999
    for e in "$@"; do
        n=${e%%:*}
        r=0
        case $e in *:*) r=${e#*:} ;; esac
        case $n in *[!0-9]* | 0* | '') echo "scale: not a count: $e" >> "$OUT/about.txt"; continue ;; esac
        case $r in 0) ;; *[!0-9]* | 0* | '') echo "scale: not a count of snapshots: $e" >> "$OUT/about.txt"; continue ;; esac
        if [ "${#n}" -gt 6 ] || [ "$n" -gt 100000 ]; then
            echo "scale: $n is more than 100000; not run" >> "$OUT/about.txt"
            continue
        fi
        if [ "${#r}" -gt 6 ] || [ "$r" -gt 20000 ]; then
            echo "scale: $e retains more than 20000 of each dataset; not run" >> "$OUT/about.txt"
            continue
        fi
        if [ $(((n + 1) * r)) -gt 200000 ]; then
            echo "scale: $e makes more than 200000 snapshots; not run" >> "$OUT/about.txt"
            continue
        fi
        case $seen in *" $n:$r "*) echo "scale: $e twice; run once" >> "$OUT/about.txt"; continue ;; esac
        if [ "$k" -ge 3 ]; then
            echo "scale: more than three counts; $e not run" >> "$OUT/about.txt"
            continue
        fi
        seen="$seen$n:$r "
        k=$((k + 1))
        scale "$n" "$r"
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

# began FILE...: until each FILE has something in it, at most 10 s.
began() {
    local i=0 f ok
    while [ "$i" -lt 100 ]; do
        ok=1
        for f in "$@"; do [ -s "$f" ] || ok=0; done
        [ "$ok" = 0 ] || return 0
        sleep 0.1
        i=$((i + 1))
    done
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
    # The margin runs from when the logs have begun, not from when their
    # programs were started: on a loaded machine a start can take longer
    # than a short margin.
    if [ -n "$fpid" ]; then began "$log.tsv" "$log.fill.tsv" "$log.txg.tsv"; else began "$log.tsv" "$log.txg.tsv"; fi
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

# retain LABEL POOL N K: K snapshots of the root, recursive (@r1 to
# @rK), so that each of its N+1 datasets has K that no take names, for
# the reads that list every snapshot (#157's R1). One name a call: ZFS
# refuses two snapshots of one dataset in one call ("multiple snapshots
# of same fs not allowed", run 45774416615). Each call is under the
# scale's limit. It stops, before a call, once the pool has less than
# ZCR_RETAIN_FLOOR bytes free (8 GiB), read every 100 calls, so that the
# takes and writers after it have room: in run 45774416615 a 16 GiB pool
# was full at 9729 of each of 2 datasets, and the takes after it failed.
# At each tenth of K and at the last, about.txt gets the pool's
# allocated and free bytes and the time so far, which show how the
# space grows with K; at the end, how many the pool has and how long
# they took.
retain() {
    local lbl=$1 p=$2 n=$3 k=$4 i=1 t0 rc cut="" have step free floor=${ZCR_RETAIN_FLOOR:-8589934592}
    step=$((k / 10))
    [ "$step" -ge 1 ] || step=1
    t0=$(date +%s)
    while [ "$i" -le "$k" ]; do
        if ! scale_limit; then
            cut=" (stopped: the budget was spent)"
            break
        fi
        if [ $(((i - 1) % 100)) = 0 ]; then
            free=$(lim "$LIMIT" zpool list -Hp -o free "$p" 2> /dev/null)
            case $free in
                '' | *[!0-9]*) ;;
                *) if [ "$free" -lt "$floor" ]; then
                    cut=" (stopped before @r$i: $free bytes free, under $floor)"
                    break
                fi ;;
            esac
        fi
        rc=0
        lim "$LIMIT" zfs snapshot -r "$p@r$i" >> "$OUT/retain-$p-$lbl.log" 2>&1 || rc=$?
        if [ "$rc" != 0 ]; then
            cut=" (stopped: zfs snapshot -r of @r$i ended $rc, retain-$p-$lbl.log)"
            break
        fi
        if [ $((i % step)) = 0 ] || [ "$i" = "$k" ]; then
            echo "scale $lbl: retained @r$i at $(($(date +%s) - t0)) s: allocated and free $(lim "$LIMIT" zpool list -Hp -o alloc,free "$p" 2> /dev/null | tr '\t' ' ')" >> "$OUT/about.txt"
        fi
        i=$((i + 1))
    done
    have=$(lim 600 zfs list -H -t snapshot -o name -r "$p" 2> /dev/null | wc -l)
    echo "scale $lbl: $have snapshots in the pool ($k of each of $((n + 1)) datasets asked, one name a call) made in $(($(date +%s) - t0)) s$cut" >> "$OUT/about.txt"
}

# scale N [K]: a pool with N file systems under its root, and the adapter's
# programs and holds on it, each timed (the secs of its record); with K,
# K retained snapshots of each dataset as well (retain), and its records
# are named scale-N-sK.
scale() {
    local n=$1 k=${2:-0} lbl p=skzqs t0 made step cut="" rc wdir
    local LIMIT FILL_SPAN=""
    lbl=$n
    [ "$k" = 0 ] || lbl=$n-s$k
    if ! scale_limit; then
        echo "scale $lbl: not run, the budget was spent" >> "$OUT/about.txt"
        return 0
    fi
    df -h "$WORK" 2>&1 | sed "s/^/scale $lbl: df: /" >> "$OUT/about.txt"
    # Retained snapshots take room of their own (retain).
    if [ "$k" = 0 ]; then truncate -s 16G "$WORK/scale.img"; else truncate -s 32G "$WORK/scale.img"; fi
    if ! lim 180 zpool create -f -o ashift=12 -o cachefile=none -O mountpoint="$WORK/mnt/$p" \
        -O compression=off "$p" "$WORK/scale.img" > "$OUT/create-$p-$lbl.log" 2>&1; then
        echo "scale $lbl: zpool create failed (create-$p-$lbl.log)" >> "$OUT/about.txt"
        rm -f "$WORK/scale.img"
        return 0
    fi
    zfs set "$ASSIST=1700000000000" "$p" >> "$OUT/create-$p-$lbl.log" 2>&1
    # The writer's file is in the root, and only if the root is mounted.
    wdir=""
    if mountpoint -q "$WORK/mnt/$p"; then
        wdir=$WORK/mnt/$p
        fill_span "$lbl"
    else
        echo "scale $lbl: the root is not mounted; no writer" >> "$OUT/about.txt"
    fi
    t0=$(date +%s)
    if scale_limit; then
        rc=0
        seq 1 "$n" | lim "$LIMIT" xargs -P 8 -I{} zfs create -u -o skyzfs:note=local "$p/d{}" >> "$OUT/create-$p-$lbl.log" 2>&1 || rc=$?
        [ "$rc" = 124 ] && cut=" (stopped at the limit of $LIMIT s)"
        made=$(lim 120 zfs list -H -o name -r "$p" 2> /dev/null | wc -l)
        echo "scale $lbl: $made datasets (the root among them) made in $(($(date +%s) - t0)) s$cut" >> "$OUT/about.txt"
    fi
    [ "$k" = 0 ] || retain "$lbl" "$p" "$n" "$k"
    for step in count count-snapshots snapshot snapshot-cli members hold holds tagged release destroy take-cli take-cli-members take-cli-get take-cli-get-pairs take-cli-get-json take-cli-list take-cli-full; do
        if ! scale_limit; then
            echo "scale $lbl: the budget was spent before $step; it and the rest were not run" >> "$OUT/about.txt"
            break
        fi
        case $step in
            count) rec "scale-$lbl-count" zfs list -r -H -o name -t filesystem,volume "$p" ;;
            count-snapshots)
                rec "scale-$lbl-count-snapshots" zfs list -r -H -o name -t snapshot "$p"
                lines_line "scale-$lbl-count-snapshots"
                ;;
            snapshot)
                if [ "$SNAPSHOT_LUA" != 1 ]; then
                    echo "scale $lbl: the snapshot program not run: there is no snapshot.lua" >> "$OUT/about.txt"
                elif [ -n "${ZCR_SCALE_LUA_MAX:-}" ] && [ "$n" -gt "$ZCR_SCALE_LUA_MAX" ]; then
                    echo "scale $lbl: the snapshot program not run: $n is more than ZCR_SCALE_LUA_MAX ($ZCR_SCALE_LUA_MAX)" >> "$OUT/about.txt"
                else
                    writing "scale-$lbl-snapshot" "$wdir" "$p" program "scale-$lbl-snapshot" "$p" snapshot.lua "$p" t "$ASSIST"
                fi
                ;;
            # Without the program's whole @t, the CLI's, so that the rest
            # has one.
            snapshot-cli) cli_snapshot "scale-$lbl-snapshot-cli" "scale-$lbl-snapshot" "$p" ;;
            members)
                program "scale-$lbl-members" "$p" members.lua -n "$p" t
                members_line "scale-$lbl-members"
                ;;
            hold) rec "scale-$lbl-hold" zfs hold -r "$TAG" "$p@t" ;;
            holds)
                rec "scale-$lbl-holds" zfs holds -H -r "$p@t"
                lines_line "scale-$lbl-holds"
                ;;
            tagged) program "scale-$lbl-tagged" "$p" tagged.lua -n "$p" "$TAG" ;;
            release) rec "scale-$lbl-release" zfs release -r "$TAG" "$p@t" ;;
            destroy) rec "scale-$lbl-destroy" zfs destroy -r "$p@t" ;;
            take-cli) writing "scale-$lbl-take-cli" "$wdir" "$p" rec "scale-$lbl-take-cli" zfs snapshot -r "$p@c" ;;
            # What the manifest reads after the take, at this count: by
            # the plain program, by the CLI (M2.6c-5), and by the full
            # program. Each read but the full one, and that one at the
            # counts asked, with the load beside it.
            take-cli-members)
                writing "scale-$lbl-take-cli-members" "$wdir" "$p" program "scale-$lbl-take-cli-members" "$p" members.lua -n "$p" c
                members_line "scale-$lbl-take-cli-members"
                ;;
            take-cli-get)
                writing "scale-$lbl-take-cli-get" "$wdir" "$p" rec "scale-$lbl-take-cli-get" zfs get -r -H -p -t filesystem,volume -s local -o name,property,value all "$p"
                lines_line "scale-$lbl-take-cli-get"
                ;;
            # The agent reads the pairs first, then the values (cli.rs).
            take-cli-get-pairs)
                rec "scale-$lbl-take-cli-get-pairs" zfs get -r -H -p -t filesystem,volume -s local -o name,property all "$p"
                lines_line "scale-$lbl-take-cli-get-pairs"
                ;;
            take-cli-get-json)
                rec "scale-$lbl-take-cli-get-json" zfs get -j -r -p -t filesystem,volume -s local -o name,property,value all "$p"
                ;;
            take-cli-list)
                writing "scale-$lbl-take-cli-list" "$wdir" "$p" rec "scale-$lbl-take-cli-list" zfs list -r -H -p -t filesystem,volume,snapshot -o name,guid,type,createtxg "$p"
                lines_line "scale-$lbl-take-cli-list"
                ;;
            take-cli-full)
                if [ "$MEMBERS_FULL" = 1 ] && [ -n "${ZCR_SCALE_FULL_MAX:-}" ] && [ "$n" -gt "$ZCR_SCALE_FULL_MAX" ]; then
                    echo "scale $lbl: the full mode not run: $n is more than ZCR_SCALE_FULL_MAX ($ZCR_SCALE_FULL_MAX)" >> "$OUT/about.txt"
                elif [ "$MEMBERS_FULL" = 1 ]; then
                    # The read is read-only (-n): whether it delays
                    # writes or the pool's groups is measured beside it
                    # at the counts asked.
                    case " ${ZCR_SCALE_FULL_BESIDE-1000} " in
                        *" $n "*) writing "scale-$lbl-take-cli-full" "$wdir" "$p" full "scale-$lbl-take-cli-full" "$p" c ;;
                        *) full "scale-$lbl-take-cli-full" "$p" c ;;
                    esac
                    members_line "scale-$lbl-take-cli-full"
                else
                    echo "scale $lbl: the full mode not run: members.lua has none" >> "$OUT/about.txt"
                fi
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
    for f in members tagged; do
        [ -r "$LUA_DIR/$f.lua" ] || { echo "no $f.lua in $LUA_DIR" >&2; exit 64; }
    done
    [ -r "$LUA_DIR/snapshot.lua" ] || SNAPSHOT_LUA=0
    has_full "$LUA_DIR/members.lua" || MEMBERS_FULL=0
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
        for p in skzq skzqm skzqr skzqs skzrd; do lim 60 zpool destroy -f "$p" > /dev/null 2>&1; done
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
        [ "$SNAPSHOT_LUA" = 1 ] || echo "no snapshot.lua: the take is zfs snapshot -r alone; the snapshot program's records are not made"
        [ "$MEMBERS_FULL" = 1 ] || echo "members.lua has no full mode: the members-full records are not made"
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
    rec pools-none zpool list -H -o name
    make_pool skzq 1024 /dev/ublkb0 log "$slog"
    rec pools-one zpool list -H -o name
    make_pool skzqm 1024 mirror "$WORK/m0.img" "$WORK/m1.img" special mirror "$WORK/s0.img" "$WORK/s1.img" \
        dedup mirror "$WORK/d0.img" "$WORK/d1.img" cache "$cache" spare "$WORK/spare.img"
    make_pool skzqr 768 "$WORK/r0.img" "$WORK/r1.img"
    rec pools-three zpool list -H -o name
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
    record_reads read skzrd "$WORK/rd.img"

    # The programs' times on many file systems, when asked.
    local counts
    read -ra counts <<< "${ZCR_SCALE:-}"
    [ "${#counts[@]}" = 0 ] || scale_all "${counts[@]}"
    exit 0
}

# Sourced with ZCR_LIB=1 (the unit test), only the functions above.
[ "${ZCR_LIB:-}" = 1 ] || main "$@"

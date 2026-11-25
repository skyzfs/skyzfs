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
#   note    only where there is something to say: a state looked for
#           and whether it was caught, or an expected message and
#           whether it was seen
#
# and one line "NAME<TAB>rc<TAB>note<TAB>cmd" of OUT/index.tsv, in the
# order run. NAME is "<pool>-<what>" with pool
#
#   rig     skzq: the data on /dev/ublkb0 of the reference ublk server,
#           with a log device
#   mirror  skzqm: a mirror of two files, with a special and a dedup
#           mirror, a cache device and a spare, so that each class's
#           header is in its status. Its files scan in seconds, so its
#           scrubs, resilver and rebuild run with a delay of 10 ms
#           injected on every I/O of m0 (zinject -D 10:1, from the
#           package zfs-test; about.txt says whether it took, and
#           zinject.txt why not): the state's text is the same, it
#           lasts long enough to be seen
#   remove  skzqr: two files, one of them removed (the removal's states)
#   none    skznone, a pool that does not exist
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
#   snapshot-cli           only if snapshot-1 did not take @t: zfs
#                          snapshot -r, so that what follows has its @t
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
#                          rebuild)
#
# On files, `zpool online` and `zpool attach` return only once the
# resilver has ended, so these two run in the background while the
# status is watched.
#   remove-status, remove-status-g, remove, remove-in-progress[-g],
#   remove-done[-g]        skzqr's second file removed
#
# and once: `nocacheflush` (the module parameter), `version` and
# `locale`. On the pool that does not exist, every command the adapter
# runs: status, status-g, get, assist-set, assist-sync, the three
# programs, holds, holds-r, hold, release.
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

# rec NAME [--in FILE] [--expect PATTERN] COMMAND...: run COMMAND once
# and keep it as the record NAME. With --expect, the note says whether
# its stdout or stderr matched PATTERN (grep -E).
rec() {
    local name=$1 in=/dev/null expect="" shown d rc=0 note=""
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
    lim "$LIMIT" "$@" < "$in" > "$d/stdout" 2> "$d/stderr" || rc=$?
    echo "$rc" > "$d/rc"
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
    local name=$1 pattern=$2 tries=$3 i tmp rc caught="" note
    shift 3
    tmp=$OUT/.catch
    for ((i = 1; i <= tries; i++)); do
        rm -rf "${tmp:?}"
        mkdir -p "$tmp"
        rc=0
        lim "$LIMIT" "$@" < /dev/null > "$tmp/stdout" 2> "$tmp/stderr" || rc=$?
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
    keep "$name" "$tmp" "$rc" "$note" "$@" || { rm -rf "${tmp:?}"; return 70; }
    [ -n "$caught" ]
}

# keep NAME DIR RC NOTE COMMAND...: DIR's stdout and stderr as the
# record NAME.
keep() {
    local name=$1 tmp=$2 rc=$3 note=$4 d shown
    shift 4
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
# pool DELAY_POOL to inject a delay on.
DELAY_POOL="" DELAY="" ZINJECT=""
slow_on() { # pool
    [ "$1" = "$DELAY_POOL" ] && [ -n "$DELAY" ] && [ -n "$ZINJECT" ] || return 0
    lim 30 "$ZINJECT" -d "$DELAY" -D 10:1 "$1" > /dev/null 2>&1
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
    zfs list -H -o name "$p@t" > /dev/null 2>&1 || rec "$x-snapshot-cli" zfs snapshot -r "$p@t"
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
    rec "$x-checkpoint" zpool checkpoint "$p"
    rec "$x-get-checkpoint" zpool get -H -p -o property,value guid,load_guid,multihost,readonly,checkpoint "$p"
    rec "$x-checkpoint-discard" zpool checkpoint -d -w "$p"
    slow_on "$p"
    rec "$x-scrub" zpool scrub "$p"
    catch "$x-scrub-status" 'scrub in progress' 100 zpool status -P "$p"
    catch "$x-scrub-status-g" 'scrub in progress' 100 zpool status -P -g "$p"
    lim 600 zpool wait -t scrub "$p" > /dev/null 2>&1
    rec "$x-scrub-again" zpool scrub "$p"
    rec "$x-scrub-pause" zpool scrub -p "$p"
    status "$x-scrub-paused" "$p"
    rec "$x-scrub-stop" zpool scrub -s "$p"
    status "$x-scrub-canceled" "$p"
    slow_off "$p"
}

# catch_status NAME PATTERN POOL: the pair during a state, if caught.
catch_status() {
    catch "$1" "$2" 100 zpool status -P "$3"
    catch "$1-g" "$2" 100 zpool status -P -g "$3"
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
    # The adapter's locale (src/host.rs).
    export LC_ALL=C
    export DEBIAN_FRONTEND=noninteractive
    LOOPS=()
    # shellcheck disable=SC2317,SC2329  # reached through the trap
    cleanup() {
        set +e
        local p l
        [ -n "$ZINJECT" ] && lim 30 "$ZINJECT" -c all > /dev/null 2>&1
        for p in skzq skzqm skzqr; do lim 60 zpool destroy -f "$p" > /dev/null 2>&1; done
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
    rec mirror-online zpool online skzqm "$WORK/m1.img" &
    catch_status mirror-resilver-status 'resilver in progress' skzqm
    wait
    lim 600 zpool wait -t resilver skzqm > /dev/null 2>&1
    status mirror-resilvered skzqm

    rec mirror-rebuild zpool attach -s skzqm "$WORK/m0.img" "$WORK/m2.img" &
    catch_status mirror-rebuild-status 'in progress' skzqm
    wait
    lim 600 zpool wait -t resilver skzqm > /dev/null 2>&1
    lim 600 zpool wait -t scrub skzqm > /dev/null 2>&1
    slow_off skzqm
    status mirror-rebuilt skzqm

    status remove-status skzqr
    rec remove zpool remove skzqr "$WORK/r1.img"
    catch_status remove-in-progress 'in progress' skzqr
    lim 600 zpool wait -t remove skzqr > /dev/null 2>&1
    status remove-done skzqr

    # A pool that does not exist.
    agent_commands none skznone
    program none-members skznone members.lua -n skznone t
    program none-tagged skznone tagged.lua -n skznone "$TAG"
    rec none-holds zfs holds -H skznone@t
    rec none-holds-r zfs holds -H -r skznone@t
    rec none-hold zfs hold -r "$TAG" skznone@t
    rec none-release zfs release -r "$TAG" skznone@t
    exit 0
}

# Sourced with ZCR_LIB=1 (the unit test), only the functions above.
[ "${ZCR_LIB:-}" = 1 ] || main "$@"

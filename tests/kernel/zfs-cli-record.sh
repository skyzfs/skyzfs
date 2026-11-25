#!/usr/bin/env bash
# Recorded outputs of the ZFS commands the agent's ZFS adapter runs
# (skyzfs-agent, crates/skyzfs-agent/src/zfs/cli/), for its fixtures:
# the M2.6 plan's Q10. It judges nothing.
#
#   zfs-cli-record.sh OUT LUA_DIR [WORK]
#
# LUA_DIR holds snapshot.lua, members.lua and tagged.lua, the programs
# as the adapter pipes them to `zfs program`. Each command is one
# record, OUT/NAME/ with:
#
#   cmd     the command line, quoted for bash, with "< FILE" for stdin
#   stdout  what it wrote to stdout
#   stderr  what it wrote to stderr
#   rc      its exit code (124: it did not end within the limit)
#   note    only where there is something to say (a state looked for
#           and not caught)
#
# and one line "NAME<TAB>rc<TAB>note<TAB>cmd" of OUT/index.tsv, in the
# order run. NAME is "<pool>-<what>" with pool `rig` (skzq: the data on
# /dev/ublkb0 of the reference ublk server, with a log device), `mirror`
# (skzqm: a mirror of two files) or `none` (skznone, a pool that does
# not exist).
#
# Each pool gets a child file system, a volume, the assist's property
# set on its root as the agent sets it (skyzfs:assist) and one user
# property set locally on the child (skyzfs:note), before anything is
# recorded on it. In order, on each pool:
#
#   status, status-g       zpool status -P [-g], idle
#   get                    zpool get -H -p -o property,value guid,
#                          load_guid,multihost,readonly,checkpoint
#   snapshot-1, snapshot-2 snapshot.lua twice (the second: EEXIST);
#                          snapshot-cli only if neither took @t
#   members, members-absent, tagged
#                          members.lua at t and at a t that is absent;
#                          tagged.lua before any hold
#   hold-1, holds, holds-r, hold-2, tagged-held, release-1, release-2
#                          the holds, and each twice for its message
#   checkpoint, get-checkpoint, checkpoint-discard
#                          the same zpool get with a checkpoint
#   scrub, scrub-status, scrub-status-g
#                          during a scrub, if caught (note otherwise)
#   scrub-again, scrub-pause, scrub-paused, scrub-stop, scrub-canceled
#                          zpool status -P after `zpool scrub -p`, and
#                          after `zpool scrub -s`
#
# then, since they change the pools, the resilvers:
#
#   attach or offline, online
#   resilver-status, resilver-status-g
#                          during a resilver, if caught: the rig pool's
#                          device attached to /dev/ublkb1, the mirror's
#                          second file brought back online after writes
#   resilvered             zpool status -P once it ended
#
# and once: `nocacheflush` (the module parameter) and `version`. On the
# pool that does not exist, every command of the agent's (status,
# status-g, get, the three programs, holds, holds-r, hold, release).
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

# rec NAME [--in FILE] COMMAND...: run COMMAND once and keep it as the
# record NAME.
rec() {
    local name=$1 in=/dev/null shown d rc=0
    shift
    if [ "${1:-}" = --in ]; then
        in=$2
        shift 2
    fi
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
    lim "$LIMIT" "$@" < "$in" > "$d/stdout" 2> "$d/stderr" || rc=$?
    echo "$rc" > "$d/rc"
    printf '%s\t%s\t%s\t%s\n' "$name" "$rc" "" "$shown" >> "$OUT/index.tsv"
    printf '== %s: rc %s\n' "$name" "$rc"
}

# catch NAME PATTERN TRIES COMMAND...: run COMMAND up to TRIES times,
# 0.1 s apart, and keep the first output whose stdout matches PATTERN
# (grep -E) as the record NAME. If none does, the last is kept, with a
# note that says so. Returns 0 when caught.
catch() {
    local name=$1 pattern=$2 tries=$3 i tmp rc
    shift 3
    tmp=$OUT/.catch
    for ((i = 1; i <= tries; i++)); do
        rm -rf "${tmp:?}"
        mkdir -p "$tmp"
        rc=0
        lim "$LIMIT" "$@" < /dev/null > "$tmp/stdout" 2> "$tmp/stderr" || rc=$?
        grep -Eq "$pattern" "$tmp/stdout" && break
        [ "$i" -lt "$tries" ] && sleep 0.1
    done
    keep "$name" "$tmp" "$rc" "$([ "$i" -le "$tries" ] && echo "caught at try $i" ||
        echo "not caught: no output of $tries matched /$pattern/; this is the last")" "$@"
    [ "$i" -le "$tries" ]
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
    echo "$rc" > "$d/rc"
    printf '%s\n' "$note" > "$d/note"
    printf '%s\t%s\t%s\t%s\n' "$name" "$rc" "$note" "$shown" >> "$OUT/index.tsv"
    printf '== %s: rc %s (%s)\n' "$name" "$rc" "$note"
}

program() { # name P LUA [-n] ARGS...: zfs program -j as the adapter runs it
    local name=$1 p=$2 lua=$3 ro=()
    shift 3
    if [ "${1:-}" = -n ]; then
        ro=(-n)
        shift
    fi
    rec "$name" --in "$LUA_DIR/$lua" zfs program "${ro[@]}" -j "${PROGRAM_LIMITS[@]}" "$p" - "$@"
}

# The agent's commands on pool P, records named PREFIX-...; t is the
# snapshot's name.
agent_commands() { # prefix pool: the commands the adapter runs, as it runs them
    local x=$1 p=$2
    rec "$x-status" zpool status -P "$p"
    rec "$x-status-g" zpool status -P -g "$p"
    rec "$x-get" zpool get -H -p -o property,value guid,load_guid,multihost,readonly,checkpoint "$p"
    program "$x-snapshot-1" "$p" snapshot.lua "$p" t skyzfs:assist
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
    export DEBIAN_FRONTEND=noninteractive
    SLOG=""
    # shellcheck disable=SC2317,SC2329  # reached through the trap
    cleanup() {
        set +e
        local p
        for p in skzq skzqm; do lim 60 zpool destroy -f "$p" > /dev/null 2>&1; done
        command -v ublk > /dev/null && lim 60 ublk del -a > /dev/null 2>&1
        [ -n "$SLOG" ] && losetup -d "$SLOG" 2> /dev/null
        rm -rf "${OUT:?}/.catch"
        chmod -R a+rX "$OUT"
    }
    trap cleanup EXIT

    setup_failed() { echo "setup: $*" | tee "$OUT/setup-failed.txt" >&2; exit 2; }

    apt-get update > /dev/null 2>&1
    apt-get install -y --no-install-recommends zfsutils-linux > "$OUT/apt.log" 2>&1
    [ -e /dev/ublk-control ] || modprobe ublk_drv 2>> "$OUT/apt.log" || {
        apt-get install -y --no-install-recommends "linux-modules-extra-$(uname -r)" >> "$OUT/apt.log" 2>&1
        modprobe ublk_drv 2>> "$OUT/apt.log"
    }
    modprobe zfs 2>> "$OUT/apt.log"
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
        echo "lua from ${LUA_REF:-$LUA_DIR}"
        tail -n 1 "$OUT/ublksrv.ver"
    } > "$OUT/about.txt"
    cp "$LUA_DIR"/*.lua "$OUT/"

    rec version zfs version
    rec nocacheflush cat /sys/module/zfs/parameters/zfs_nocacheflush

    # The rig pool: data on /dev/ublkb0, a log on a loop device; ublkb1 for
    # the resilver.
    for n in 0 1; do
        truncate -s 4G "$WORK/dev$n.img"
        lim 60 ublk add -t loop -n "$n" -f "$WORK/dev$n.img" -q 1 -d 64 > "$OUT/ublk-add-$n.log" 2>&1 ||
            setup_failed "ublk add $n (ublk-add-$n.log)"
    done
    for _ in $(seq 1 40); do [ -b /dev/ublkb0 ] && [ -b /dev/ublkb1 ] && break; sleep 0.25; done
    [ -b /dev/ublkb0 ] && [ -b /dev/ublkb1 ] || setup_failed "no /dev/ublkb0 or ublkb1"
    truncate -s 512M "$WORK/slog.img"
    SLOG=$(losetup --find --show "$WORK/slog.img") || setup_failed "losetup of the log"
    truncate -s 4G "$WORK/m0.img" "$WORK/m1.img"

    make_pool() { # pool vdevs...
        local p=$1
        shift
        lim 180 zpool create -f -o ashift=12 -o cachefile=none -O mountpoint="$WORK/mnt/$p" -O compression=off \
            "$p" "$@" > "$OUT/create-$p.log" 2>&1 || setup_failed "zpool create $p (create-$p.log)"
        {
            zfs create "$p/child" && zfs create -V 256M "$p/vol" && zfs set skyzfs:assist=1700000000000 "$p" &&
                zfs set skyzfs:note=local "$p/child" &&
                # Enough data that a scrub and a resilver take a moment.
                "$DD" if=/dev/urandom of="$WORK/mnt/$p/child/data" bs=1M count=1024 conv=fsync status=none &&
                zpool sync "$p"
        } >> "$OUT/create-$p.log" 2>&1 || setup_failed "the datasets of $p (create-$p.log)"
    }
    make_pool skzq /dev/ublkb0 log "$SLOG"
    make_pool skzqm mirror "$WORK/m0.img" "$WORK/m1.img"
    echo "set up" >> "$OUT/about.txt"

    record_pool() { # prefix pool
        local x=$1 p=$2
        agent_commands "$x" "$p"
        program "$x-snapshot-2" "$p" snapshot.lua "$p" t skyzfs:assist
        # Without the program, a snapshot for the holds all the same.
        zfs list -H -o name "$p@t" > /dev/null 2>&1 || rec "$x-snapshot-cli" zfs snapshot -r "$p@t"
        program "$x-members" "$p" members.lua -n "$p" t
        program "$x-members-absent" "$p" members.lua -n "$p" absent
        program "$x-tagged" "$p" tagged.lua -n "$p" skyzfs
        rec "$x-hold-1" zfs hold -r skyzfs "$p@t"
        rec "$x-holds" zfs holds -H "$p@t"
        rec "$x-holds-r" zfs holds -H -r "$p@t"
        rec "$x-hold-2" zfs hold -r skyzfs "$p@t"
        program "$x-tagged-held" "$p" tagged.lua -n "$p" skyzfs
        rec "$x-release-1" zfs release -r skyzfs "$p@t"
        rec "$x-release-2" zfs release -r skyzfs "$p@t"
        rec "$x-checkpoint" zpool checkpoint "$p"
        rec "$x-get-checkpoint" zpool get -H -p -o property,value guid,load_guid,multihost,readonly,checkpoint "$p"
        rec "$x-checkpoint-discard" zpool checkpoint -d -w "$p"
        rec "$x-scrub" zpool scrub "$p"
        catch "$x-scrub-status" 'scrub in progress' 100 zpool status -P "$p"
        catch "$x-scrub-status-g" 'scrub in progress' 100 zpool status -P -g "$p"
        lim 600 zpool wait -t scrub "$p" > /dev/null 2>&1
        rec "$x-scrub-again" zpool scrub "$p"
        rec "$x-scrub-pause" zpool scrub -p "$p"
        rec "$x-scrub-paused" zpool status -P "$p"
        rec "$x-scrub-stop" zpool scrub -s "$p"
        rec "$x-scrub-canceled" zpool status -P "$p"
    }
    record_pool rig skzq
    record_pool mirror skzqm

    # The resilvers last: they change the pools.
    rec rig-attach zpool attach -f skzq /dev/ublkb0 /dev/ublkb1
    catch rig-resilver-status 'resilver in progress' 100 zpool status -P skzq
    catch rig-resilver-status-g 'resilver in progress' 100 zpool status -P -g skzq
    lim 600 zpool wait -t resilver skzq > /dev/null 2>&1
    rec rig-resilvered zpool status -P skzq
    rec mirror-offline zpool offline skzqm "$WORK/m1.img"
    "$DD" if=/dev/urandom of="$WORK/mnt/skzqm/child/more" bs=1M count=512 conv=fsync status=none
    zpool sync skzqm
    rec mirror-online zpool online skzqm "$WORK/m1.img"
    catch mirror-resilver-status 'resilver in progress' 100 zpool status -P skzqm
    catch mirror-resilver-status-g 'resilver in progress' 100 zpool status -P -g skzqm
    lim 600 zpool wait -t resilver skzqm > /dev/null 2>&1
    rec mirror-resilvered zpool status -P skzqm

    # A pool that does not exist.
    agent_commands none skznone
    program none-members skznone members.lua -n skznone t
    program none-tagged skznone tagged.lua -n skznone skyzfs
    rec none-holds zfs holds -H skznone@t
    rec none-holds-r zfs holds -H -r skznone@t
    rec none-hold zfs hold -r skyzfs skznone@t
    rec none-release zfs release -r skyzfs skznone@t
    exit 0
}

# Sourced with ZCR_LIB=1 (the unit test), only the functions above.
[ "${ZCR_LIB:-}" = 1 ] || main "$@"

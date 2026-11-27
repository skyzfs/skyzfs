#!/usr/bin/env bash
# Unit tests for zfs-cli-record.sh's records (rec, catch, keep, program):
# no kernel, no ZFS. Fake commands stand in for zpool and zfs.
#
#   bash tests/kernel/test_zfs_cli_record.sh
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
T=$(mktemp -d)
trap 'rm -rf "${T:?}"' EXIT
fail=0
check() { # what got want
    if [ "$2" = "$3" ]; then
        echo "ok: $1"
    else
        echo "FAIL: $1: got '$2', want '$3'"
        fail=1
    fi
}

ZCR_LIB=1
# shellcheck source=tests/kernel/zfs-cli-record.sh
. "$HERE/zfs-cli-record.sh"
OUT=$T/out LUA_DIR=$T/lua
mkdir -p "$OUT" "$LUA_DIR"
: > "$OUT/index.tsv"

# A command's stdout, stderr and exit code, each whole, and its line.
# shellcheck disable=SC2329 # run through rec, catch or program
said() { printf 'one\ntwo\n'; printf 'err\n' >&2; return 3; }
rec r-said said "a b" > /dev/null
check "stdout" "$(cat "$OUT/r-said/stdout")" $'one\ntwo'
check "stderr" "$(cat "$OUT/r-said/stderr")" "err"
check "rc" "$(cat "$OUT/r-said/rc")" 3
check "cmd quoted for bash" "$(cat "$OUT/r-said/cmd")" 'said a\ b'
check "no note" "$([ -e "$OUT/r-said/note" ] && echo yes || echo no)" no
check "index line" "$(sed -n 1p "$OUT/index.tsv")" $'r-said\t3\t\tsaid a\\ b'
check "argv unquoted" "$(tr '\0' '|' < "$OUT/r-said/argv")" "said|a b|"

# stdin from a file, named in cmd; none otherwise.
printf 'the program\n' > "$LUA_DIR/x.lua"
rec r-in --in "$LUA_DIR/x.lua" cat > /dev/null
check "stdin read" "$(cat "$OUT/r-in/stdout")" "the program"
check "stdin in cmd" "$(cat "$OUT/r-in/cmd")" "cat < x.lua"
rec r-noin cat > /dev/null
check "no stdin" "$(cat "$OUT/r-noin/stdout")" ""

# --expect: the note says whether stdout or stderr matched.
rec r-exp --expect '^err$' said > /dev/null
check "expected seen" "$(cat "$OUT/r-exp/note")" 'expected /^err$/: seen'
rec r-exp-no --expect 'no such tag' said > /dev/null
check "expected not seen" "$(cat "$OUT/r-exp-no/note")" 'expected /no such tag/: NOT SEEN'
check "expected in the index" "$(grep '^r-exp-no' "$OUT/index.tsv" | cut -f3)" 'expected /no such tag/: NOT SEEN'

# A name used twice is refused, and the first record stays.
rec r-said true > /dev/null 2>&1
check "twice refused" "$?" 70
check "first kept" "$(cat "$OUT/r-said/rc")" 3

# A command that does not end is 124, not a hang.
LIMIT=1
rec r-slow sleep 30 > /dev/null
check "limit" "$(cat "$OUT/r-slow/rc")" 124
LIMIT=5

# catch: the first matching output, with the try; the last otherwise.
# lim runs the command in a subshell, so the count lives in a file.
# shellcheck disable=SC2329 # run through rec, catch or program
counter() { local c; c=$(($(cat "$T/n" 2>/dev/null || echo 0) + 1)); echo "$c" > "$T/n"; echo "try $c"; [ "$c" -ge 3 ] && echo "scrub in progress"; return 0; }
catch c-yes 'scrub in progress' 10 counter > /dev/null
check "caught: rc" "$?" 0
check "caught at the third" "$(head -n 1 "$OUT/c-yes/stdout")" "try 3"
check "caught: note" "$(cat "$OUT/c-yes/note")" "caught at try 3"
check "caught: tries run" "$(cat "$T/n")" 3
check "caught: secs" "$(grep -Ec '^[0-9]+\.[0-9]{3}$' "$OUT/c-yes/secs")" 1
check "caught: argv unquoted" "$(tr '\0' '|' < "$OUT/c-yes/argv")" "counter|"
# The rc kept is the kept try's: not an earlier one's.
# shellcheck disable=SC2329 # run through catch
failing() { local c; c=$(($(cat "$T/m" 2>/dev/null || echo 0) + 1)); echo "$c" > "$T/m"; [ "$c" -ge 3 ] && { echo "scrub in progress"; return 0; }; return "$c"; }
catch c-rc 'scrub in progress' 10 failing > /dev/null
check "caught: the kept try's rc" "$(cat "$OUT/c-rc/rc")" 0
# shellcheck disable=SC2329 # run through catch
tries() { local c; c=$(($(cat "$T/k" 2>/dev/null || echo 0) + 1)); echo "$c" > "$T/k"; return "$c"; }
catch c-rc-no 'never' 4 tries > /dev/null
check "not caught: the last try's rc" "$(cat "$OUT/c-rc-no/rc")" 4
rm -f "$T/n"
catch c-no 'resilver in progress' 4 counter > /dev/null
check "not caught: rc" "$?" 1
check "not caught: the last kept" "$(head -n 1 "$OUT/c-no/stdout")" "try 4"
check "not caught: note" "$(cat "$OUT/c-no/note")" "not caught: no output of 4 matched /resilver in progress/; this is the last"
check "not caught: index note" "$(grep '^c-no' "$OUT/index.tsv" | cut -f3)" "not caught: no output of 4 matched /resilver in progress/; this is the last"
check "no scratch left" "$([ -e "$OUT/.catch" ] && echo yes || echo no)" no
# A name used twice: refused, and nothing left behind.
rm -f "$T/n"
catch c-yes 'scrub in progress' 10 counter > /dev/null 2>&1
check "catch twice: rc" "$?" 70
check "catch twice: no scratch left" "$([ -e "$OUT/.catch" ] && echo yes || echo no)" no
# A try that does not end is the last.
LIMIT=1
catch c-hang 'never' 3 sleep 30 > /dev/null
check "hung: rc" "$?" 1
check "hung: one try" "$(cat "$OUT/c-hang/note")" "not caught: try 1 did not end within 1 s; this is it"
check "hung: 124 kept" "$(cat "$OUT/c-hang/rc")" 124
LIMIT=5

# program: zfs program as the adapter runs it, the Lua on stdin.
# shellcheck disable=SC2329 # run through rec, catch or program
zfs() { printf '%s\n' "$*"; cat; }
printf -- '-- lua\n' > "$LUA_DIR/members.lua"
program p-ro skzq members.lua -n skzq t > /dev/null
check "program -n" "$(head -n 1 "$OUT/p-ro/stdout")" "program -n -j -t 100000000 -m 104857600 skzq - skzq t"
check "program stdin" "$(sed -n 2p "$OUT/p-ro/stdout")" "-- lua"
check "program cmd" "$(cat "$OUT/p-ro/cmd")" "zfs program -n -j -t 100000000 -m 104857600 skzq - skzq t < members.lua"
program p-rw skzq members.lua skzq t skyzfs:assist > /dev/null
check "program without -n" "$(head -n 1 "$OUT/p-rw/stdout")" "program -j -t 100000000 -m 104857600 skzq - skzq t skyzfs:assist"

# With a locale: env in front, and LC_ALL set for zfs alone. env runs
# the zfs on PATH, not a function: the fake is a script.
mkdir -p "$T/bin"
cat > "$T/bin/zfs" << 'FAKE'
#!/bin/sh
printf '%s %s\n' "${LC_ALL:-unset}" "$*"
FAKE
chmod +x "$T/bin/zfs"
PATH=$T/bin:$PATH
program p-loc skzq members.lua --locale C -n skzq t > /dev/null
check "program --locale" "$(cat "$OUT/p-loc/stdout")" "C program -n -j -t 100000000 -m 104857600 skzq - skzq t"
check "program --locale cmd" "$(cat "$OUT/p-loc/cmd")" "env LC_ALL=C zfs program -n -j -t 100000000 -m 104857600 skzq - skzq t < members.lua"
check "locale not left set" "${LC_ALL:-unset}" unset

# The index has one line a record, in order.
check "index order" "$(cut -f1 "$OUT/index.tsv" | tr '\n' ' ')" "r-said r-in r-noin r-exp r-exp-no r-slow c-yes c-rc c-rc-no c-no c-hang p-ro p-rw p-loc "

# The rebuild's pattern: a rebuild under way, not the scrub after it.
check "rebuild pattern: under way" "$(printf '  scan: resilver (mirror-0) in progress since Wed\n' | grep -Ec -- "$REBUILD_PATTERN")" 1
check "rebuild pattern: not the scrub after it" "$(printf '  scan: scrub in progress since Wed\n  scan: resilvered (mirror-0) 2G in 00:00:06\n' | grep -Ec -- "$REBUILD_PATTERN")" 0
check "rebuild pattern: not a healing resilver" "$(printf '  scan: resilver in progress since Wed\n' | grep -Ec -- "$REBUILD_PATTERN")" 0

# The commands as the adapter runs them (crates/skyzfs-agent, src/zfs.rs,
# zfs/cli.rs and src/host.rs), written out here: record_pool,
# record_none and scale on fake zpool and zfs that print the LC_ALL they
# were given and their words. A word the recorder changes fails here,
# at every call site: the rules below hold for each record.
#
# When #147's adapter changes a command (cli.rs, zfs.rs or host.rs),
# change it here and in zfs-cli-record.sh, dispatch
# skyzfs-zfs-cli-record.yml with lua_ref at that head, and refresh the
# adapter's recorded/*.json from that run.
unset -f zfs
cat > "$T/bin/zfs" << 'FAKE'
#!/bin/sh
# Every call, when a log is given: before the output, which can end
# the fake (SIGPIPE, as zfs send's into a recv that does not read).
[ -n "${FAKE_ZFS_CALLS:-}" ] && printf '%s\n' "$*" >> "$FAKE_ZFS_CALLS"
printf '%s %s\n' "${LC_ALL:-unset}" "$*"
[ "$1" = hold ] && [ -n "${FAKE_SLEEP:-}" ] && sleep "$FAKE_SLEEP"
[ "$1" = snapshot ] && [ -n "${FAKE_SNAP_SLEEP:-}" ] && sleep "$FAKE_SNAP_SLEEP"
# A snapshot call refused, when asked: from that name on.
[ "$1" = snapshot ] && [ -n "${FAKE_SNAP_FAIL:-}" ] && [ "$3" = "$FAKE_SNAP_FAIL" ] && { echo "cannot create snapshots : out of space" >&2; exit 1; }
# The sweep's list: the pool's @t and its child's with a hold (or
# FAKE_HELD names with one, the most a test needs), and @r1 with none.
case "$*" in
    "list -H -p -r -t snapshot -o name,userrefs "*)
        if [ -n "${FAKE_HELD:-}" ]; then
            i=1
            while [ "$i" -le "$FAKE_HELD" ]; do printf '%s@h%s\t1\n' "$9" "$i"; i=$((i + 1)); done
        else
            printf '%s@t\t%s\n%s/child@t\t1\n%s@r1\t0\n' "$9" "${FAKE_USERREFS:-2}" "$9" "$9"
        fi
        exit 0 ;;
esac
[ "$1" = list ] && exit "${FAKE_LIST_RC:-0}"
# What holds -r holds: the snapshot and its child's, the first twice
# (a second tag), after the words.
case "$*" in
    "holds -H -r "*@*)
        case ${FAKE_HOLDS_R:-} in
            none) exit 0 ;;
            one) printf '%s\tskyzfs-publish\tThu\n' "$4"; exit 0 ;;
        esac
        printf '%s\tskyzfs-publish\tThu\n%s\tother\tThu\n%s/child@%s\tskyzfs-publish\tThu\n' "$4" "$4" "${4%@*}" "${4#*@}"
        exit 0 ;;
esac
# The snapshot program's answer, when one is given.
case "$*" in
    program*skyzfs:assist) [ -n "${FAKE_PROGRAM_OUT:-}" ] && printf '%s\n' "$FAKE_PROGRAM_OUT" ;;
esac
exit 0
FAKE
cat > "$T/bin/zpool" << 'FAKE'
#!/bin/sh
# The pool's space, as retain reads it: FAKE_POOL_FREE bytes free (or
# FAKE_FREE_TEXT in place of the free alone), each read logged to
# FAKE_ZPOOL_LISTS, after FAKE_LIST_SLEEP seconds when set.
case "$*" in
    "list -Hp -o "*)
        [ -n "${FAKE_ZPOOL_LISTS:-}" ] && printf '%s\n' "$*" >> "$FAKE_ZPOOL_LISTS"
        [ -n "${FAKE_LIST_SLEEP:-}" ] && sleep "$FAKE_LIST_SLEEP"
        [ "$4" = free ] && echo "${FAKE_FREE_TEXT:-${FAKE_POOL_FREE:-34359738368}}" || printf '4096\t%s\n' "${FAKE_POOL_FREE:-34359738368}"
        exit 0 ;;
esac
printf '%s %s\n' "${LC_ALL:-unset}" "$*"
# Every call, with its time, when a log is given.
[ -n "${FAKE_ZPOOL_CALLS:-}" ] && printf '%s %s\n' "$(date +%s.%N)" "$*" >> "$FAKE_ZPOOL_CALLS"
echo "scrub in progress"
FAKE
# The scale pool's root is mounted only when FAKE_MOUNTED is set.
cat > "$T/bin/mountpoint" << 'FAKE'
#!/bin/sh
[ -n "${FAKE_MOUNTED:-}" ]
FAKE
# The image's size, logged to FAKE_TRUNCATE_CALLS, then made.
cat > "$T/bin/truncate" << FAKE
#!/bin/sh
[ -n "\${FAKE_TRUNCATE_CALLS:-}" ] && printf '%s\n' "\$*" >> "\$FAKE_TRUNCATE_CALLS"
exec $(command -v truncate) "\$@"
FAKE
chmod +x "$T/bin/zfs" "$T/bin/zpool" "$T/bin/mountpoint" "$T/bin/truncate"
adapter_env
# The sweep's rounds (sweep-rounds) only where a test asks for them.
export ZCR_SWEEP_ROUNDS=0
OUT=$T/pins
mkdir -p "$OUT"
: > "$OUT/index.tsv"
printf -- '-- lua\n' > "$LUA_DIR/snapshot.lua"
printf -- '-- lua\n' > "$LUA_DIR/tagged.lua"
WORK=$T/work
mkdir -p "$WORK"
SCALE_END=$(($(date +%s) + 600))
FAKE_ZFS_CALLS=$T/zfs.calls record_pool rig skzq > /dev/null 2> "$T/pins.err"
record_none none skznone > /dev/null 2>> "$T/pins.err"
FAKE_ZFS_CALLS=$T/zfs.calls record_reads read skzrd "$WORK/rd.img" > /dev/null 2>> "$T/pins.err"
export FAKE_LIST_RC=1 # the program took no @t: the CLI's
export FAKE_MOUNTED=1 ZCR_WRITER_MARGIN=0.3
# The fake kstat commits no group after the probe begins: no settle,
# except where a test asks for it.
export ZCR_SETTLE_TXGS=0
mkdir -p "$WORK/mnt/skzqs"
# The pool's txgs kstat and the module's parameters, as files: a
# dirty-data limit of 1 MiB (a span of 2 MiB) and the history off.
export ZCR_KSTAT_DIR=$T/kstat ZCR_ZFS_PARAMS=$T/params
mkdir -p "$ZCR_KSTAT_DIR/skzqs" "$ZCR_ZFS_PARAMS"
printf 'txg      birth            state ndirty       nread        nwritten     reads    writes   otime        qtime        wtime        stime\n' > "$ZCR_KSTAT_DIR/skzqs/txgs"
printf '7        %s     C     4096         0            8192         0        2        1000000000   1000         2000         300000000\n' \
    "$(python3 -c 'import time; print(time.clock_gettime_ns(time.CLOCK_MONOTONIC_RAW) - 5 * 10**9)')" >> "$ZCR_KSTAT_DIR/skzqs/txgs"
echo 1048576 > "$ZCR_ZFS_PARAMS/zfs_dirty_data_max"
echo 0 > "$ZCR_ZFS_PARAMS/zfs_txg_history"
export FAKE_ZPOOL_CALLS=$T/zpool.calls
FAKE_TRUNCATE_CALLS=$T/truncate.calls ZCR_SCALE_LIMIT=7 scale 3 > /dev/null 2>> "$T/pins.err"
# The same with 2 retained snapshots of each dataset (#157's R1), in
# its own directory.
mkdir -p "$T/pins-s"
: > "$T/pins-s/index.tsv"
OUT=$T/pins-s FAKE_TRUNCATE_CALLS=$T/truncate.calls FAKE_ZPOOL_CALLS=$T/zpool-s.calls FAKE_ZFS_CALLS=$T/zfs-s.calls ZCR_SCALE_LIMIT=7 scale 3 2 > /dev/null 2>> "$T/pins.err"
unset FAKE_LIST_RC FAKE_MOUNTED FAKE_ZPOOL_CALLS
check "pins: no name twice" "$(grep -c twice "$T/pins.err")" 0
words() { tr '\0' ' ' < "$OUT/$1/argv"; }
# order A B: yes when A's line of the index comes before B's, each looked
# up by itself (one grep for both would give them in the file's order).
line_of() { grep -n -m 1 "^$1	" "$OUT/index.tsv" | cut -d: -f1; }
order() {
    local a b
    a=$(line_of "$1")
    b=$(line_of "$2")
    [ -n "$a" ] && [ -n "$b" ] && [ "$a" -lt "$b" ] && echo yes || echo no
}
mkdir -p "$T/ord"
printf 'x\t0\t\tx\ny\t0\t\ty\n' > "$T/ord/index.tsv"
check "order: x before y, y not before x, a missing one not" "$(OUT=$T/ord order x y; OUT=$T/ord order y x; OUT=$T/ord order x z)" $'yes\nno\nno'
P="-j -t 100000000 -m 104857600"
check "pin status" "$(words rig-status)" "zpool status -P skzq "
check "pin status -g" "$(words rig-status-g)" "zpool status -P -g skzq "
check "pin get" "$(words rig-get)" "zpool get -H -p -o property,value guid,load_guid,multihost,readonly,checkpoint skzq "
check "pin assist set" "$(words rig-assist-set)" "zfs set skyzfs:assist=1700000000001 skzq "
check "pin assist sync" "$(words rig-assist-sync)" "zpool sync skzq "
check "pin snapshot" "$(words rig-snapshot-1)" "zfs program $P skzq - skzq t skyzfs:assist "
check "pin snapshot stdin" "$(sed 's/.* < //' "$OUT/rig-snapshot-1/cmd")" "snapshot.lua"
check "pin members" "$(words rig-members)" "zfs program -n $P skzq - skzq t "
check "pin tagged" "$(words rig-tagged)" "zfs program -n $P skzq - skzq skyzfs-publish "
check "pin hold" "$(words rig-hold-1)" "zfs hold -r skyzfs-publish skzq@t "
check "pin holds" "$(words rig-holds)" "zfs holds -H skzq@t "
check "pin holds -r" "$(words rig-holds-r)" "zfs holds -H -r skzq@t "
check "pin release" "$(words rig-release-1)" "zfs release -r skyzfs-publish skzq@t "
check "pin destroy" "$(words rig-destroy)" "zfs destroy -r skzq@o0 "
check "pin none's destroy" "$(words none-destroy)" "zfs destroy -r skznone@t "
check "pin none's take" "$(words none-snapshot-cli)" "zfs snapshot -r skznone@t "
check "pin the take at the child's @p" "$(words rig-cli-exists-child)" "zfs snapshot -r skzq@p "
check "pin the take at the root's @t" "$(words rig-cli-exists-root)" "zfs snapshot -r skzq@t "
check "pin the limit" "$(words rig-cli-limit-set)" "zfs set snapshot_limit=0 skzq/child "
check "pin the take under the limit" "$(words rig-cli-limit)" "zfs snapshot -r skzq@q "
check "pin the snapshots after each" "$(words rig-cli-exists-child-after)/$(words rig-cli-limit-after)" "zfs list -H -t snapshot -o name -r skzq /zfs list -H -t snapshot -o name -r skzq "
check "pin the odd one in C.UTF-8" "$(words rig-snapshot-odd-utf8)" "env LC_ALL=C.UTF-8 zfs program $P skzq - skzq o2 skyzfs:assist "
check "pin the odd one as the adapter" "$(words rig-snapshot-odd)" "zfs program $P skzq - skzq o0 skyzfs:assist "
check "pin none's assist" "$(words none-assist-set)" "zfs set skyzfs:assist=1700000000001 skznone "
check "pin none's pools named" "$(words none-pools-named)" "zpool list -H -o name skznone "
check "pin secs" "$(grep -Ec '^[0-9]+\.[0-9]{3}$' "$OUT/rig-status/secs")" 1
check "pin a catch's secs" "$(grep -Ec '^[0-9]+\.[0-9]{3}$' "$OUT/rig-scrub-status/secs")" 1
check "pin scale snapshot" "$(words scale-3-snapshot)" "zfs program $P skzqs - skzqs t skyzfs:assist "
check "pin scale's own snapshot" "$(words scale-3-snapshot-cli)" "zfs snapshot -r skzqs@t "
check "pin scale hold" "$(words scale-3-hold)" "zfs hold -r skyzfs-publish skzqs@t "
check "pin scale release" "$(words scale-3-release)" "zfs release -r skyzfs-publish skzqs@t "
check "pin scale destroy" "$(words scale-3-destroy)" "zfs destroy -r skzqs@t "
check "pin scale's take by the CLI" "$(words scale-3-take-cli)" "zfs snapshot -r skzqs@c "
check "pin scale's members after it" "$(words scale-3-take-cli-members)" "zfs program -n $P skzqs - skzqs c "
# The adapter's count and the members program's full mode (#153).
check "pin count" "$(words rig-count)" "zfs list -r -H -o name -t filesystem,volume skzq "
check "pin none's count" "$(words none-count)" "zfs list -r -H -o name -t filesystem,volume skznone "
check "pin scale's count" "$(words scale-3-count)" "zfs list -r -H -o name -t filesystem,volume skzqs "
check "pin members full" "$(words rig-members-full)" "zfs program -n $P skzq - skzq t full skyzfs:assist "
check "pin members full stdin" "$(sed 's/.* < //' "$OUT/rig-members-full/cmd")" "members.lua"
check "pin members full, absent" "$(words rig-members-full-absent)" "zfs program -n $P skzq - skzq absent full skyzfs:assist "
check "pin members full, odd" "$(words rig-members-full-odd)" "zfs program -n $P skzq - skzq o0 full skyzfs:assist "
check "pin members full, odd in C.UTF-8" "$(words rig-members-full-odd-utf8)" "env LC_ALL=C.UTF-8 zfs program -n $P skzq - skzq o2 full skyzfs:assist "
check "pin none's members full" "$(words none-members-full)" "zfs program -n $P skznone - skznone t full skyzfs:assist "
check "pin scale's members full after the take" "$(words scale-3-take-cli-full)" "zfs program -n $P skzqs - skzqs c full skyzfs:assist "
# The manifest's read by the CLI (M2.6c-5), at scale and on the odd
# values.
GET="zfs get -r -H -p -t filesystem,volume -s local -o name,property,value all"
LIST="zfs list -r -H -p -t filesystem,volume,snapshot -o name,guid,type,createtxg"
check "pin scale's get" "$(words scale-3-take-cli-get)" "$GET skzqs "
check "pin scale's get in JSON" "$(words scale-3-take-cli-get-json)" "zfs get -j -r -p -t filesystem,volume -s local -o name,property,value all skzqs "
check "pin scale's list" "$(words scale-3-take-cli-list)" "$LIST skzqs "
check "pin scale's count of the snapshots" "$(words scale-3-count-snapshots)" "zfs list -r -H -o name -t snapshot skzqs "
check "pin scale's holds" "$(words scale-3-holds)" "zfs holds -H -r skzqs@t "
check "scale's holds while held" "$(order scale-3-hold scale-3-holds)$(order scale-3-holds scale-3-release)" yesyes
# lines_said RECORD: RECORD's line in about.txt, its lines and its secs,
# found once.
lines_said() { grep -cxF "scale: $1: $(wc -l < "$OUT/$1/stdout") lines in $(cat "$OUT/$1/secs") s" "$OUT/about.txt"; }
check "lines said: holds" "$(lines_said scale-3-holds)" 1
check "lines said: with the seconds" "$(grep -Ec '^scale: scale-3-holds: [0-9]+ lines in [0-9]+\.[0-9]{3} s$' "$OUT/about.txt")" 1
# The sweep's two reads (M2.6c-5d): the list, then zfs holds -H on the
# snapshots it gives with a hold, in its order; with a snapshot
# destroyed between them, after the first and last.
SWL="zfs list -H -p -r -t snapshot -o name,userrefs"
check "pin sweep list" "$(words rig-sweep-list)/$(words rig-sweep-list-held)" "$SWL skzq /$SWL skzq "
check "pin sweep holds" "$(words rig-sweep-holds)" "zfs holds -H skzq@t skzq/child@t "
check "pin sweep gone" "$(words rig-sweep-gone-take)/$(words rig-sweep-gone-destroy)" "zfs snapshot -r skzq@gone /zfs destroy -r skzq@gone "
check "pin sweep holds, one gone" "$(words rig-sweep-holds-gone)" "zfs holds -H skzq@t skzq@gone skzq/child@t "
check "pin sweep holds, the last gone" "$(words rig-sweep-holds-gone-last)" "zfs holds -H skzq@t skzq/child@t skzq@gone "
check "pin sweep odd tag" "$(words rig-sweep-hold-odd)/$(words rig-sweep-release-odd)" "zfs hold $ODD_TAG skzq@t /zfs release $ODD_TAG skzq@t "
check "the odd tag's bytes" "$(printf '%s' "$ODD_TAG" | od -An -tx1 | tr -d ' ')" 636166e9
check "pin sweep reads with the odd tag" "$(words rig-sweep-list-odd)/$(words rig-sweep-holds-odd)" "$SWL skzq /zfs holds -H skzq@t skzq/child@t "
check "sweep: the odd tag after the gone, let go before the release" \
    "$(order rig-sweep-holds-gone-last rig-sweep-hold-odd)$(order rig-sweep-hold-odd rig-sweep-list-odd)$(order rig-sweep-list-odd rig-sweep-holds-odd)$(order rig-sweep-holds-odd rig-sweep-release-odd)$(order rig-sweep-release-odd rig-release-1)" \
    yesyesyesyesyes
check "sweep: the odd tag's cmd quoted" "$(cat "$OUT/rig-sweep-hold-odd/cmd")" "zfs hold \$'caf\\351' skzq@t"
check "sweep: the list before any hold, the rest while held" \
    "$(order rig-sweep-list rig-hold-1)$(order rig-hold-1 rig-sweep-list-held)$(order rig-sweep-list-held rig-sweep-holds)$(order rig-sweep-holds rig-sweep-gone-take)$(order rig-sweep-gone-take rig-sweep-gone-destroy)$(order rig-sweep-gone-destroy rig-sweep-holds-gone)$(order rig-sweep-holds-gone rig-sweep-holds-gone-last)$(order rig-sweep-holds-gone-last rig-release-1)" \
    yesyesyesyesyesyesyesyes
# The agent's read of the counts after holds -r (#165): the names it
# gave, once each, in its order; then with the middle of @c's destroyed
# after its holds read, let go first.
CL="zfs list -H -p -o name,userrefs -t snapshot"
check "pin counts" "$(words rig-holds-r-counts)" "$CL skzq@t skzq/child@t "
check "pin counts on @c" "$(words rig-counts-take)/$(words rig-counts-hold)/$(words rig-counts-holds-r)" \
    "zfs snapshot -r skzq@c /zfs hold -r skyzfs-publish skzq@c /zfs holds -H -r skzq@c "
check "pin counts, one gone" "$(words rig-counts-release-one)/$(words rig-counts-destroy-one)/$(words rig-holds-r-counts-gone)/$(words rig-counts-destroy)" \
    "zfs release skyzfs-publish skzq/child@c /zfs destroy skzq/child@c /$CL skzq@c skzq/child@c /zfs destroy -r skzq@c "
# The take's holds let go before it is destroyed (K04 of #169's review):
# release_quietly lists the pool's snapshots (the fake lists none @c).
check "counts: released before the destroy" "$(awk '$0 == "holds -H -r skzq@c" { h = NR } h && $0 == "list -H -o name -t snapshot -r skzq" && !r { r = NR } h && $0 == "destroy -r skzq@c" { d = NR } END { print (h && r > h && d > r) ? "yes" : h " " r " " d }' "$T/zfs.calls")" yes
# What release_quietly lets go: the tag's hold on each snapshot of the
# name, on the pool and its children, and on no other (P18 of #169's
# review: the course's fake lists none @c).
check "release_quietly: each snapshot of the name, and only those" "$(
    zfs() { echo "$*" >> "$T/rq.calls"; [ "$1" != list ] || printf 'skzq@c\nskzq/child@c\nskzq@t\nskzq@cc\nskzq/c@x\nskzq@c2\n'; }
    rm -f "$T/rq.calls"; release_quietly skzq c; tr '\n' ';' < "$T/rq.calls")" \
    "list -H -o name -t snapshot -r skzq;release skyzfs-publish skzq@c;release skyzfs-publish skzq/child@c;"
check "counts: the gone one said" "$(grep -c '^rig: rig-holds-r-counts-gone names skzq/child@c, destroyed after rig-counts-holds-r$' "$OUT/about.txt")" 1
check "counts: after holds-r, in order, before hold-2" \
    "$(order rig-holds-r rig-holds-r-counts)$(order rig-holds-r-counts rig-counts-take)$(order rig-counts-take rig-counts-hold)$(order rig-counts-hold rig-counts-holds-r)$(order rig-counts-holds-r rig-counts-release-one)$(order rig-counts-release-one rig-counts-destroy-one)$(order rig-counts-destroy-one rig-holds-r-counts-gone)$(order rig-holds-r-counts-gone rig-counts-destroy)$(order rig-counts-destroy rig-hold-2)" \
    yesyesyesyesyesyesyesyesyes
# Too few names: no counts at all, or none with one gone.
cnt_t() { # FAKE_HOLDS_R
    local d=$T/cnt-$1
    rm -rf "${d:?}"; mkdir -p "$d"; : > "$d/index.tsv"
    OUT=$d FAKE_HOLDS_R=$1 rec rig-holds-r zfs holds -H -r skzq@t > /dev/null
    OUT=$d FAKE_HOLDS_R=$1 holds_counts rig skzq > /dev/null 2>&1
    echo "$(cut -f1 "$d/index.tsv" | tr '\n' ' ')|$(tr '\n' ';' < "$d/about.txt")"
}
check "counts: holds -r names none" "$(cnt_t none)" \
    "rig-holds-r rig-counts-take rig-counts-hold rig-counts-holds-r rig-counts-destroy |rig: the counts not read: rig-holds-r names no snapshot;rig: the counts with one gone not read: rig-counts-holds-r names 0 snapshots;"
check "counts: holds -r names one" "$(cnt_t one)" \
    "rig-holds-r rig-holds-r-counts rig-counts-take rig-counts-hold rig-counts-holds-r rig-counts-destroy |rig: the counts with one gone not read: rig-counts-holds-r names 1 snapshots;"
# Another holder's tags with a tab and a newline on @t, read, let go.
check "pin odd tags" "$(words rig-hold-tab)/$(words rig-holds-odd-tags)/$(words rig-holds-r-odd-tags)/$(words rig-release-nl)" \
    "zfs hold $TAB_TAG skzq@t /zfs holds -H skzq@t /zfs holds -H -r skzq@t /zfs release $NL_TAG skzq@t "
check "odd tags: the bytes" "$(printf '%q|%q' "$TAB_TAG" "$NL_TAG")" "\$'other\\tholder'|\$'other\\nholder'"
check "odd tags: in order, after the release, before holds-absent" \
    "$(order rig-release-2 rig-hold-tab)$(order rig-hold-tab rig-hold-nl)$(order rig-hold-nl rig-holds-odd-tags)$(order rig-holds-odd-tags rig-holds-r-odd-tags)$(order rig-holds-r-odd-tags rig-release-tab)$(order rig-release-tab rig-release-nl)$(order rig-release-nl rig-holds-absent)" \
    yesyesyesyesyesyesyes
check "pin none's sweep" "$(words none-sweep-list)/$(words none-sweep-holds)" "$SWL skznone /zfs holds -H skznone@t skznone@u "
check "pin scale's sweep" "$(words scale-3-sweep-list)/$(words scale-3-sweep-holds)" "$SWL skzqs /zfs holds -H skzqs@t skzqs/child@t "
check "scale's sweep while held" "$(order scale-3-hold scale-3-sweep-list)$(order scale-3-sweep-list scale-3-sweep-holds)$(order scale-3-sweep-holds scale-3-release)" yesyesyes
check "scale's sweep with the load beside it" "$([ -s "$OUT/scale-3-sweep-list/writer.tsv" ] && [ -s "$OUT/scale-3-sweep-holds/writer.tsv" ] && echo yes || echo no)" yes
check "lines said: sweep list, sweep holds" "$(lines_said scale-3-sweep-list)$(lines_said scale-3-sweep-holds)" 11
# held_names: a hold count above 0 of a snapshot alone; the header line
# of the fake, a count of 0, a dataset and a word are not.
printf 'C list -H\na@t\t1\nb@t\t0\nc\t3\nd@t\tx\ne@t\t12\nf@t\t1\textra\n' > "$T/held.txt"
check "held_names" "$(held_names "$T/held.txt" | tr '\n' ' ')" "a@t e@t "
# No snapshot with a hold: no holds read, said.
mkdir -p "$T/nohold"
: > "$T/nohold/index.tsv"
OUT=$T/nohold FAKE_USERREFS=0 FAKE_HELD=0 record_pool rig skzq > /dev/null 2> "$T/nohold.err"
check "no hold: the holds not read" "$(find "$T/nohold" -mindepth 1 -maxdepth 1 -name 'rig-sweep-*' -printf '%f\n' | sort | tr '\n' ' ')" "rig-sweep-list rig-sweep-list-held "
check "no hold: no odd tag either" "$(grep -c 'caf' "$T/nohold/index.tsv")" 0
check "no hold: said" "$(grep -cx 'rig: the sweep.s holds not read: rig-sweep-list-held lists no snapshot with a hold' "$T/nohold/about.txt")" 1
# At scale, HOLDS_AT_ONCE names at most, the first of the list's.
mkdir -p "$T/many"
: > "$T/many/index.tsv"
OUT=$T/many FAKE_HELD=1002 FAKE_LIST_RC=1 ZCR_SCALE_LIMIT=7 scale 2 > /dev/null 2> "$T/many.err"
check "scale's sweep: 1000 names at most" "$(tr '\0' '\n' < "$T/many/scale-2-sweep-holds/argv" | grep -c '@h')" 1000
check "scale's sweep: the first" "$(tr '\0' '\n' < "$T/many/scale-2-sweep-holds/argv" | sed -n '4p;1003p' | tr '\n' ' ')" "skzqs@h1 skzqs@h1000 "
mkdir -p "$T/nohs"
: > "$T/nohs/index.tsv"
OUT=$T/nohs FAKE_HELD=0 FAKE_LIST_RC=1 ZCR_SCALE_LIMIT=7 scale 2 > /dev/null 2> "$T/nohs.err"
check "scale's sweep: none held, said" "$([ -e "$T/nohs/scale-2-sweep-holds" ] && echo yes || echo no)$(grep -cx "scale 2: the sweep's holds not read: its list gives no snapshot with a hold" "$T/nohs/about.txt")" no1
check "scale's counts, then the snapshot" "$(order scale-3-count scale-3-count-snapshots)$(order scale-3-count-snapshots scale-3-snapshot)" yesyes
check "lines said: count-snapshots" "$(lines_said scale-3-count-snapshots)" 1
check "pin scale's get of the pairs" "$(words scale-3-take-cli-get-pairs)" "zfs get -r -H -p -t filesystem,volume -s local -o name,property all skzqs "
check "scale's reads after the take, in order" "$(order scale-3-take-cli scale-3-take-cli-members)$(order scale-3-take-cli-members scale-3-take-cli-get)$(order scale-3-take-cli-get scale-3-take-cli-get-pairs)$(order scale-3-take-cli-get-pairs scale-3-take-cli-get-json)$(order scale-3-take-cli-get-json scale-3-take-cli-list)$(order scale-3-take-cli-list scale-3-take-cli-full)" yesyesyesyesyesyes
check "scale's get in JSON: no load beside it" "$([ -e "$OUT/scale-3-take-cli-get-json/writer.tsv" ] && echo yes || echo no)" no
# With retained snapshots: the records named scale-N-sK, the snapshots
# taken one name a call before the take, and said.
check "scale 3:2: its reads" "$(OUT=$T/pins-s words scale-3-s2-take-cli-list)/$(OUT=$T/pins-s words scale-3-s2-take-cli-get-pairs)" \
    "$LIST skzqs /zfs get -r -H -p -t filesystem,volume -s local -o name,property all skzqs "
check "scale 3:2: the retained snapshots, then the take" \
    "$(grep -n '^snapshot ' "$T/zfs-s.calls" | grep -v '@t$' | tr '\n' '|')" \
    "$(printf '%s|' "$(grep -n '^snapshot -r skzqs@r1$' "$T/zfs-s.calls")" "$(grep -n '^snapshot -r skzqs@r2$' "$T/zfs-s.calls")" "$(grep -n '^snapshot -r skzqs@c$' "$T/zfs-s.calls")")"
check "scale 3:2: the snapshots before the take" "$(grep -n '^snapshot ' "$T/zfs-s.calls" | grep -v '@t$' | head -n 2 | cut -d: -f2- | tr '\n' '|')" "snapshot -r skzqs@r1|snapshot -r skzqs@r2|"
check "scale 3:2: said" "$(grep -c '^scale 3-s2: [0-9]* snapshots in the pool (2 of each of 4 datasets asked, one name a call) made in [0-9]* s$' "$T/pins-s/about.txt")" 1
check "scale 3:2: the count lists every dataset's snapshots" "$(grep -c '^list -H -t snapshot -o name -r skzqs$' "$T/zfs-s.calls")" 1
check "scale 3: none retained, none said" "$(grep -c '^scale 3: .* datasets asked' "$OUT/about.txt")" 0
check "scale: a pool of 16 GiB, and of 32 GiB with K" "$(cut -d' ' -f1,2 "$T/truncate.calls" | tr '\n' '|')" "-s 16G|-s 32G|"
check "scale 3:2: the K reached said" "$(grep -cx 'scale 3-s2: K reached: 2 of the 2 asked' "$T/pins-s/about.txt")" 1
for r in take-cli-get take-cli-get-pairs take-cli-list; do
    check "lines said: $r" "$(lines_said "scale-3-$r")" 1
done
for k in $ODD_KINDS; do
    check "odd $k: set on its own file system" "$(words "rig-odd-set-$k" | tr '\n' '|')" "zfs set skyzfs:v=$(odd_value "$k" skzq | tr '\n' '|') skzq/odd-$k "
    check "odd $k: made before its set" "$(grep -n -m 1 "^create -u skzq/odd-$k$" "$T/zfs.calls" | cut -d: -f1 | sed 's/^[0-9][0-9]*$/made/')" made
    check "odd $k: its value alone" "$(words "rig-odd-value-$k")" "zfs get -H -p -o value skyzfs:v skzq/odd-$k "
    check "odd $k: its value alone in JSON" "$(words "rig-odd-json-$k")" "zfs get -j -p -o name,property,value skyzfs:v skzq/odd-$k "
    check "odd $k: set before the reads" "$(order "rig-odd-set-$k" rig-odd-get-local)" yes
    check "odd $k: destroyed after" "$(grep -c "^destroy -r skzq/odd-$k$" "$T/zfs.calls")" 1
done
check "odd: fourteen kinds" "$(wc -w <<< "$ODD_KINDS")" 14
# The manifest's read by the CLI (M2.6c-5) on skzrd, in each state, as
# coding asked: the commands, LC_ALL=C, the states in order.
for st in tree not odd counts none; do
    check "read $st: list" "$(words "read-$st-list")" "$LIST skzrd "
    check "read $st: names" "$(words "read-$st-get-names")" \
        "zfs get -r -H -p -s local -t filesystem,volume -o name,property all skzrd "
    check "read $st: values" "$(words "read-$st-get-values")" \
        "zfs get -r -H -p -s local -t filesystem,volume -o name,property,value all skzrd "
    for r in list get-names get-values; do
        check "read $st $r: LC_ALL=C" "$(cut -d' ' -f1 "$OUT/read-$st-$r/stdout")" C
    done
done
check "read: the states in order" \
    "$(order read-tree-get-values read-not-list)$(order read-not-get-values read-odd-list)$(order read-odd-get-values read-counts-list)$(order read-counts-get-values read-none-list)" yesyesyesyes
check "read tree: @t, then late made" "$(grep -e '^snapshot -r skzrd@t$' -e '^create skzrd/late$' "$T/zfs.calls" | tr '\n' '|')" \
    "snapshot -r skzrd@t|create skzrd/late|snapshot -r skzrd@t|snapshot -r skzrd@t|"
check "read: a value set" "$(words read-value-set)" "zfs get -H -p -o value skyzfs:note skzrd/child "
check "read: a value unset" "$(words read-value-unset)" "zfs get -H -p -o value skyzfs:unset skzrd/child "
check "read: a dataset missing" "$(words read-value-missing)" "zfs get -H -p -o value skyzfs:note skzrd/missing "
check "read not: @t destroyed between" "$(grep -c '^destroy -r skzrd@t$' "$T/zfs.calls")" 2
for k in $READ_KINDS; do
    check "read odd $k: set" "$(words "read-odd-set-$k" | tr '\n' '|')" "zfs set skyzfs:v=$(odd_value "$k" skzrd | tr '\n' '|') skzrd/odd-$k "
    check "read odd $k: its value alone" "$(words "read-odd-value-$k")" "zfs get -H -p -o value skyzfs:v skzrd/odd-$k "
    check "read odd $k: destroyed before the counts" "$(grep -c "^destroy -r skzrd/odd-$k$" "$T/zfs.calls")" 1
done
check "read odd: the kinds coding asked for" "$READ_KINDS" "nl tab utf8 raw forge empty dash cr spaces bslash len8191"
check "read counts: the limits" "$(words read-counts-set)" "zfs set filesystem_limit=100 snapshot_limit=100 skzrd/child "
check "read counts: the counts' values" "$(words read-counts-fs)$(words read-counts-snap)" \
    "zfs get -H -p -o value filesystem_count skzrd/child zfs get -H -p -o value snapshot_count skzrd/child "
check "read none: the pool destroyed before" "$(grep -c 'destroy -f skzrd' "$OUT/create-skzrd.log")" 1
check "read none: a value" "$(words read-none-value)" "zfs get -H -p -o value skyzfs:note skzrd/child "
check "odd: empty is no byte" "$(odd_value empty skzq | wc -c)" 0
check "odd: dash" "$(odd_value dash skzq | od -An -c | tr -s ' ')" " -"
check "odd: a carriage return" "$(odd_value cr skzq | od -An -c | tr -s ' ')" " o n e \r t w o"
check "odd: a space at each end" "$(odd_value spaces skzq | od -An -tx1 | tr -s ' ')" " 20 6f 6e 65 20 74 77 6f 20"
check "odd: a backslash and an n, not a newline" "$(odd_value bslash skzq | od -An -tx1 | tr -s ' ')" " 6f 6e 65 5c 6e 74 77 6f"
check "odd: the newline's bytes" "$(odd_value nl skzq | od -An -c | tr -s ' ')" " o n e \n t w o"
check "odd: the tab's bytes" "$(odd_value tab skzq | od -An -c | tr -s ' ')" " o n e \t t w o"
check "odd: bytes that are not UTF-8" "$(odd_value raw skzq | od -An -tx1 | tr -s ' ')" " 61 ff fe 62 c3"
check "odd: UTF-8 above 0x7f" "$(odd_value utf8 skzq | iconv -f UTF-8 -t UTF-8 > /dev/null && odd_value utf8 skzq | LC_ALL=C grep -c '[^ -~]')" 1
check "odd: a forged line after the newline" "$(odd_value forge skzq | sed -n 2p)" "skzq/child	skyzfs:forged	yes"
for k in 4095 4096 8191 8192; do
    check "odd: len$k is $k bytes" "$(odd_value "len$k" skzq | wc -c)" "$k"
done
check "pin odd get" "$(words rig-odd-get-local)" "$GET skzq "
check "pin odd get with its header" "$(words rig-odd-get-local-header)" "zfs get -r -p -t filesystem,volume -s local -o name,property,value all skzq "
check "pin odd get in JSON" "$(words rig-odd-get-local-json)" "zfs get -j -r -p -t filesystem,volume -s local -o name,property,value all skzq "
check "pin odd take" "$(words rig-odd-take)" "zfs snapshot -r skzq@g "
# shellcheck disable=SC2016 # the script bash -c runs, as written
check "pin odd recv" "$(words rig-odd-recv)" 'bash -c zfs send -p "$1" | zfs recv -u "$2" odd-recv skzq/odd-nl@g skzq/odd-recv '
check "odd recv: send and recv run" "$(grep -c -e '^send -p skzq/odd-nl@g$' -e '^recv -u skzq/odd-recv$' "$T/zfs.calls")" 2
check "pin odd get with the received" "$(words rig-odd-get-received)" "zfs get -r -H -p -t filesystem,volume -s local,received -o name,property,value,source all skzq "
check "odd recv: after the take, then the get, before the list" "$(order rig-odd-take rig-odd-recv)$(order rig-odd-recv rig-odd-get-received)$(order rig-odd-get-received rig-odd-list)" yesyesyes
check "odd recv: destroyed, before @g" "$(grep -n -m 1 '^destroy -r skzq/odd-recv$' "$T/zfs.calls" | cut -d: -f1)" "$(($(grep -n -m 1 '^destroy -r skzq@g$' "$T/zfs.calls" | cut -d: -f1) - 1))"
check "pin odd list" "$(words rig-odd-list)" "$LIST skzq "
check "pin odd full" "$(words rig-odd-full)" "zfs program -n $P skzq - skzq g full skyzfs:assist "
check "odd: reads, take, list, full in order" "$(order rig-odd-get-local rig-odd-get-local-header)$(order rig-odd-get-local-header rig-odd-get-local-json)$(order rig-odd-get-local-json rig-odd-value-nl)$(order rig-odd-value-len8192 rig-odd-take)$(order rig-odd-take rig-odd-list)$(order rig-odd-list rig-odd-full)" yesyesyesyesyesyes
check "odd: after the limit is lifted, before the checkpoint" "$(order rig-cli-limit-after rig-odd-set-nl)$(order rig-odd-full rig-checkpoint)" yesyes
check "odd: @g destroyed" "$(grep -c '^destroy -r skzq@g$' "$T/zfs.calls")" 1
check "odd: @g destroyed before the file systems" "$(grep -n -m 1 '^destroy -r skzq@g$' "$T/zfs.calls" | cut -d: -f1)" "$(($(grep -n -m 1 '^destroy -r skzq/odd-nl$' "$T/zfs.calls" | cut -d: -f1) - 1))"
# Each full record after what it reads: odd after the takes of o0 and
# o2, and before their destroy.
check "members full after the take" "$(order rig-snapshot-1 rig-members-full)" yes
check "members full, odd after its take" "$(order rig-snapshot-odd rig-members-full-odd)" yes
check "members full, odd in C.UTF-8 after its take" "$(order rig-snapshot-odd-utf8 rig-members-full-odd-utf8)" yes
check "members full, odd before the destroy" "$(order rig-members-full-odd rig-destroy)" yes
check "members full, odd in C.UTF-8 before the destroy" "$(order rig-members-full-odd-utf8 rig-destroy-held)" yes
check "members full, odd after the properties" "$(order rig-set-raw rig-members-full-odd)" yes
check "pin members full, partial" "$(words rig-members-full-partial)" "zfs program -n $P skzq - skzq p full skyzfs:assist "
check "members full, partial after the child's @p" "$(order rig-exists-child-pre rig-members-full-partial)" yes
check "members full, partial before the take at @p" "$(order rig-members-full-partial rig-cli-exists-child)" yes
# The writer beside each take: its log and its stall, in the record
# and in about.txt.
for r in snapshot take-cli; do
    check "writer beside $r: its log" "$(head -n 1 "$OUT/scale-3-$r/writer.tsv" | cut -f1)" start
    check "writer beside $r: its stall" "$(cut -c1-8 "$OUT/scale-3-$r/stall")" "longest="
    check "writer beside $r: said" "$(grep -c "^scale: writer beside scale-3-$r: longest=" "$OUT/about.txt")" 1
done
for r in snapshot take-cli; do
    check "new data beside $r: its log" "$(head -n 1 "$OUT/scale-3-$r/fill.tsv" | cut -f1)" start
    check "new data beside $r: its line" "$(cut -c1-8 "$OUT/scale-3-$r/fill")" "longest="
    check "new data beside $r: said" "$(grep -c "^scale: new data beside scale-3-$r: longest=" "$OUT/about.txt")" 1
    check "txgs beside $r: the probe's log" "$(head -n 1 "$OUT/scale-3-$r/txg.tsv" | cut -f1)" start
    check "txgs beside $r: the committed group" "$(grep -c '^txg	7	' "$OUT/scale-3-$r/txg.tsv")" 1
    check "txgs beside $r: zpool sync timed" "$(grep -c '^sync	' "$OUT/scale-3-$r/txg.tsv" | sed 's/^[1-9][0-9]*$/some/')" some
    check "txgs beside $r: its line" "$(cut -c1-5 "$OUT/scale-3-$r/txg")" "txgs="
    check "txgs beside $r: said" "$(grep -c "^scale: txgs beside scale-3-$r: txgs=" "$OUT/about.txt")" 1
done
check "new data: over twice the limit" "$(stat -c %s "$WORK/mnt/skzqs/fill.dat")" 2097152
check "new data: the span said" "$(grep -c '^scale 3: zfs_dirty_data_max 1048576; the writer of new data writes over 2097152 bytes' "$OUT/about.txt")" 1
check "txg history: turned on, said" "$(cat "$ZCR_ZFS_PARAMS/zfs_txg_history"; grep -c '^scale 3: zfs_txg_history 0, set to 100' "$OUT/about.txt")" $'100\n1'
check "writer: its file in the root" "$([ -s "$WORK/mnt/skzqs/writer.dat" ] && echo yes || echo no)" yes
check "writer: nothing left behind" "$(find "$OUT" -maxdepth 1 -name '.writer-*' | wc -l)" 0
check "writer: beside the takes, the holds, the sweep's reads and the reads after the take, at 3" "$(find "$OUT" -name writer.tsv | sed 's|.*/\(scale-[0-9]*-[a-z-]*\)/writer.tsv|\1|' | sort | tr '\n' ' ')" "scale-3-holds scale-3-snapshot scale-3-sweep-holds scale-3-sweep-list scale-3-take-cli scale-3-take-cli-get scale-3-take-cli-list scale-3-take-cli-members "
check "scale's limit is its own" "$LIMIT" 5
check "scale's image removed" "$([ -e "$WORK/scale.img" ] && echo yes || echo no)" no
check "scale's df" "$(grep -c '^scale 3: df: ' "$OUT/about.txt")" 2
# The records meant to show one message say whether they did.
for r in snapshot-2 members-absent hold-2 release-2 snapshot-exists-child destroy-absent cli-exists-child cli-exists-root; do
    check "pin --expect on $r" "$(cut -c1-10 "$OUT/rig-$r/note" 2>/dev/null)" "expected /"
done

# The rules, for every record of the pins. A rule that met no record
# fails too.
bad="" seen=""
for d in "$OUT"/*/; do
    n=$(basename "$d")
    w=()
    while IFS= read -r -d '' x; do w+=("$x"); done < "$d/argv"
    loc=C
    if [ "${w[0]}" = env ]; then
        loc=${w[1]#LC_ALL=}
        w=("${w[@]:2}")
    fi
    # The locale the command was given, as the fake saw it.
    [ "$(head -n 1 "$d/stdout" | cut -d' ' -f1)" = "$loc" ] || bad="$bad locale:$n"
    case "${w[0]} ${w[1]}" in
        "zfs program")
            seen="$seen program"
            case $(sed 's/.* < //' "$d/cmd") in
                members.lua | tagged.lua) [ "${w[2]}" = -n ] || bad="$bad -n:$n"; r=("${w[@]:3}") ;;
                snapshot.lua) [ "${w[2]}" != -n ] || bad="$bad rw:$n"; r=("${w[@]:2}") ;;
                *) bad="$bad lua:$n"; r=() ;;
            esac
            [ "${r[*]:0:5}" = "-j -t 100000000 -m 104857600" ] || bad="$bad limits:$n"
            ;;
        "zfs hold" | "zfs release")
            seen="$seen hold"
            case $n in
                # A hold on one snapshot alone, on purpose.
                *-hold-root | *-hold-child | *-destroy-held-hold) [ "${w[2]}" = skyzfs-publish ] || bad="$bad tag:$n" ;;
                # The tag that is not UTF-8, on one snapshot.
                *-sweep-hold-odd | *-sweep-release-odd) [ "${w[2]}" = "$ODD_TAG" ] && [ "${#w[@]}" = 4 ] || bad="$bad odd:$n" ;;
                # Another holder's tags, with a tab and a newline, on @t.
                *-hold-tab | *-release-tab) [ "${w[2]}" = "$TAB_TAG" ] && [ "${#w[@]}" = 4 ] || bad="$bad tab:$n" ;;
                *-hold-nl | *-release-nl) [ "${w[2]}" = "$NL_TAG" ] && [ "${#w[@]}" = 4 ] || bad="$bad nl:$n" ;;
                # The snapshot destroyed after the holds read, let go first.
                *-counts-release-one) [ "${w[2]}" = skyzfs-publish ] && [ "${#w[@]}" = 4 ] || bad="$bad one:$n" ;;
                *) [ "${w[2]} ${w[3]}" = "-r skyzfs-publish" ] || bad="$bad -r:$n" ;;
            esac
            ;;
        "zfs holds") seen="$seen holds"; [ "${w[2]}" = -H ] || bad="$bad -H:$n" ;;
        "zfs snapshot")
            seen="$seen take"
            case $n in
                # The child's @p alone, on purpose.
                *-exists-child-pre) [ "${#w[@]}" = 3 ] || bad="$bad pre:$n" ;;
                *) [ "${#w[@]}" = 4 ] && [ "${w[2]}" = -r ] && [[ ${w[3]} == *@* ]] || bad="$bad take:$n" ;;
            esac
            ;;
        "zfs destroy")
            seen="$seen destroy"
            case $n in
                # One snapshot alone, on purpose.
                *-counts-destroy-one) [ "${#w[@]}" = 3 ] && [[ ${w[2]} == *@* ]] || bad="$bad destroy-one:$n" ;;
                *) [ "${#w[@]}" = 4 ] && [ "${w[2]}" = -r ] && [[ ${w[3]} == *@* ]] || bad="$bad destroy:$n" ;;
            esac
            ;;
        "zpool status")
            seen="$seen status"
            [ "${w[2]}" = -P ] || bad="$bad -P:$n"
            # Every status has its -g partner.
            if [ "${w[3]}" != -g ]; then
                [ "$(words "$n-g" 2>/dev/null)" = "zpool status -P -g ${w[3]} " ] || bad="$bad -g:$n"
            fi
            ;;
    esac
done
check "rules: every record" "${bad:-none}" none
for k in program hold holds destroy status take; do
    check "rules: met a $k" "$(case "$seen " in *" $k "*) echo yes ;; *) echo no ;; esac)" yes
done
# The locale: adapter_env sets it, and nothing else in the script does
# but program's --locale. A status outside status() and catch_status
# would have no -g partner.
code() { grep -v '^ *#' "$HERE/zfs-cli-record.sh"; }
check "pin the locale's sites" "$(code | grep -c 'LC_ALL')" 2
check "pin the locale" "$(code | grep -c '^    export LC_ALL=C$')" 1
check "pin the status sites" "$(code | grep -c 'zpool status -P')" 4
# shellcheck disable=SC2016 # the variable's name, as written in the script
check "pin the rebuild's pattern" "$(code | grep -cF '"$REBUILD_PATTERN"')" 1

# scale: its own limit, not LIMIT; and none past the budget.
LIMIT=1
export FAKE_SLEEP=1.5
ZCR_SCALE_LIMIT=7 scale 4 > /dev/null 2>> "$T/pins.err"
check "scale's limit: a hold of 1.5 s under LIMIT=1" "$(cat "$OUT/scale-4-hold/rc")" 0
# The budget ends during the hold: its limit is what is left, and
# nothing starts after it.
SCALE_END=$(($(date +%s) + 2))
export FAKE_SLEEP=4
ZCR_SCALE_LIMIT=7 scale 6 > /dev/null 2>> "$T/pins.err"
check "budget: the hold cut at what was left" "$(cat "$OUT/scale-6-hold/rc")" 124
check "budget: nothing after it" "$([ -e "$OUT/scale-6-holds" ] || [ -e "$OUT/scale-6-sweep-list" ] || [ -e "$OUT/scale-6-tagged" ] && echo yes || echo no)" no
check "budget: said" "$(grep -c '^scale 6: the budget was spent before holds' "$OUT/about.txt")" 1
unset FAKE_SLEEP
# Above ZCR_SCALE_LUA_MAX, no program: the CLI takes @t, and the rest
# runs on it.
SCALE_END=$(($(date +%s) + 600))
export FAKE_LIST_RC=1
ZCR_SCALE_LUA_MAX=4 scale 5 > /dev/null 2>> "$T/pins.err"
unset FAKE_LIST_RC
check "lua max: not run, said" "$(grep -c '^scale 5: the snapshot program not run: 5 is more than ZCR_SCALE_LUA_MAX (4)' "$OUT/about.txt")" 1
check "lua max: no program" "$([ -e "$OUT/scale-5-snapshot" ] && echo yes || echo no)" no
check "lua max: the CLI's @t" "$(words scale-5-snapshot-cli)" "zfs snapshot -r skzqs@t "
check "lua max: the rest on it" "$(words scale-5-members)" "zfs program -n $P skzqs - skzqs t "
ZCR_SCALE_LUA_MAX=9 ZCR_SCALE_FULL_MAX=9 scale 9 > /dev/null 2>> "$T/pins.err"
check "lua max: at the count itself, run" "$(words scale-9-snapshot)" "zfs program $P skzqs - skzqs t skyzfs:assist "
# Above ZCR_SCALE_FULL_MAX, no full read, said; at it, run.
ZCR_SCALE_FULL_MAX=4 scale 7 > /dev/null 2>> "$T/pins.err"
check "full max: not run, said" "$(grep -c '^scale 7: the full mode not run: 7 is more than ZCR_SCALE_FULL_MAX (4)$' "$OUT/about.txt")" 1
check "full max: no full read" "$([ -e "$OUT/scale-7-take-cli-full" ] && echo yes || echo no)" no
check "full max: no count line for it" "$(grep -c '^scale: scale-7-take-cli-full' "$OUT/about.txt")" 0
check "full max: the CLI's reads still run" "$(words scale-7-take-cli-get)" "$GET skzqs "
check "full max: at the count itself, run" "$(words scale-9-take-cli-full)" "zfs program -n $P skzqs - skzqs c full skyzfs:assist "
SCALE_END=$(($(date +%s) - 1))
scale 8 > /dev/null 2>> "$T/pins.err"
check "budget spent: not run" "$(grep -c '^scale 8: not run' "$OUT/about.txt")" 1
check "budget spent: no pool" "$(find "$OUT" -maxdepth 1 -name 'scale-8-*' | wc -l)" 0
check "not mounted: no writer, said" "$(grep -c '^scale 4: the root is not mounted; no writer' "$OUT/about.txt")" 1
# A program that failed part way: what it took is destroyed, the CLI
# takes @t, and about.txt says how many it had taken; though the root
# has @t (the list answers 0).
SCALE_END=$(($(date +%s) + 600))
export FAKE_PROGRAM_OUT='{"return":{"taken":"4563","failed":"skzqs/d183@t","errno":"28"}}'
scale 10 > /dev/null 2>> "$T/pins.err"
unset FAKE_PROGRAM_OUT
check "partial: what was taken destroyed" "$(words scale-10-snapshot-cli-destroy-partial)" "zfs destroy -r skzqs@t "
check "partial: then the CLI's @t" "$(words scale-10-snapshot-cli)" "zfs snapshot -r skzqs@t "
check "partial: in that order" "$(order scale-10-snapshot-cli-destroy-partial scale-10-snapshot-cli)" yes
check "partial: said, with the count" "$(grep -c '^scale-10-snapshot-cli: scale-10-snapshot failed after taking 4563 snapshots: destroyed, and zfs snapshot -r takes @t$' "$OUT/about.txt")" 1
check "not partial, @t there: no CLI" "$([ -e "$OUT/scale-9-snapshot-cli" ] && echo yes || echo no)" no
# The writers and the probe beside the full read, at the counts in
# ZCR_SCALE_FULL_BESIDE (1000 by default: not at 3 above).
check "full read: no writer beside it at 3" "$([ -e "$OUT/scale-3-take-cli-full/writer.tsv" ] && echo yes || echo no)" no
OUT=$T/beside
mkdir -p "$OUT"
: > "$OUT/index.tsv"
SCALE_END=$(($(date +%s) + 600))
export FAKE_LIST_RC=1 FAKE_MOUNTED=1 ZCR_WRITER_MARGIN=0.3
ZCR_SCALE_FULL_BESIDE="5 11" scale 11 > /dev/null 2> "$T/beside.err"
ZCR_SCALE_FULL_BESIDE="" scale 12 > /dev/null 2>> "$T/beside.err"
unset FAKE_LIST_RC FAKE_MOUNTED
check "full read beside: no name twice" "$(grep -c twice "$T/beside.err")" 0
check "full read beside: the read itself" "$(words scale-11-take-cli-full)" "zfs program -n $P skzqs - skzqs c full skyzfs:assist "
for f in writer fill txg; do
    check "full read beside: $f's log" "$(head -n 1 "$OUT/scale-11-take-cli-full/$f.tsv" | cut -f1)" start
done
check "full read beside: its window" "$(wc -l < "$OUT/scale-11-take-cli-full/window")" 1
check "full read beside: covered" "$(cat "$OUT/scale-11-take-cli-full/stall" "$OUT/scale-11-take-cli-full/fill" "$OUT/scale-11-take-cli-full/txg" | grep -c '^NOT COVERED')" 0
check "full read beside: said" "$(grep -c -e '^scale: writer beside scale-11-take-cli-full: longest=' -e '^scale: new data beside scale-11-take-cli-full: longest=' -e '^scale: txgs beside scale-11-take-cli-full: txgs=' "$OUT/about.txt")" 3
check "full read beside: its count still said" "$(grep -c '^scale: scale-11-take-cli-full: ' "$OUT/about.txt")" 1
check "full read beside: settled after it" "$(grep -Ec ' settled=[0-9]+\.[0-9]{3} settle_rc=0$' "$OUT/scale-11-take-cli-full/fill")" 1
check "full read beside: empty list, none" "$([ -e "$OUT/scale-12-take-cli-full/writer.tsv" ] && echo yes || echo no)/$(words scale-12-take-cli-full)" "no/zfs program -n $P skzqs - skzqs c full skyzfs:assist "
check "full read beside: the others' writers as without it" "$(find "$OUT" -name writer.tsv | sed 's|.*/\(scale-[0-9]*-[a-z-]*\)/writer.tsv|\1|' | sort | tr '\n' ' ')" "scale-11-holds scale-11-snapshot scale-11-sweep-holds scale-11-sweep-list scale-11-take-cli scale-11-take-cli-full scale-11-take-cli-get scale-11-take-cli-list scale-11-take-cli-members scale-12-holds scale-12-snapshot scale-12-sweep-holds scale-12-sweep-list scale-12-take-cli scale-12-take-cli-get scale-12-take-cli-list scale-12-take-cli-members "
# A count that is part of a listed one is not listed (11 is in 110), and
# an empty list is none, even at the default's own count.
OUT=$T/beside-more
mkdir -p "$OUT"
: > "$OUT/index.tsv"
export FAKE_LIST_RC=1 FAKE_MOUNTED=1
ZCR_SCALE_FULL_BESIDE="110" scale 11 > /dev/null 2> "$T/beside-more.err"
ZCR_SCALE_FULL_BESIDE="" scale 1000 > /dev/null 2>> "$T/beside-more.err"
unset FAKE_LIST_RC FAKE_MOUNTED
check "full read beside: no name twice, again" "$(grep -c twice "$T/beside-more.err")" 0
check "full read beside: not at a count inside a listed one" "$([ -e "$OUT/scale-11-take-cli-full/writer.tsv" ] && echo yes || echo no)/$(words scale-11-take-cli-full)" "no/zfs program -n $P skzqs - skzqs c full skyzfs:assist "
check "full read beside: an empty list, none at 1000" "$([ -e "$OUT/scale-1000-take-cli-full/writer.tsv" ] && echo yes || echo no)/$(words scale-1000-take-cli-full)" "no/zfs program -n $P skzqs - skzqs c full skyzfs:assist "
check "full read beside: the takes still have theirs" "$([ -e "$OUT/scale-1000-take-cli/writer.tsv" ] && echo yes || echo no)" yes
OUT=$T/pins
# In record_pool too.
OUT=$T/partial
mkdir -p "$OUT/rp-snapshot-1"
: > "$OUT/index.tsv"
printf '%s\n' '{"return":{"failed":"skzq/c@t","errno":"28"}}' > "$OUT/rp-snapshot-1/stdout"
cli_snapshot rp-snapshot-cli rp-snapshot-1 skzq > /dev/null 2>> "$T/pins.err"
check "partial, no count: said" "$(grep -c '^rp-snapshot-cli: rp-snapshot-1 failed after taking an unknown number of snapshots' "$OUT/about.txt")" 1
check "partial, no count: retaken" "$(words rp-snapshot-cli)" "zfs snapshot -r skzq@t "
# members_line: the count of a members program's answer, or "not read".
mkdir -p "$OUT/m-ok" "$OUT/m-bad"
printf '%s\n' '{"return":{"members":["skzq@t","skzq/a@t","skzq/b@t"]}}' > "$OUT/m-ok/stdout"
printf 'zfs: out of memory\n' > "$OUT/m-bad/stdout"
members_line m-ok
members_line m-bad
members_line m-none
mkdir -p "$OUT/m-full"
printf '%s\n' '{"return":{"datasets":{"skzq":{},"skzq/a":{}}}}' > "$OUT/m-full/stdout"
members_line m-full
check "members_line" "$(grep '^scale: m-' "$OUT/about.txt")" $'scale: m-ok: 3 members\nscale: m-bad: not read\nscale: m-none: not read\nscale: m-full: 2 datasets'
OUT=$T/pins
LIMIT=5

# The window file: each line is its script's line on the record's own
# log and window, and the logs run past the window by the margin.
for r in snapshot holds sweep-list sweep-holds take-cli take-cli-members take-cli-get take-cli-list; do
    IFS=$'\t' read -r w0 w1 < "$OUT/scale-3-$r/window"
    check "window beside $r: t0 before t1" "$(awk -v a="$w0" -v b="$w1" 'BEGIN { print (a < b) ? "yes" : "no" }')" yes
    check "window beside $r: the stall again" "$(python3 "$HERE/dsync_writer.py" stall "$OUT/scale-3-$r/writer.tsv" "$w0" "$w1")" "$(cat "$OUT/scale-3-$r/stall")"
    check "window beside $r: the txg line again" "$(python3 "$HERE/txg_probe.py" report "$OUT/scale-3-$r/txg.tsv" "$w0" "$w1")" "$(cat "$OUT/scale-3-$r/txg")"
    check "window beside $r: the fill line again" "$(python3 "$HERE/dsync_writer.py" stall "$OUT/scale-3-$r/fill.tsv" "$w0" "$w1") settled" "$(sed 's/ settled=.*/ settled/' "$OUT/scale-3-$r/fill")"
    for f in writer fill txg; do
        s=$(head -n 1 "$OUT/scale-3-$r/$f.tsv" | cut -f2)
        check "window beside $r: $f's log began a margin before" "$(awk -v s="$s" -v a="$w0" 'BEGIN { print (s <= a - 0.2) ? "yes" : "no" }')" yes
    done
    check "window beside $r: covered" "$(cat "$OUT/scale-3-$r/stall" "$OUT/scale-3-$r/fill" "$OUT/scale-3-$r/txg" | grep -c '^NOT COVERED')" 0
    check "window beside $r: settled" "$(grep -Ec ' settled=[0-9]+\.[0-9]{3} settle_rc=0$' "$OUT/scale-3-$r/fill")" 1
done
# The settle sync: a `zpool sync` after each record's probe stopped
# (the take's, the holds' and each read's with the load beside it) and
# before the next probe began.
prev=snapshot
for r in holds sweep-list sweep-holds take-cli take-cli-members take-cli-get take-cli-list; do
    s0=$(tail -n 1 "$OUT/scale-3-$prev/txg.tsv" | cut -f2)
    s1=$(head -n 1 "$OUT/scale-3-$r/txg.tsv" | cut -f2)
    check "settle beside $prev: a sync after the probe" "$(awk -v a="$s0" -v b="$s1" '$2 == "sync" && $3 == "skzqs" && $1 >= a && $1 < b' "$T/zpool.calls" | wc -l)" 1
    prev=$r
done
s0=$(tail -n 1 "$OUT/scale-3-take-cli-list/txg.tsv" | cut -f2)
check "settle beside take-cli-list: a sync after the probe" "$(awk -v a="$s0" '$2 == "sync" && $3 == "skzqs" && $1 >= a' "$T/zpool.calls" | wc -l)" 1
for r in members take-cli-members; do
    check "members line: $r" "$(grep -c "^scale: scale-3-$r: " "$OUT/about.txt")" 1
done

# The margin runs from when each log has begun (began), not from when
# its program was started: here the writer, then the writer of new data,
# takes 1.5 s to start, longer than the margin.
mkdir -p "$T/slow"
cp "$HERE/txg_probe.py" "$T/slow/"
cat > "$T/slow/dsync_writer.py" << 'SLOW'
import os, runpy, sys, time
real = os.path.join(os.environ['REAL_HERE'], 'dsync_writer.py')
if len(sys.argv) > 1 and sys.argv[1] == os.environ.get('SLOW_START'):
    time.sleep(1.5)
sys.argv[0] = real
runpy.run_path(real, run_name='__main__')
SLOW
OUT=$T/began
mkdir -p "$OUT"
: > "$OUT/index.tsv"
export REAL_HERE=$HERE
for slow in write fill; do
    export SLOW_START=$slow
    FILL_SPAN=1048576 HERE=$T/slow writing "began-$slow" "$WORK/mnt/skzqs" skzqs true > /dev/null 2>> "$T/began.err"
    IFS=$'\t' read -r w0 w1 < "$OUT/began-$slow/window"
    for f in writer fill txg; do
        s=$(head -n 1 "$OUT/began-$slow/$f.tsv" | cut -f2)
        check "began: $f's log a margin before, the $slow writer slow to start" "$(awk -v s="$s" -v a="$w0" 'BEGIN { print (s != "" && s <= a - 0.2) ? "yes" : "no" }')" yes
    done
done
unset REAL_HERE SLOW_START
OUT=$T/pins

# main's records of `zpool list -H -o name` (#160): none before the
# first pool, one after skzq, three after skzqr, each with no pool
# named. The tests do not run main; its text is read.
check "main: the pools listed between the pools made" \
    "$(sed -n '/^main() {/,/^}/p' "$HERE/zfs-cli-record.sh" | grep -Eo '^ *(rec pools-[a-z]+ zpool list -H -o name|make_pool skzq[a-z]? )$|^ *make_pool skzq[a-z]? ' | sed 's/^ *//' | tr '\n' '|')" \
    "rec pools-none zpool list -H -o name|make_pool skzq |rec pools-one zpool list -H -o name|make_pool skzqm |make_pool skzqr |rec pools-three zpool list -H -o name|"
check "members line: take-cli-full" "$(grep -c "^scale: scale-3-take-cli-full: " "$OUT/about.txt")" 1

# Without snapshot.lua (the take is zfs snapshot -r alone since
# M2.6c-2b): no record runs the program; the CLI takes @t, @o0 and @o2,
# and the rest is recorded as with it.
OUT=$T/nolua
mkdir -p "$OUT"
: > "$OUT/index.tsv"
SNAPSHOT_LUA=0
SCALE_END=$(($(date +%s) + 600))
export FAKE_LIST_RC=1 # no @t until the CLI's
record_pool rig skzq > /dev/null 2> "$T/nolua.err"
record_none none skznone > /dev/null 2>> "$T/nolua.err"
ZCR_SCALE_LIMIT=7 scale 3 > /dev/null 2>> "$T/nolua.err"
unset FAKE_LIST_RC
SNAPSHOT_LUA=1
check "no lua: no name twice" "$(grep -c twice "$T/nolua.err")" 0
check "no lua: snapshot.lua run by none" "$(cat "$OUT"/*/cmd | grep -c 'snapshot\.lua')" 0
check "no lua: none of the program's records" "$(find "$OUT" -mindepth 1 -maxdepth 1 -type d \( -name '*-snapshot-[12]' -o -name '*-snapshot-odd*' -o -name '*-snapshot-exists-child' -o -name scale-3-snapshot \) | wc -l)" 0
check "no lua: the CLI's @t" "$(words rig-snapshot-cli)" "zfs snapshot -r skzq@t "
check "no lua: the CLI's @o0" "$(words rig-take-o0)" "zfs snapshot -r skzq@o0 "
check "no lua: the CLI's @o2" "$(words rig-take-o2)" "zfs snapshot -r skzq@o2 "
check "no lua: members full after the CLI's take" "$(order rig-snapshot-cli rig-members-full)" yes
check "no lua: members full, odd after its take" "$(order rig-take-o0 rig-members-full-odd)" yes
check "no lua: members full, odd in C.UTF-8 after its take" "$(order rig-take-o2 rig-members-full-odd-utf8)" yes
check "no lua: members full, odd in C.UTF-8" "$(words rig-members-full-odd-utf8)" "env LC_ALL=C.UTF-8 zfs program -n $P skzq - skzq o2 full skyzfs:assist "
check "no lua: count" "$(words rig-count)/$(words none-count)" "zfs list -r -H -o name -t filesystem,volume skzq /zfs list -r -H -o name -t filesystem,volume skznone "
check "no lua: members full" "$(words rig-members-full)" "zfs program -n $P skzq - skzq t full skyzfs:assist "
check "no lua: none's take" "$(words none-snapshot-cli)" "zfs snapshot -r skznone@t "
check "no lua: the take's failures still recorded" "$(words rig-cli-exists-child)/$(words rig-cli-exists-root)" "zfs snapshot -r skzq@p /zfs snapshot -r skzq@t "
check "no lua: the child's @p still made" "$(words rig-exists-child-pre)" "zfs snapshot skzq/child@p "
check "no lua: scale said" "$(grep -c '^scale 3: the snapshot program not run: there is no snapshot.lua$' "$OUT/about.txt")" 1
check "no lua: scale's @t by the CLI" "$(words scale-3-snapshot-cli)" "zfs snapshot -r skzqs@t "
check "no lua: scale's full after the take" "$(words scale-3-take-cli-full)" "zfs program -n $P skzqs - skzqs c full skyzfs:assist "
check "no lua: members full, partial" "$(words rig-members-full-partial)" "zfs program -n $P skzq - skzq p full skyzfs:assist "
check "no lua: main needs members alone" "$(code | grep -c 'no members.lua in')/$(code | grep -c 'no tagged.lua in\|no snapshot.lua in')" 1/0
# Without tagged.lua (the sweep by the CLI since M2.6c-5d): no record
# runs it, the sweep's records are made all the same, and each step
# that would have run it says so.
OUT=$T/notag
mkdir -p "$OUT"
: > "$OUT/index.tsv"
TAGGED_LUA=0
SCALE_END=$(($(date +%s) + 600))
record_pool rig skzq > /dev/null 2> "$T/notag.err"
record_none none skznone > /dev/null 2>> "$T/notag.err"
FAKE_LIST_RC=1 ZCR_SCALE_LIMIT=7 scale 3 > /dev/null 2>> "$T/notag.err"
TAGGED_LUA=1
check "no tagged: no name twice" "$(grep -c twice "$T/notag.err")" 0
check "no tagged: tagged.lua run by none" "$(cat "$OUT"/*/cmd | grep -c 'tagged\.lua')" 0
check "no tagged: none of its records" "$(find "$OUT" -mindepth 1 -maxdepth 1 -type d -name '*-tagged*' | wc -l)" 0
check "no tagged: the sweep's records" "$(words rig-sweep-holds)/$(words none-sweep-list)/$(words scale-3-sweep-holds)" "zfs holds -H skzq@t skzq/child@t /$SWL skznone /zfs holds -H skzqs@t skzqs/child@t "
check "no tagged: scale said" "$(grep -cx 'scale 3: the tagged program not run: there is no tagged.lua' "$OUT/about.txt")" 1
check "no tagged: the holds as before" "$(words rig-holds-r)/$(words rig-release-1)" "zfs holds -H -r skzq@t /zfs release -r skyzfs-publish skzq@t "

# A members.lua without the full mode (before #153): no members-full
# record, as its answer would be the plain mode's under a full name.
printf -- '-- members\nif not zfs.exists(pool .. "@" .. t) then\n' > "$T/plain.lua"
printf -- '-- members\nif mode ~= "full" then\n' > "$T/full.lua"
check "has_full: the plain program" "$(has_full "$T/plain.lua" && echo yes || echo no)" no
check "has_full: #153's" "$(has_full "$T/full.lua" && echo yes || echo no)" yes
# shellcheck disable=SC2016 # the line as written in the script
check "has_full: in main" "$(grep -v '^ *#' "$HERE/zfs-cli-record.sh" | grep -c '^    has_full "$LUA_DIR/members.lua" || MEMBERS_FULL=0$')" 1
OUT=$T/nofull
mkdir -p "$OUT"
: > "$OUT/index.tsv"
MEMBERS_FULL=0
SCALE_END=$(($(date +%s) + 600))
record_pool rig skzq > /dev/null 2> "$T/nofull.err"
record_none none skznone > /dev/null 2>> "$T/nofull.err"
ZCR_SCALE_LIMIT=7 scale 3 > /dev/null 2>> "$T/nofull.err"
MEMBERS_FULL=1
check "no full: no name twice" "$(grep -c twice "$T/nofull.err")" 0
check "no full: no full record" "$(find "$OUT" -mindepth 1 -maxdepth 1 -type d -name '*-full*' | wc -l)" 0
check "no full: no full mode run" "$(cat "$OUT"/*/argv | tr '\0' ' ' | grep -c ' full skyzfs:assist')" 0
check "no full: the rest recorded" "$(words rig-members)/$(words rig-count)" "zfs program -n $P skzq - skzq t /zfs list -r -H -o name -t filesystem,volume skzq "
check "no full: scale said" "$(grep -c '^scale 3: the full mode not run: members.lua has none$' "$OUT/about.txt")" 1
check "no full: no scale line" "$(grep -c '^scale: scale-3-take-cli-full' "$OUT/about.txt")" 0
OUT=$T/pins
check "the take's create: -u and a local property, each" "$(grep -c '^C create -u -o skyzfs:note=local skzqs/d[123]$' "$OUT/create-skzqs-3.log")" 3

# stopped and stop_bg: a process that ended with a status says it; one
# that ends on SIGTERM gives its status; one that ignores it is killed
# and said not to have stopped.
# Each is let start (and the first two end) before the signal: a
# SIGTERM to the shell's child before it runs sh would run this test's
# EXIT trap there.
printf 'start\t1.0\nend\t30.0\t1\n' > "$T/w.tsv"
sh -c 'exit 3' &
sleep 0.3
stopped $! "the writer" dsync_writer.py stall "$T/w.tsv" 10 20
check "stopped: its status" "$STOPPED" "longest=<0.01 writes=0 before=0.000 exit=3"
sh -c 'exit 0' &
sleep 0.3
stopped $! "the writer" dsync_writer.py stall "$T/w.tsv" 10 20
check "stopped: 0 not said" "$STOPPED" "longest=<0.01 writes=0 before=0.000"
sh -c 'trap "exit 5" TERM; while :; do sleep 0.05; done' &
pid=$!
sleep 0.2
stop_bg "$pid"
check "stop_bg: a status on SIGTERM" "$?/$BG_RC" "0/5"
sh -c 'trap "" TERM; while :; do sleep 0.05; done' &
pid=$!
disown "$pid" # stop_bg does not wait for it; nor does the shell's notice
sleep 0.2
ZCR_STOP_TENTHS=10 stopped "$pid" "the probe" txg_probe.py report "$T/w.tsv" 10 20
check "stopped: one that ignores SIGTERM" "$STOPPED" "the probe did not stop within 1 s"
check "stop_bg: no status then" "${BG_RC:-empty}" empty
for i in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$pid" 2> /dev/null || break; sleep 0.1; done
check "stop_bg: killed" "$(kill -0 "$pid" 2> /dev/null && echo alive || echo gone)" gone

# fill_span: at most 4 GiB; and an unread limit is no writer of new data.
mkdir -p "$T/params2"
echo 3221225472 > "$T/params2/zfs_dirty_data_max"
echo 100 > "$T/params2/zfs_txg_history"
ZCR_ZFS_PARAMS=$T/params2 fill_span 77
check "fill_span: capped" "$FILL_SPAN" 4294967296
check "fill_span: the history as it was" "$(grep -c '^scale 77: zfs_txg_history 100$' "$OUT/about.txt")" 1
ZCR_ZFS_PARAMS=$T/none fill_span 78
check "fill_span: not read" "${FILL_SPAN:-empty}/$(grep -c '^scale 78: zfs_dirty_data_max not read; no writer of new data' "$OUT/about.txt")" "empty/1"
FILL_SPAN=""

# scale_limit: ZCR_SCALE_LIMIT, 1800 by default, or what is left.
SCALE_END=$(($(date +%s) + 600))
ZCR_SCALE_LIMIT=7 scale_limit
check "scale_limit: its own" "$LIMIT" 7
scale_limit
check "scale_limit: 1800 by default, cut to what is left" "$([ "$LIMIT" -le 600 ] && [ "$LIMIT" -ge 598 ] && echo yes || echo "no: $LIMIT")" yes
SCALE_END=$(($(date +%s) + 4000))
unset ZCR_SCALE_LIMIT
scale_limit
check "scale_limit: 1800 by default" "$LIMIT" 1800
LIMIT=5

# during: the command in the background under its own limit, watched;
# it returns when the command has ended.
OUT=$T/during
mkdir -p "$OUT"
: > "$OUT/index.tsv"
LIMIT=1
during d-bg d-st 'scrub in progress' skzq sleep 1.5 > /dev/null
check "during: the command ended first" "$(cat "$OUT/d-bg/rc" 2>/dev/null)" 0
check "during: watched" "$(cat "$OUT/d-st/note" "$OUT/d-st-g/note")" $'caught at try 1\ncaught at try 1'
LIMIT=5

# retain: one name a call (ZFS refuses two snapshots of one dataset in
# a call); the space read every 100 calls and said at each tenth; a
# stop under the floor, and at a call that fails.
retained() { # N K
    rm -f "$T/r.calls" "$T/r.lists"
    FAKE_ZFS_CALLS=$T/r.calls FAKE_ZPOOL_LISTS=$T/r.lists retain "r$1-$2" skzqs "$1" "$2" > /dev/null 2>&1
    grep '^snapshot ' "$T/r.calls" | tr '\n' '|'
}
check "retain: one name a call" "$(retained 3 3)" "snapshot -r skzqs@r1|snapshot -r skzqs@r2|snapshot -r skzqs@r3|"
check "retain: said, with what the pool lists" "$(grep -c '^scale r3-3: [0-9]* snapshots in the pool (3 of each of 4 datasets asked, one name a call) made in [0-9]* s$' "$OUT/about.txt")" 1
retained 3 250 > /dev/null
check "retain: the space at each tenth" "$(grep '^scale r3-250: retained ' "$OUT/about.txt" | sed 's/ at [0-9]* s:/ at S s:/' | tr '\n' '|')" \
    "$(for i in 25 50 75 100 125 150 175 200 225 250; do printf 'scale r3-250: retained @r%s at S s: allocated and free 4096 34359738368|' "$i"; done)"
check "retain: free read before calls 1, 101 and 201" "$(grep -c -- '-o free skzqs$' "$T/r.lists")" 3
check "retain: the tenths read" "$(grep -c -- '-o alloc,free skzqs$' "$T/r.lists")" 10
check "retain: a step of 1 under ten" "$(retained 3 2 > /dev/null; grep -c '^scale r3-2: retained @r[12] ' "$OUT/about.txt")" 2
check "retain: none under the floor" "$(FAKE_POOL_FREE=8589934591 retained 3 3)" ""
check "retain: the floor said" "$(grep -c '^scale r3-3: [0-9]* snapshots in the pool (3 of each of 4 datasets asked, one name a call) made in [0-9]* s (stopped before @r1: 8589934591 bytes free, under 8589934592)$' "$OUT/about.txt")" 1
check "retain: at the floor, made" "$(FAKE_POOL_FREE=8589934592 retained 3 1)" "snapshot -r skzqs@r1|"
check "retain: a call that fails stops it" "$(FAKE_SNAP_FAIL=skzqs@r2 retained 3 4)" "snapshot -r skzqs@r1|snapshot -r skzqs@r2|"
check "retain: the failure said" "$(grep -c '^scale r3-4: .* (stopped: zfs snapshot -r of @r2 ended 1, retain-skzqs-r3-4.log)$' "$OUT/about.txt")" 1
check "retain: none once the budget is spent" "$(SCALE_END=$(($(date +%s) - 1)) retained 3 5)" ""
check "retain: the budget said" "$(grep -c '^scale r3-5: .* made in [0-9]* s (stopped: the budget was spent)$' "$OUT/about.txt")" 1
# The K reached, said after each: all, none, and up to a call that
# failed.
check "retain: the K reached" "$(grep -x 'scale r3-[345]: K reached: [0-9]* of the [345] asked' "$OUT/about.txt" | tr '\n' '|')" \
    "scale r3-3: K reached: 3 of the 3 asked|scale r3-3: K reached: 0 of the 3 asked|scale r3-4: K reached: 1 of the 4 asked|scale r3-5: K reached: 0 of the 5 asked|"
# A free that is not a number stops nothing: the floor is for a free
# that was read.
check "retain: a free not read, made" "$(FAKE_FREE_TEXT=- retained 3 2)" "snapshot -r skzqs@r1|snapshot -r skzqs@r2|"
# The last call has its line when K is not a multiple of its tenth.
retained 3 25 > /dev/null
check "retain: a line at the last" "$(grep '^scale r3-25: retained ' "$OUT/about.txt" | sed 's/ at [0-9]* s:.*//' | tail -n 2 | tr '\n' '|')" "scale r3-25: retained @r24|scale r3-25: retained @r25|"
# Each call and each read of the space is under the limit: a call that
# does not end stops it (124), a free not read in time stops nothing,
# and a tenth not read in time is said empty.
check "retain: a call past the limit stops it" "$(FAKE_SNAP_SLEEP=5 ZCR_SCALE_LIMIT=1 retained 3 2)" "snapshot -r skzqs@r1|"
check "retain: the call past the limit said" "$(grep -c '^scale r3-2: .* (stopped: zfs snapshot -r of @r1 ended 124, retain-skzqs-r3-2.log)$' "$OUT/about.txt")" 1
check "retain: a free past the limit, made" "$(FAKE_LIST_SLEEP=5 FAKE_POOL_FREE=1 ZCR_SCALE_LIMIT=1 retained 3 1)" "snapshot -r skzqs@r1|"
check "retain: a tenth past the limit, said empty" "$(grep -cx 'scale r3-1: retained @r1 at [0-9]* s: allocated and free ' "$OUT/about.txt")" 1

# sweep-rounds: each sweep read again, back to back for a window, beside
# an idle control of the load alone for the same window, the control
# first in odd rounds and second in even ones; each window opens after
# the load's first groups committed (design §5.2; R1 to R3 of #164's
# review).
check "median: odd" "$(median 3 1 2)" 2.000
check "median: even" "$(median 1 2)" 1.500
check "median: <x read as x" "$(median '<0.1' 0.3)" 0.200
check "median: >x read as x" "$(median '>0.5' 0.3)" 0.400
check "median: none a number" "$(median '?' 'NOT')" "?"
check "median: none" "$(median)" "?"
check "rate_of: groups a second" "$(rate_of 7 10.00)" 0.70
check "rate_of: rounded to two places" "$(rate_of 11 11.50)" 0.96
check "rate_of: no count" "$(rate_of '?' 10.00)" "?"
check "rate_of: no window" "$(rate_of 7 '?')" "?"
check "rate_of: a window of 0" "$(rate_of 7 0.00)" "?"
check "rate_of: a count that is not one" "$(rate_of 1.5 10)" "?"
check "share_of: a share" "$(share_of 0.8 1.25)" 0.64
check "share_of: no time" "$(share_of '?' 1.25)" "?"
check "share_of: no window" "$(share_of 0.8 '?')" "?"
check "share_of: a window of 0" "$(share_of 0.8 0)" "?"
mkdir -p "$T/sr/none"
printf 'longest=1.250 writes=3 before=0.100\n' > "$T/sr/none/stall"
printf 'txgs=4 longest_sync=0.2 longest_group=9:1/1 longest_gap=1.0 syncs=2 sync_longest=>0.300 sync_failed=0\n' > "$T/sr/none/txg"
check "stall_of: the longest" "$(OUT=$T/sr stall_of none)" 1.250
check "stall_of: no stall file" "$(OUT=$T/sr stall_of nothing)" "?"
check "field_of: the first key" "$(OUT=$T/sr field_of none txg txgs)" 4
check "field_of: a key inside, not its suffix's" "$(OUT=$T/sr field_of none txg sync_longest)/$(OUT=$T/sr field_of none txg longest_sync)" ">0.300/0.2"
check "field_of: no key" "$(OUT=$T/sr field_of none txg nothing)" "?"
mkdir -p "$T/sr/win"; printf '100.25\t110.5\n' > "$T/sr/win/window"
check "window_of: its length" "$(OUT=$T/sr window_of win)" "10.25"
check "window_of: no window" "$(OUT=$T/sr window_of none)" "?"
# A NOT COVERED line's figures are not the window's (D3 of #169's review).
mkdir -p "$T/sr/nc"; echo "NOT COVERED: the log runs from 1 to 2, the window from 0 to 3; longest=3.000 writes=1 before=0.000" > "$T/sr/nc/stall"
check "field_of: a NOT COVERED line gives none" "$(OUT=$T/sr field_of nc stall longest)/$(OUT=$T/sr stall_of nc)" "?/?"
# reading_for: back to back for the window, at least one run, the window
# not overrun by a run that would take it past.
rf() { local a b; a=$(date +%s.%N); reading_for "$@" > "$T/rf.out"; b=$(date +%s.%N); secs_between "$a" "$b"; }
rfw=$(rf 1 sleep 0.1)
check "reading_for: runs back to back" "$(sed -n 's/^runs=\([0-9]*\) failed=0 longest=0\.[0-9]* busy=[0-9.]*$/\1/p' "$T/rf.out" | awk '{ print ($1 >= 7 && $1 <= 10) ? "yes" : $0 }')" yes
check "reading_for: the window, no shorter" "$(awk -v w="$rfw" 'BEGIN { print (w >= 1.0 && w <= 1.2) ? "yes" : w }')" yes
# Runs of 0.4 s in a window of 1 s: a third would end past it, so two.
rfw=$(rf 1 sleep 0.4)
check "reading_for: no run that would end past the window" "$(cut -d' ' -f1 "$T/rf.out")" "runs=2"
# The runs' sum: two of 0.4 s, not the window.
check "reading_for: its busy time" "$(sed -n 's/.* busy=//p' "$T/rf.out" | awk '{ print ($1 >= 0.79 && $1 < 0.95) ? "yes" : $1 }')" yes
# What the last run left of the window is waited out: as long as the
# idle control's, whose groups it is compared with.
check "reading_for: the rest of the window waited" "$(awk -v w="$rfw" 'BEGIN { print (w >= 1.0 && w < 1.2) ? "yes" : w }')" yes
rfw=$(rf 1 sleep 1.5)
check "reading_for: a run longer than the window, once" "$(cut -d' ' -f1,2 "$T/rf.out")/$(awk -v w="$rfw" 'BEGIN { print (w < 2.0) ? "yes" : w }')" "runs=1 failed=0/yes"
rf 0 false > /dev/null
check "reading_for: failures counted" "$(cut -d' ' -f1,2 "$T/rf.out")" "runs=1 failed=1"
mkdir -p "$T/rounds"
: > "$T/rounds/index.tsv"
OUT=$T/rounds SCALE_END=$(($(date +%s) + 600)) FAKE_LIST_RC=1 FAKE_MOUNTED=1 ZCR_WRITER_MARGIN=0.3 ZCR_SWEEP_ROUNDS=2 ZCR_SWEEP_WINDOW=1 ZCR_SETTLE_S=1 ZCR_SCALE_LIMIT=7 \
    scale 3 > /dev/null 2> "$T/rounds.err"
check "rounds: no name twice" "$(grep -c twice "$T/rounds.err")" 0
check "rounds: the records" \
    "$(find "$T/rounds" -mindepth 1 -maxdepth 1 -name 'scale-3-sweep-*' -printf '%f\n' | sort | tr '\n' ' ')" \
    "scale-3-sweep-holds scale-3-sweep-holds-idle-r1 scale-3-sweep-holds-idle-r2 scale-3-sweep-holds-r1 scale-3-sweep-holds-r2 scale-3-sweep-list scale-3-sweep-list-idle-r1 scale-3-sweep-list-idle-r2 scale-3-sweep-list-r1 scale-3-sweep-list-r2 "
same_read() { printf 'reading_for\0001\0' | cat - "$T/rounds/scale-3-sweep-$1/argv" | cmp -s - "$T/rounds/scale-3-sweep-$1-$2/argv" && echo same; }
check "rounds: the reads are the step's, for the window" "$(same_read list r1)$(same_read list r2)$(same_read holds r1)$(same_read holds r2)" samesamesamesame
check "rounds: the controls run no command" "$(cut -f1 "$T/rounds/index.tsv" | grep -c idle)" 0
# t0 of a record's window.
w0() { cut -f1 "$T/rounds/$1/window"; }
w_len() { awk -F '\t' '{ printf "%.3f\n", $2 - $1 }' "$T/rounds/$1/window"; }
before() { awk -v a="$(w0 "$1")" -v b="$(w0 "$2")" 'BEGIN { print (a < b) ? "yes" : "no" }'; }
for kind in list holds; do
    check "rounds $kind: round 1, the control first" "$(before "scale-3-sweep-$kind-idle-r1" "scale-3-sweep-$kind-r1")" yes
    check "rounds $kind: round 2, the read first" "$(before "scale-3-sweep-$kind-r2" "scale-3-sweep-$kind-idle-r2")" yes
    check "rounds $kind: round 1 before round 2" "$(before "scale-3-sweep-$kind-r1" "scale-3-sweep-$kind-r2")" yes
    for r in r1 idle-r1 r2 idle-r2; do
        check "rounds $kind $r: the load beside it" "$([ -s "$T/rounds/scale-3-sweep-$kind-$r/writer.tsv" ] && [ -s "$T/rounds/scale-3-sweep-$kind-$r/stall" ] && [ -s "$T/rounds/scale-3-sweep-$kind-$r/txg" ] && echo yes || echo no)" yes
        check "rounds $kind $r: the settle said" "$(cat "$T/rounds/scale-3-sweep-$kind-$r/settled")" settle=off
    done
    # Both sides a window of 1 s: the control's exactly, the reads'
    # within one run of it (R2: each round's, not the step's first).
    check "rounds $kind: the windows" \
        "$(awk -v a="$(w_len "scale-3-sweep-$kind-idle-r1")" -v b="$(w_len "scale-3-sweep-$kind-idle-r2")" -v c="$(w_len "scale-3-sweep-$kind-r1")" -v d="$(w_len "scale-3-sweep-$kind-r2")" \
            'BEGIN { print (a >= 1 && b >= 1 && a < 1.3 && b < 1.3 && c >= 1 && d >= 1 && c < 1.5 && d < 1.5) ? "yes" : a " " b " " c " " d }')" yes
    check "rounds $kind: the reads back to back" "$(sed -n 's/^runs=\([0-9]*\) failed=0 .*/\1/p' "$T/rounds/scale-3-sweep-$kind-r1/stdout" | awk '{ print ($1 > 1) ? "yes" : $1 }')" yes
    f() { local x="" r; for r in r1 r2; do x="$x $(OUT=$T/rounds field_of "scale-3-sweep-$kind-$1$r" "$2" "$3")"; done; echo "${x# }"; }
    xrn=$(f "" stdout runs) xrf=$(f "" stdout failed) xrl=$(f "" stall longest) xil=$(f idle- stall longest) xrt=$(f "" txg txgs) xit=$(f idle- txg txgs) xrs=$(f "" txg sync_longest) xis=$(f idle- txg sync_longest)
    xrw="$(OUT=$T/rounds window_of "scale-3-sweep-$kind-r1") $(OUT=$T/rounds window_of "scale-3-sweep-$kind-r2")"
    xiw="$(OUT=$T/rounds window_of "scale-3-sweep-$kind-idle-r1") $(OUT=$T/rounds window_of "scale-3-sweep-$kind-idle-r2")"
    xrb="$(share_of "$(OUT=$T/rounds field_of "scale-3-sweep-$kind-r1" stdout busy)" "${xrw% *}") $(share_of "$(OUT=$T/rounds field_of "scale-3-sweep-$kind-r2" stdout busy)" "${xrw#* }")"
    # shellcheck disable=SC2086
    xrr="$(set -- $xrt; a=$1 b=$2; set -- $xrw; echo "$(rate_of "$a" "$1") $(rate_of "$b" "$2")")"
    # shellcheck disable=SC2086
    xir="$(set -- $xit; a=$1 b=$2; set -- $xiw; echo "$(rate_of "$a" "$1") $(rate_of "$b" "$2")")"
    # shellcheck disable=SC2086
    check "rounds $kind: said, with the medians" \
        "$(grep -cxF "scale 3: sweep-$kind against an idle control, 2 rounds of 1 s (odd rounds idle first): reads run $xrn, failed $xrf, a read in $xrb of their window (median $(median $xrb)); longest write read $xrl (median $(median $xrl)), idle $xil (median $(median $xil)); groups committed read $xrt (median $(median $xrt)), idle $xit (median $(median $xit)); groups a second of each window read $xrr (median $(median $xrr)), idle $xir (median $(median $xir)); longest zpool sync read $xrs (median $(median $xrs)), idle $xis (median $(median $xis)); windows read $xrw s, idle $xiw s" "$T/rounds/about.txt")" 1
done
check "rounds: after the sweep's reads, before tagged" \
    "$(OUT=$T/rounds order scale-3-sweep-holds scale-3-sweep-list-r1)$(OUT=$T/rounds order scale-3-sweep-holds-r2 scale-3-tagged)" yesyes
# None held: no holds rounds, said; the list's rounds run.
mkdir -p "$T/rnh"
: > "$T/rnh/index.tsv"
OUT=$T/rnh SCALE_END=$(($(date +%s) + 600)) FAKE_HELD=0 FAKE_LIST_RC=1 FAKE_MOUNTED=1 ZCR_WRITER_MARGIN=0.3 ZCR_SWEEP_ROUNDS=1 ZCR_SWEEP_WINDOW=1 ZCR_SETTLE_S=1 ZCR_SCALE_LIMIT=7 \
    scale 2 > /dev/null 2> "$T/rnh.err"
check "rounds, none held: the list's alone" \
    "$(find "$T/rnh" -mindepth 1 -maxdepth 1 -name 'scale-2-sweep-*-r*' -printf '%f\n' | sort | tr '\n' ' ')" "scale-2-sweep-list-idle-r1 scale-2-sweep-list-r1 "
check "rounds, none held: said" "$(grep -cx "scale 2: the sweep's holds rounds not run: its list gives no snapshot with a hold" "$T/rnh/about.txt")" 1
# Not a count, no writer, no time, a cut read, the budget, the limit:
# said, nothing run. RT_SECS is the list's time, RT_LEFT the seconds
# the budget has left, RT_LIMIT a record's limit, RT_WIN the window,
# RT_MARGIN and RT_TXGS writing's margin and the settle's groups (0.3
# and 2), RT_CUT the round whose read is cut at its limit.
rounds_t() { # ROUNDS WDIR [LIMIT_AFTER [notime|cut]]
    local d=$T/rt
    rm -rf "${d:?}"; mkdir -p "$d/scale-9-sweep-list" "$d/scale-9-sweep-holds"; : > "$d/index.tsv"
    printf 'P@t\t1\n' > "$d/scale-9-sweep-list/stdout"
    echo 0.050 > "$d/scale-9-sweep-holds/secs"; echo 0 > "$d/scale-9-sweep-holds/rc"
    case ${4:-} in
        notime) ;;
        cut) echo "${RT_SECS:-0.050}" > "$d/scale-9-sweep-list/secs"; echo 124 > "$d/scale-9-sweep-list/rc" ;;
        failed) echo "${RT_SECS:-0.050}" > "$d/scale-9-sweep-list/secs"; echo 1 > "$d/scale-9-sweep-list/rc" ;;
        *) echo "${RT_SECS:-0.050}" > "$d/scale-9-sweep-list/secs"; echo 0 > "$d/scale-9-sweep-list/rc" ;;
    esac
    (
        n=0 most=${3:-99}
        scale_limit() { n=$((n + 1)); LIMIT=${RT_LIMIT:-1800}; [ "$n" -le "$most" ]; }
        writing() {
            echo "$1" >> "$T/rt/calls"
            if [ "$4" = idle_for ]; then echo "$1 $5" >> "$T/rt/wins"; else echo "$1 $6 $7" >> "$T/rt/wins"; fi
            # Each record's figures told apart: 1 for a read, 2 for its
            # control, then the round; longest_sync, the group's, never
            # the zpool sync's.
            local k=1 r=${1##*-r}
            case $1 in *-idle-r*) k=2 ;; esac
            mkdir -p "$OUT/$1"
            echo "longest=$k.$r writes=3" > "$OUT/$1/stall"
            echo "txgs=$k$r longest_sync=9.9 syncs=2 sync_longest=0.$k$r sync_failed=0" > "$OUT/$1/txg"
            if [ "$k" = 1 ]; then
                echo "runs=$r$r failed=$r longest=0.1 busy=$r.15" > "$OUT/$1/stdout"
                if [ "$r" = "${RT_CUT:-}" ]; then echo 124 > "$OUT/$1/rc"; else echo 0 > "$OUT/$1/rc"; fi
            fi
            printf '100.0\t%s.5\n' "$((100 + 10 * k + r))" > "$OUT/$1/window"
        }
        SCALE_END=$(($(date +%s) + ${RT_LEFT:-3600}))
        OUT=$d ZCR_SWEEP_ROUNDS=$1 ZCR_SWEEP_WINDOW=${RT_WIN:-10} ZCR_WRITER_MARGIN=${RT_MARGIN-0.3} ZCR_SETTLE_TXGS=${RT_TXGS-2} sweep_rounds 9 "$2" P
    )
    echo "$(cat "$d/calls" 2> /dev/null | tr '\n' ' ')|$(grep -v ': sweep-' "$d/about.txt" 2> /dev/null | tr '\n' ' ')"
}
# Each side's figures from its own records, in round order.
rounds_t 2 "$T/w" > /dev/null
check "rounds: the line from each side's records" "$(grep -c . "$T/rt/about.txt")/$(grep -F 'scale 9: sweep-list' "$T/rt/about.txt")" \
    "2/scale 9: sweep-list against an idle control, 2 rounds of 10 s (odd rounds idle first): reads run 11 22, failed 1 2, a read in 0.10 0.17 of their window (median 0.135); longest write read 1.1 1.2 (median 1.150), idle 2.1 2.2 (median 2.150); groups committed read 11 12 (median 11.500), idle 21 22 (median 21.500); groups a second of each window read 0.96 0.96 (median 0.960), idle 0.98 0.98 (median 0.980); longest zpool sync read 0.11 0.12 (median 0.115), idle 0.21 0.22 (median 0.215); windows read 11.50 12.50 s, idle 21.50 22.50 s"
# A round whose read was cut at its limit: cut on both sides, in no
# median (the review's nit on #169).
RT_CUT=2 rounds_t 2 "$T/w" > /dev/null
check "rounds: a round with its read cut" "$(grep -F 'scale 9: sweep-list' "$T/rt/about.txt")" \
    "scale 9: sweep-list against an idle control, 2 rounds of 10 s (odd rounds idle first): reads run 11 cut, failed 1 cut, a read in 0.10 cut of their window (median 0.100); longest write read 1.1 cut (median 1.100), idle 2.1 cut (median 2.100); groups committed read 11 cut (median 11.000), idle 21 cut (median 21.000); groups a second of each window read 0.96 cut (median 0.960), idle 0.98 cut (median 0.980); longest zpool sync read 0.11 cut (median 0.110), idle 0.21 cut (median 0.210); windows read 11.50 cut s, idle 21.50 cut s"
check "rounds: not a count" "$(rounds_t x "$T/w")" "|scale 9: the sweep's rounds not run: ZCR_SWEEP_ROUNDS is not a count of at most 99: x "
check "rounds: 100 refused" "$(rounds_t 100 "$T/w")" "|scale 9: the sweep's rounds not run: ZCR_SWEEP_ROUNDS is not a count of at most 99: 100 "
check "rounds: 0, nothing" "$(rounds_t 0 "$T/w")" "|"
# 0 turns the step off: no line at all, not even the writer's absence.
check "rounds: 0, no line" "$(rounds_t 0 ""; cat "$T/rt/about.txt" 2> /dev/null)" "|"
check "rounds: 00, nothing" "$(rounds_t 00 "$T/w")" "|"
# A leading zero is decimal, not octal.
check "rounds: 08 is eight" "$(rounds_t 08 "$T/w" > /dev/null; grep -c 'sweep-list-r' "$T/rt/calls")" 8
check "rounds: no writer" "$(rounds_t 1 "")" "|scale 9: the sweep's rounds not run: no writer "
check "rounds: no time, said" "$(rounds_t 1 "$T/w" 99 notime)" \
    "scale-9-sweep-holds-idle-r1 scale-9-sweep-holds-r1 |scale 9: the sweep's list rounds not run: scale-9-sweep-list has no time "
# R3: a read cut at its limit gives no rounds.
check "rounds: the step's read cut, said" "$(rounds_t 1 "$T/w" 99 cut)" \
    "scale-9-sweep-holds-idle-r1 scale-9-sweep-holds-r1 |scale 9: the sweep's list rounds not run: scale-9-sweep-list was cut at its limit "
# A read that failed otherwise gives none either (R3, the review of #169).
check "rounds: the step's read failed, said" "$(rounds_t 1 "$T/w" 99 failed)" \
    "scale-9-sweep-holds-idle-r1 scale-9-sweep-holds-r1 |scale 9: the sweep's list rounds not run: scale-9-sweep-list ended with status 1 "
# ZCR_SETTLE_S with a leading zero is decimal in the budget: 2 * (10 + 2
# + 8 + 2) = 44 s fits in 50.
check "rounds: a settle of 08 s in the budget" "$(RT_LEFT=50 ZCR_SETTLE_S=08 rounds_t 1 "$T/w" 2>&1)" \
    "scale-9-sweep-list-idle-r1 scale-9-sweep-list-r1 scale-9-sweep-holds-idle-r1 scale-9-sweep-holds-r1 |"
check "rounds: the budget spent" "$(rounds_t 3 "$T/w" 1)" \
    "scale-9-sweep-list-idle-r1 scale-9-sweep-list-r1 |scale 9: the budget was spent before the sweep's list round 2; it and the rest were not run scale 9: the budget was spent before the sweep's holds round 1; it and the rest were not run "
# R3: a round is not started that the budget cannot hold: two windows,
# each with its margins rounded up, its settle and 2 s: 2 * (10 + 2 + 30
# + 2).
check "rounds: the budget cannot hold a round" "$(RT_LEFT=80 rounds_t 1 "$T/w" | sed 's/([0-9]* s left/(N s left/g')" \
    "|scale 9: the budget cannot hold the sweep's list round 1 (N s left of the 88 s it may take); it and the rest were not run scale 9: the budget cannot hold the sweep's holds round 1 (N s left of the 88 s it may take); it and the rest were not run "
# writing's own margin, 2 s, where none is set (P15 of #169's review):
# 2 * (10 + 2 * 3 + 30 + 2).
check "rounds: the margin's default in the budget" "$(RT_LEFT=90 RT_MARGIN='' rounds_t 1 "$T/w" | sed 's/([0-9]* s left/(N s left/g')" \
    "|scale 9: the budget cannot hold the sweep's list round 1 (N s left of the 96 s it may take); it and the rest were not run scale 9: the budget cannot hold the sweep's holds round 1 (N s left of the 96 s it may take); it and the rest were not run "
# The settle in the budget as settle_txgs waits: not at all where it is
# off or its settings are not counts (2 * (10 + 2 + 0 + 2) = 28), its
# setting where it is a count, and a count of more than six digits as
# 999999 s.
for st in "ZCR_SETTLE_S=abc" "RT_TXGS=0" "RT_TXGS=00" "RT_TXGS=x"; do
    check "rounds: no settle in the budget, $st" "$(export "${st?}"; RT_LEFT=30 rounds_t 1 "$T/w" 2>&1)" \
        "scale-9-sweep-list-idle-r1 scale-9-sweep-list-r1 scale-9-sweep-holds-idle-r1 scale-9-sweep-holds-r1 |"
done
check "rounds: a settle of 12345 s in the budget" "$(RT_LEFT=3600 ZCR_SETTLE_S=12345 rounds_t 1 "$T/w" | sed 's/([0-9]* s left/(N s left/g')" \
    "|scale 9: the budget cannot hold the sweep's list round 1 (N s left of the 24718 s it may take); it and the rest were not run scale 9: the budget cannot hold the sweep's holds round 1 (N s left of the 24718 s it may take); it and the rest were not run "
check "rounds: a settle of seven digits in the budget" "$(RT_LEFT=3600 ZCR_SETTLE_S=1234567 rounds_t 1 "$T/w" | sed 's/([0-9]* s left/(N s left/g')" \
    "|scale 9: the budget cannot hold the sweep's list round 1 (N s left of the 2000026 s it may take); it and the rest were not run scale 9: the budget cannot hold the sweep's holds round 1 (N s left of the 2000026 s it may take); it and the rest were not run "
check "rounds: a budget that holds one" "$(RT_LEFT=95 rounds_t 1 "$T/w")" \
    "scale-9-sweep-list-idle-r1 scale-9-sweep-list-r1 scale-9-sweep-holds-idle-r1 scale-9-sweep-holds-r1 |"
check "rounds: a record's limit under two windows" "$(RT_LIMIT=19 rounds_t 1 "$T/w")" \
    "|scale 9: the sweep's list round 1 not run: a record's limit of 19 s cannot hold a window of 10 s; it and the rest were not run scale 9: the sweep's holds round 1 not run: a record's limit of 19 s cannot hold a window of 10 s; it and the rest were not run "
# The window: ZCR_SWEEP_WINDOW, or the step's read rounded up where
# that is longer; the same for both sides.
rounds_t 1 "$T/w" > /dev/null
check "rounds: the window, both sides" "$(head -n 2 "$T/rt/wins" | tr '\n' ' ')" "scale-9-sweep-list-idle-r1 10 scale-9-sweep-list-r1 reading_for 10 "
RT_SECS=12.2 rounds_t 1 "$T/w" > /dev/null
check "rounds: a longer read, the window its time rounded up" "$(head -n 2 "$T/rt/wins" | tr '\n' ' ')" "scale-9-sweep-list-idle-r1 13 scale-9-sweep-list-r1 reading_for 13 "
RT_SECS=12 rounds_t 1 "$T/w" > /dev/null
check "rounds: a whole second not rounded up" "$(head -n 1 "$T/rt/wins")" "scale-9-sweep-list-idle-r1 12"
check "rounds: window not a count" "$(RT_WIN=x rounds_t 1 "$T/w")" "|scale 9: the sweep's rounds not run: ZCR_SWEEP_WINDOW is not a count of seconds of at most 999: x "
check "rounds: window 1000 refused" "$(RT_WIN=1000 rounds_t 1 "$T/w")" "|scale 9: the sweep's rounds not run: ZCR_SWEEP_WINDOW is not a count of seconds of at most 999: 1000 "
check "rounds: window 0" "$(RT_WIN=0 rounds_t 1 "$T/w")" "|scale 9: the sweep's rounds not run: ZCR_SWEEP_WINDOW is 0 "
RT_WIN=09 rounds_t 1 "$T/w" > /dev/null
check "rounds: window 09 is nine" "$(head -n 1 "$T/rt/wins")" "scale-9-sweep-list-idle-r1 9"

# settle_txgs: the window waits for the load's first groups (R1 of
# #164's review): groups whose sync ended after the probe began.
st_t() { # LOG-LINES... (\t-separated)
    printf '%b\n' "$@" > "$T/st.tsv"
    settle_txgs "$T/st.tsv"
    echo "$SETTLED"
}
check "settle: two groups after the start" "$(ZCR_SETTLE_TXGS=2 st_t 'start\t100.0\t0' 'txg\t5\t99.5\t1\t0\t0\t1\t0\t0' 'txg\t6\t100.5\t1\t0\t0\t1\t0\t0' 'txg\t7\t101.0\t1\t0\t0\t1\t0\t0' | sed 's/settled=[0-9.]*/settled=S/')" "settled=S groups=2"
check "settle: a group before the start not counted" "$(ZCR_SETTLE_TXGS=2 ZCR_SETTLE_S=1 st_t 'start\t100.0\t0' 'txg\t5\t99.5\t1\t0\t0\t1\t0\t0' 'tick\t100.2\t6\tO' 'txg\t6\t100.5\t1\t0\t0\t1\t0\t0')" "NOT SETTLED: 1 of 2 groups in 1 s"
check "settle: no log" "$(rm -f "$T/st.tsv"; ZCR_SETTLE_TXGS=1 ZCR_SETTLE_S=0 settle_txgs "$T/st.tsv"; echo "$SETTLED")" "NOT SETTLED: 0 of 1 groups in 0 s"
check "settle: off" "$(ZCR_SETTLE_TXGS=0 st_t 'start\t100.0\t0')" "settle=off"
check "settle: the default, two" "$(unset ZCR_SETTLE_TXGS; ZCR_SETTLE_S=0 st_t 'start\t100.0\t0' 'txg\t6\t100.5\t1\t0\t0\t1\t0\t0')" "NOT SETTLED: 1 of 2 groups in 0 s"
check "settle: not a count" "$(ZCR_SETTLE_TXGS=x st_t 'start\t100.0\t0')" "NOT SETTLED: ZCR_SETTLE_TXGS and ZCR_SETTLE_S are counts: x, 30"
# A group that commits while it waits.
printf 'start\t100.0\t0\ntxg\t6\t100.5\t1\t0\t0\t1\t0\t0\n' > "$T/st2.tsv"
(sleep 0.6; printf 'txg\t7\t101.5\t1\t0\t0\t1\t0\t0\n' >> "$T/st2.tsv") &
ZCR_SETTLE_TXGS=2 ZCR_SETTLE_S=5 settle_txgs "$T/st2.tsv"
wait
check "settle: waits for the group" "$(echo "$SETTLED" | awk '{ split($1, a, "="); print (a[2] >= 0.4 && a[2] < 3 && $2 == "groups=2") ? "yes" : $0 }')" yes
# writing waits for the settle before its margin and the command: a
# kstat that commits a group every 0.3 s from now on.
mkdir -p "$T/kst/skzst" "$T/wst"
cp "$ZCR_KSTAT_DIR/skzqs/txgs" "$T/kst/skzst/txgs"
(
    for g in 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39; do
        sleep 0.3
        printf '%s %s C 4096 0 8192 0 2 100000000 1000 2000 1000000\n' "$g" \
            "$(python3 -c 'import time; print(time.clock_gettime_ns(time.CLOCK_MONOTONIC_RAW) - 2 * 10**8)')" >> "$T/kst/skzst/txgs"
    done
) &
kpid=$!
OUT=$T/wst ZCR_KSTAT_DIR=$T/kst ZCR_SETTLE_TXGS=2 ZCR_SETTLE_S=10 writing w-settle "$WORK/mnt/skzqs" skzst sleep 0.1
wait "$kpid"
# The probe reads the kstat once a second: two or more groups at once.
check "writing: settled before the window" "$(sed 's/settled=[0-9.]*/settled=S/; s/groups=[2-9]$/groups=N/' "$T/wst/w-settle/settled")" "settled=S groups=N"
check "writing: the settle said" "$(grep -c '^scale: settle beside w-settle: settled=' "$T/wst/about.txt")" 1
# The window opens after the second group's end in the probe's log.
# The settle comes before the margin, not after it (S01 of #169's
# review): the margin is what follows the settled groups.
check "writing: the settle, then the margin" "$(
    settle_txgs() { echo settle >> "$T/wso"; SETTLED=x; }
    sleep() { [ "$1" != 0.77 ] || echo margin >> "$T/wso"; command sleep "$@"; }
    rm -f "$T/wso"
    OUT=$T/wst ZCR_KSTAT_DIR=$T/kst ZCR_WRITER_MARGIN=0.77 writing w-order "$WORK/mnt/skzqs" skzst true
    tr '\n' ' ' < "$T/wso")" "settle margin margin "
# writing's own margin where none is set: 2 s before the command and
# after it (P15 of #169's review).
check "writing: the margin's default" "$(
    settle_txgs() { SETTLED=x; }
    sleep() { echo "$*" >> "$T/wsd"; command sleep 0.01; }
    rm -f "$T/wsd"
    unset ZCR_WRITER_MARGIN
    OUT=$T/wst ZCR_KSTAT_DIR=$T/kst writing w-default "$WORK/mnt/skzqs" skzst true
    grep -cx 2 "$T/wsd")" 2
check "writing: the window after the groups" "$(awk -F '\t' -v t0="$(cut -f1 "$T/wst/w-settle/window")" '$1 == "start" { s = $2 } $1 == "txg" && $3 > s { n++; if (n == 2) e = $3 } END { print (e != "" && t0 > e) ? "yes" : "no " e " " t0 }' "$T/wst/w-settle/txg.tsv")" yes

# scale_all: at most three entries, each once; a count at most 100000,
# and N:K at most 200000 snapshots in all.
# shellcheck disable=SC2329 # run through scale_all
scale() { echo "$1${2:+:$2}" >> "$T/scaled"; }
scale_all 5 100001 99999999999999999999 5 abc 012 7 8 9 > /dev/null
check "scale_all: the counts run" "$(tr '\n' ' ' < "$T/scaled")" "5:0 7:0 8:0 "
check "scale_all: the settle's setting said once" "$(grep -c '^scale settings: each window beside a writer opens after ZCR_SETTLE_TXGS=0 groups have committed since its probe began, waiting at most ZCR_SETTLE_S=30 s (0 groups: no wait, the margin of ZCR_WRITER_MARGIN=0.3 s alone)$' "$OUT/about.txt")" 1
# The line's defaults are the code's (S10, S12 of #169's review).
check "scale_all: the setting's defaults" "$(
    unset ZCR_SETTLE_TXGS ZCR_SETTLE_S ZCR_WRITER_MARGIN
    mkdir -p "$T/sad"; OUT=$T/sad scale_all 5 > /dev/null; cat "$T/sad/about.txt")" \
    "scale settings: each window beside a writer opens after ZCR_SETTLE_TXGS=2 groups have committed since its probe began, waiting at most ZCR_SETTLE_S=30 s (0 groups: no wait, the margin of ZCR_WRITER_MARGIN=2 s alone)"
check "scale_all: what was not run, said" "$(grep -c '^scale: ' "$OUT/about.txt")" 6
rm -f "$T/scaled"
: > "$OUT/about.txt"
scale_all 10:1000 10:1000 10:02 10:x 0:5 100:2000 10: :5 10:5:3 10:10000 99999:3 5:0 > /dev/null 2>&1
check "scale_all: N:K entries run" "$(tr '\n' ' ' < "$T/scaled")" "10:1000 10:10000 5:0 "
check "scale_all: N:K not run, said" "$(grep -c '^scale: ' "$OUT/about.txt")" 9
check "scale_all: an empty or a second count, said" "$(grep -c '^scale: not a count\( of snapshots\)\?: \(10:\|:5\|10:5:3\)$' "$OUT/about.txt")" 3
check "scale_all: too many snapshots, said" "$(grep -c 'makes more than 200000 snapshots' "$OUT/about.txt")" 2
rm -f "$T/scaled"
scale_all 1:20001 1:20000 1:100000 > /dev/null
check "scale_all: K at most 20000" "$(tr '\n' ' ' < "$T/scaled")" "1:20000 "
check "scale_all: K over 20000, said" "$(grep -c 'retains more than 20000 of each dataset; not run$' "$OUT/about.txt")" 2
# A K longer than the shell's integers is refused by its length.
rm -f "$T/scaled"
scale_all 1:99999999999999999999 > /dev/null 2>&1
check "scale_all: a long K not run" "$(cat "$T/scaled" 2> /dev/null)" ""
check "scale_all: a long K said" "$(grep -cx 'scale: 1:99999999999999999999 retains more than 20000 of each dataset; not run' "$OUT/about.txt")" 1
for v in abc 012 -1; do
    ZCR_SCALE_LUA_MAX=$v
    scale_all 5 > /dev/null
    check "scale_all: ZCR_SCALE_LUA_MAX $v refused" "$ZCR_SCALE_LUA_MAX" ""
done
check "scale_all: the refusals said" "$(grep -c '^scale: ZCR_SCALE_LUA_MAX is not a count' "$OUT/about.txt")" 3
ZCR_SCALE_LUA_MAX=99999999999999999999
scale_all 5 > /dev/null
check "scale_all: a long ZCR_SCALE_LUA_MAX capped" "$ZCR_SCALE_LUA_MAX" 999999
ZCR_SCALE_LUA_MAX=0
scale_all 5 > /dev/null
check "scale_all: ZCR_SCALE_LUA_MAX 0 kept" "$ZCR_SCALE_LUA_MAX" 0
ZCR_SCALE_LUA_MAX=""
for v in abc 012 -1; do
    ZCR_SCALE_FULL_MAX=$v
    scale_all 5 > /dev/null
    check "scale_all: ZCR_SCALE_FULL_MAX $v refused" "$ZCR_SCALE_FULL_MAX" ""
done
check "scale_all: its refusals said" "$(grep -c '^scale: ZCR_SCALE_FULL_MAX is not a count' "$OUT/about.txt")" 3
ZCR_SCALE_FULL_MAX=99999999999999999999
scale_all 5 > /dev/null
check "scale_all: a long ZCR_SCALE_FULL_MAX capped" "$ZCR_SCALE_FULL_MAX" 999999
ZCR_SCALE_FULL_MAX=0
scale_all 5 > /dev/null
check "scale_all: ZCR_SCALE_FULL_MAX 0 kept" "$ZCR_SCALE_FULL_MAX" 0


exit "$fail"

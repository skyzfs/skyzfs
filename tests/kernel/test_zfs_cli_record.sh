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
printf '%s %s\n' "${LC_ALL:-unset}" "$*"
[ "$1" = hold ] && [ -n "${FAKE_SLEEP:-}" ] && sleep "$FAKE_SLEEP"
[ "$1" = list ] && exit "${FAKE_LIST_RC:-0}"
# The snapshot program's answer, when one is given.
case "$*" in
    program*skyzfs:assist) [ -n "${FAKE_PROGRAM_OUT:-}" ] && printf '%s\n' "$FAKE_PROGRAM_OUT" ;;
esac
exit 0
FAKE
cat > "$T/bin/zpool" << 'FAKE'
#!/bin/sh
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
chmod +x "$T/bin/zfs" "$T/bin/zpool" "$T/bin/mountpoint"
adapter_env
OUT=$T/pins
mkdir -p "$OUT"
: > "$OUT/index.tsv"
printf -- '-- lua\n' > "$LUA_DIR/snapshot.lua"
printf -- '-- lua\n' > "$LUA_DIR/tagged.lua"
WORK=$T/work
mkdir -p "$WORK"
SCALE_END=$(($(date +%s) + 600))
record_pool rig skzq > /dev/null 2> "$T/pins.err"
record_none none skznone > /dev/null 2>> "$T/pins.err"
export FAKE_LIST_RC=1 # the program took no @t: the CLI's
export FAKE_MOUNTED=1 ZCR_WRITER_MARGIN=0.3
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
ZCR_SCALE_LIMIT=7 scale 3 > /dev/null 2>> "$T/pins.err"
unset FAKE_LIST_RC FAKE_MOUNTED FAKE_ZPOOL_CALLS
check "pins: no name twice" "$(grep -c twice "$T/pins.err")" 0
words() { tr '\0' ' ' < "$OUT/$1/argv"; }
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
# Each full record after what it reads: odd after the takes of o0 and
# o2, and before their destroy.
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
check "writer: none beside the others" "$(find "$OUT" -name writer.tsv | wc -l)" 2
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
            [ "${#w[@]}" = 4 ] && [ "${w[2]}" = -r ] && [[ ${w[3]} == *@* ]] || bad="$bad destroy:$n"
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
check "budget: nothing after it" "$([ -e "$OUT/scale-6-tagged" ] && echo yes || echo no)" no
check "budget: said" "$(grep -c '^scale 6: the budget was spent before tagged' "$OUT/about.txt")" 1
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
ZCR_SCALE_LUA_MAX=9 scale 9 > /dev/null 2>> "$T/pins.err"
check "lua max: at the count itself, run" "$(words scale-9-snapshot)" "zfs program $P skzqs - skzqs t skyzfs:assist "
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
check "full read beside: the takes' writers as before" "$(find "$OUT" -name writer.tsv | sed 's|.*/\(scale-[0-9]*-[a-z-]*\)/writer.tsv|\1|' | sort | tr '\n' ' ')" "scale-11-snapshot scale-11-take-cli scale-11-take-cli-full scale-12-snapshot scale-12-take-cli "
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
for r in snapshot take-cli; do
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
# The settle sync: a `zpool sync` after the take's probe stopped and
# before the next probe began.
s0=$(tail -n 1 "$OUT/scale-3-snapshot/txg.tsv" | cut -f2)
s1=$(head -n 1 "$OUT/scale-3-take-cli/txg.tsv" | cut -f2)
check "settle beside snapshot: a sync after the probe" "$(awk -v a="$s0" -v b="$s1" '$2 == "sync" && $3 == "skzqs" && $1 >= a && $1 < b' "$T/zpool.calls" | wc -l)" 1
s0=$(tail -n 1 "$OUT/scale-3-take-cli/txg.tsv" | cut -f2)
check "settle beside take-cli: a sync after the probe" "$(awk -v a="$s0" '$2 == "sync" && $3 == "skzqs" && $1 >= a' "$T/zpool.calls" | wc -l)" 1
for r in members take-cli-members; do
    check "members line: $r" "$(grep -c "^scale: scale-3-$r: " "$OUT/about.txt")" 1
done
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
check "no lua: main needs members and tagged alone" "$(grep -v '^ *#' "$HERE/zfs-cli-record.sh" | grep -c '^    for f in members tagged; do$')" 1

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
check "the take's create: -u, each" "$(grep -c '^C create -u skzqs/d[123]$' "$OUT/create-skzqs-3.log")" 3

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

# scale_all: at most three counts, each at most 100000, each once.
# shellcheck disable=SC2329 # run through scale_all
scale() { echo "$1" >> "$T/scaled"; }
scale_all 5 100001 99999999999999999999 5 abc 012 7 8 9 > /dev/null
check "scale_all: the counts run" "$(tr '\n' ' ' < "$T/scaled")" "5 7 8 "
check "scale_all: what was not run, said" "$(grep -c '^scale: ' "$OUT/about.txt")" 6
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

exit "$fail"

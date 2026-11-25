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
exit 0
FAKE
cat > "$T/bin/zpool" << 'FAKE'
#!/bin/sh
printf '%s %s\n' "${LC_ALL:-unset}" "$*"
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
ZCR_SCALE_LIMIT=7 scale 3 > /dev/null 2>> "$T/pins.err"
unset FAKE_LIST_RC FAKE_MOUNTED
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
# The writer beside each take: its log and its stall, in the record
# and in about.txt.
for r in snapshot take-cli; do
    check "writer beside $r: its log" "$(head -n 1 "$OUT/scale-3-$r/writer.tsv" | cut -f1)" start
    check "writer beside $r: its stall" "$(cut -c1-8 "$OUT/scale-3-$r/stall")" "longest="
    check "writer beside $r: said" "$(grep -c "^scale: writer beside scale-3-$r: longest=" "$OUT/about.txt")" 1
done
check "writer: its file in the root" "$([ -s "$WORK/mnt/skzqs/writer.dat" ] && echo yes || echo no)" yes
check "writer: nothing left behind" "$(find "$OUT" -maxdepth 1 -name '.writer-*' | wc -l)" 0
check "writer: none beside the others" "$(find "$OUT" -name writer.tsv | wc -l)" 2
check "scale's limit is its own" "$LIMIT" 5
check "scale's image removed" "$([ -e "$WORK/scale.img" ] && echo yes || echo no)" no
check "scale's df" "$(grep -c '^scale 3: df: ' "$OUT/about.txt")" 2
# The records meant to show one message say whether they did.
for r in snapshot-2 members-absent hold-2 release-2 snapshot-exists-child destroy-absent; do
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
for k in program hold holds destroy status; do
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
scale 8 > /dev/null 2>> "$T/pins.err"
check "budget spent: not run" "$(grep -c '^scale 8: not run' "$OUT/about.txt")" 1
check "budget spent: no pool" "$(find "$OUT" -maxdepth 1 -name 'scale-8-*' | wc -l)" 0
check "not mounted: no writer, said" "$(grep -c '^scale 4: the root is not mounted; no writer' "$OUT/about.txt")" 1
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

exit "$fail"

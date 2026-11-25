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
check "index order" "$(cut -f1 "$OUT/index.tsv" | tr '\n' ' ')" "r-said r-in r-noin r-exp r-exp-no r-slow c-yes c-no c-hang p-ro p-rw p-loc "

# The commands as the adapter runs them (crates/skyzfs-agent, src/zfs.rs
# and zfs/cli.rs), written out here: agent_commands and record_pool on
# fake zpool and zfs that print their words. A word the recorder changes
# fails here.
unset -f zfs
cat > "$T/bin/zpool" << 'FAKE'
#!/bin/sh
echo "scrub in progress"
FAKE
chmod +x "$T/bin/zpool"
OUT=$T/pins
mkdir -p "$OUT"
: > "$OUT/index.tsv"
printf -- '-- lua\n' > "$LUA_DIR/snapshot.lua"
printf -- '-- lua\n' > "$LUA_DIR/tagged.lua"
record_pool rig skzq > /dev/null 2> "$T/pins.err"
agent_commands none skznone > /dev/null 2>> "$T/pins.err"
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
check "pin the odd one in C.UTF-8" "$(words rig-snapshot-odd-utf8)" "env LC_ALL=C.UTF-8 zfs program $P skzq - skzq o2 skyzfs:assist "
check "pin the odd one as the adapter" "$(words rig-snapshot-odd)" "zfs program $P skzq - skzq o0 skyzfs:assist "
check "pin -g partners" "$(for r in scrub-paused scrub-canceled; do words "rig-$r-g"; done)" "zpool status -P -g skzq zpool status -P -g skzq "
check "pin none's assist" "$(words none-assist-set)" "zfs set skyzfs:assist=1700000000001 skznone "
# The adapter runs every command with LC_ALL=C (src/host.rs); main
# exports it.
check "pin the locale" "$(grep -c '^    export LC_ALL=C$' "$HERE/zfs-cli-record.sh")" 1

exit "$fail"

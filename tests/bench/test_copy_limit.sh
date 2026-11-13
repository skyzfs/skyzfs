#!/bin/bash
# The copy onto the daemon's device under a limit, and its repetition
# (topology-bench.sh, M1.T24), with no kernel: the functions are taken
# out of the script and run against stand-ins for dd, sync, the daemon
# and its status.
#
#     bash tests/bench/test_copy_limit.sh
#
# The checks are strings that `check` evaluates later, and the variables
# set here are read by the functions under test.
# shellcheck disable=SC2016,SC2034

set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
script=$here/topology-bench.sh
fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

tt=$(mktemp -d)
trap 'rm -rf -- "$tt"' EXIT
# The functions, each from its first line to the "}" that ends it.
for f in skz_create skz_hang_evidence skz_copy skz_copy_repeat; do
    awk -v f="$f" '$0 ~ "^" f "\\(\\) \\{" { on = 1 } on { print } on && /^}/ { exit }' "$script"
done > "$tt/funcs.sh"
for f in skz_create skz_hang_evidence skz_copy skz_copy_repeat; do
    grep -q "^$f() {" "$tt/funcs.sh" || { echo "FAIL $f not found in $script"; exit 1; }
done

# The world the functions expect.
setup() {
    WORK=$tt/work; OUT=$tt/out; rm -rf -- "$WORK" "$OUT"; mkdir -p "$WORK" "$OUT"
    head -c 3145728 /dev/zero > "$WORK/zfs-origin.img"
    SKZ_DIR=$WORK/vol; SKZ_RUN=$WORK/run; SKZ_PID=$$; SKZ_CGROUP=""; SKZ_DEV=/dev/null; SKZ_UP=""
    SKYZFSD_ARGS="--store,in-place"; SKYZFSD_CREATE_ARGS=""; SKYZFSD_MEM=1G; TB_SKYZFSD_REF="test"
    SKZ_SYSRQ=$tt/sysrq; : > "$SKZ_SYSRQ"
    : > "$tt/calls"
}
# shellcheck source=tests/lib/lim.sh
. "$here/../lib/lim.sh"
log() { echo "log: $*" >> "$tt/log"; }
skz_status() { echo "writes 7"; }
skz_stat() { skz_status | awk -v k="$1" '$1 == k { print $2 }'; }
skz_start() { echo start >> "$tt/calls"; SKZ_DEV=/dev/null; return 0; }
skz_stop() { echo stop >> "$tt/calls"; rm -rf -- "$SKZ_DIR" "$SKZ_RUN"; }
# dd: hangs on the call named by $tt/hang-on (1-based), else ends at once.
fake_dd() {
    local n
    n=$(( $(cat "$tt/dd-count" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$tt/dd-count"
    if [ "$n" = "$(cat "$tt/hang-on" 2>/dev/null)" ]; then sleep 30; fi
    return 0
}
DD=fake_dd
SKYZFSD=$tt/skyzfsd
printf '#!/bin/sh\n[ "$1" = create ] && { mkdir -p "$3"; echo "$*" > "%s/create-args"; echo "created" >&2; }\nexit 0\n' "$tt" > "$SKYZFSD"; chmod +x "$SKYZFSD"
# shellcheck disable=SC1091
. "$tt/funcs.sh"

# 1. A copy that ends: 0, and the times.
setup; SKZ_COPY_LIMIT_S=5; rm -f "$tt/hang-on" "$tt/dd-count"
skz_copy /dev/null; rc=$?
check "a copy that ends returns 0" '[ "$rc" = 0 ]'
check "its copy and sync times are numbers" '[[ $SKZ_COPY_MS =~ ^[0-9]+$ && $SKZ_SYNC_MS =~ ^[0-9]+$ ]]'
check "no evidence is taken" '[ ! -e "$OUT/skyzfsd-hang" ]'

# 2. A copy that does not end: 1 within the limit, and the evidence.
setup; SKZ_COPY_LIMIT_S=1; echo 1 > "$tt/hang-on"; rm -f "$tt/dd-count"
s=$(date +%s); skz_copy /dev/null; rc=$?; e=$(( $(date +%s) - s ))
check "a copy that does not end returns 1" '[ "$rc" = 1 ]'
check "it returns within the limit and the look (here $e s)" '[ "$e" -le 15 ]'
for f in why.txt status.txt threads.txt ublk.txt memory.txt serve-log-tail.txt dmesg.txt; do
    check "the evidence has $f" '[ -e "$OUT/skyzfsd-hang/$f" ]'
done
check "why.txt names the copy" 'grep -q "the copy" "$OUT/skyzfsd-hang/why.txt"'
check "the daemon's threads are listed" 'grep -q "^== $$ " "$OUT/skyzfsd-hang/threads.txt"'
check "the blocked tasks were asked for (sysrq w)" '[ "$(cat "$SKZ_SYSRQ")" = w ]'
check "the status was read" 'grep -q "writes 7" "$OUT/skyzfsd-hang/status.txt"'

# 3. Repeated, every copy ends.
setup; SKZ_COPY_LIMIT_S=5; SKZ_COPY_REPEAT=3; rm -f "$tt/hang-on" "$tt/dd-count"
skz_copy_repeat; rc=$?
check "three repetitions that end return 0" '[ "$rc" = 0 ]'
check "the table has three rows, all ok" '[ "$(awk -F"\t" "NR > 1 && \$4 == \"ok\"" "$OUT/copy-repeat.tsv" | wc -l)" = 3 ]'
check "each had its own daemon" '[ "$(grep -c start "$tt/calls")" = 3 ] && [ "$(grep -c stop "$tt/calls")" = 3 ]'
check "the summary says all ended" 'grep -q "All 3 ended" "$OUT/summary.md"'

# 4. Repeated, the second does not end: the repetitions stop there.
setup; SKZ_COPY_LIMIT_S=1; SKZ_COPY_REPEAT=3; echo 2 > "$tt/hang-on"; rm -f "$tt/dd-count"
skz_copy_repeat; rc=$?
check "a repetition that does not end returns 1" '[ "$rc" = 1 ]'
check "the table stops at the second" '[ "$(tail -n +2 "$OUT/copy-repeat.tsv" | wc -l)" = 2 ]'
check "the second is marked" 'awk -F"\t" "\$1 == 2" "$OUT/copy-repeat.tsv" | grep -q "RAN OUT"'
check "the summary names it" 'grep -q "Did not end: repetition 2 of 3" "$OUT/summary.md"'
check "its evidence was taken" '[ -e "$OUT/skyzfsd-hang/threads.txt" ]'
check "the hung daemon is left for the teardown (two starts, one stop)" '[ "$(grep -c start "$tt/calls")" = 2 ] && [ "$(grep -c stop "$tt/calls")" = 1 ]'

# 5. The create's own arguments, and what it said kept.
setup; SKYZFSD_CREATE_ARGS="--zero-fill,--x,1"
skz_create 4096 2> /dev/null; rc=$?
check "a create with arguments returns 0" '[ "$rc" = 0 ]'
check "the arguments reach the create, commas as spaces" 'grep -q -- "--size 4096 --zero-fill --x 1$" "$tt/create-args"'
check "what it said is kept" 'grep -q created "$WORK/skyzfsd-create.err"'
setup
skz_create 4096 2> /dev/null
check "with none, nothing is added" 'grep -q -- "--size 4096$" "$tt/create-args"'

if [ "$fails" = 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi

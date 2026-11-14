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
for f in skz_create skz_hang_evidence skz_copy skz_copy_repeat skz_low_bytes skz_settle; do
    awk -v f="$f" '$0 ~ "^" f "\\(\\) \\{" { on = 1 } on { print } on && /^}/ { exit }' "$script"
done > "$tt/funcs.sh"
for f in skz_create skz_hang_evidence skz_copy skz_copy_repeat skz_low_bytes skz_settle; do
    grep -q "^$f() {" "$tt/funcs.sh" || { echo "FAIL $f not found in $script"; exit 1; }
done

# The world the functions expect.
setup() {
    WORK=$tt/work; OUT=$tt/out; rm -rf -- "$WORK" "$OUT"; mkdir -p "$WORK" "$OUT"
    head -c 3145728 /dev/zero > "$WORK/zfs-origin.img"
    SKZ_DIR=$WORK/vol; SKZ_RUN=$WORK/run; SKZ_PID=$$; SKZ_CGROUP=""; SKZ_DEV=/dev/null; SKZ_UP=""
    SKYZFSD_ARGS="--io-threads,8"; SKYZFSD_CREATE_ARGS=""; SKYZFSD_MEM=1G; TB_SKYZFSD_REF="test"
    SKZ_SYSRQ=$tt/sysrq; : > "$SKZ_SYSRQ"
    : > "$tt/calls"; : > "$tt/log"
    rm -f "$tt/hang-on" "$tt/dd-count" "$tt/sync-hang-on" "$tt/sync-count"
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
# sync: the same, by $tt/sync-hang-on; the machine's own is never run.
sync() {
    local n
    n=$(( $(cat "$tt/sync-count" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$tt/sync-count"
    if [ "$n" = "$(cat "$tt/sync-hang-on" 2>/dev/null)" ]; then sleep 30; fi
    return 0
}
SKYZFSD=$tt/skyzfsd
printf '#!/bin/sh\n[ "$1" = create ] && { mkdir -p "$3"; echo "$*" > "%s/create-args"; echo "created on stdout"; echo "a note on stderr" >&2; }\nexit 0\n' "$tt" > "$SKYZFSD"; chmod +x "$SKYZFSD"
# shellcheck disable=SC1091
. "$tt/funcs.sh"

# 1. A copy that ends: 0, and the times.
setup; SKZ_COPY_LIMIT_S=5; rm -f "$tt/hang-on" "$tt/dd-count"
skz_copy /dev/null; rc=$?
check "a copy that ends returns 0" '[ "$rc" = 0 ]'
check "its copy and sync times are numbers" '[[ $SKZ_COPY_MS =~ ^[0-9]+$ && $SKZ_SYNC_MS =~ ^[0-9]+$ ]]'
check "no evidence is taken" '[ ! -e "$OUT/skyzfsd-hang" ]'
check "the sync ran once, the stand-in and not the machine's" '[ "$(cat "$tt/sync-count")" = 1 ]'

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
check "it says the daemon is alive" 'grep -q "daemon pid $$: alive" "$OUT/skyzfsd-hang/why.txt"'
check "the log's count of tasks in D state is the file's, without its header" \
    '[ "$(sed -n "s/.* threads, \([0-9]*\) in D state.*/\1/p" "$tt/log")" = "$(awk "\$1 == \"==\" && \$5 == \"D\"" "$OUT/skyzfsd-hang/threads.txt" | wc -l)" ]'
check "no sync ran after a copy that ran out" '[ ! -e "$tt/sync-count" ]'
setup; SKZ_COPY_LIMIT_S=1; echo 1 > "$tt/hang-on"; rm -f "$tt/dd-count"; SKZ_PID=999999999
skz_copy /dev/null
check "and that it is gone when it is" 'grep -q "daemon pid 999999999: GONE" "$OUT/skyzfsd-hang/why.txt"'

# 2b. The copy ends and its sync does not: 1, and the evidence names the sync.
setup; SKZ_COPY_LIMIT_S=1; echo 1 > "$tt/sync-hang-on"
s=$(date +%s); skz_copy /dev/null; rc=$?; e=$(( $(date +%s) - s ))
check "a sync that does not end returns 1" '[ "$rc" = 1 ]'
check "it returns within the limit and the look (here $e s)" '[ "$e" -le 15 ]'
check "why.txt names the sync" 'grep -q "the sync after the copy" "$OUT/skyzfsd-hang/why.txt"'
check "the copy's time is kept" '[[ $SKZ_COPY_MS =~ ^[0-9]+$ ]]'

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

# 4b. Repeated, the second sync does not end: the same.
setup; SKZ_COPY_LIMIT_S=1; SKZ_COPY_REPEAT=3; echo 2 > "$tt/sync-hang-on"
skz_copy_repeat; rc=$?
check "a repetition whose sync does not end returns 1" '[ "$rc" = 1 ]'
check "the table stops at the second, marked" '[ "$(tail -n +2 "$OUT/copy-repeat.tsv" | wc -l)" = 2 ] && awk -F"\t" "\$1 == 2" "$OUT/copy-repeat.tsv" | grep -q "RAN OUT"'
check "its evidence names the sync" 'grep -q "the sync after the copy" "$OUT/skyzfsd-hang/why.txt"'

# 5. The create's own arguments, and what it said kept.
setup; SKYZFSD_CREATE_ARGS="--zero-fill,--x,1"
skz_create 4096 2> "$tt/create-said"; rc=$?
check "a create with arguments returns 0" '[ "$rc" = 0 ]'
check "the arguments reach the create, commas as spaces" 'grep -q -- "--size 4096 --zero-fill --x 1$" "$tt/create-args"'
check "what it said on stdout is kept and passed on" 'grep -q "created on stdout" "$WORK/skyzfsd-create.out" && grep -q "created on stdout" "$tt/create-said"'
check "what it said on stderr is kept and passed on" 'grep -q "a note on stderr" "$WORK/skyzfsd-create.err" && grep -q "a note on stderr" "$tt/create-said"'
SKZ_COPY_REPEAT=1; skz_copy_repeat > /dev/null
check "the repetition's summary names the create's arguments" 'grep -q "create --zero-fill --x 1;" "$OUT/summary.md"'
setup
skz_create 4096 2> /dev/null
check "with none, nothing is added" 'grep -q -- "--size 4096$" "$tt/create-args"'

# 6. The label of a row after failed rows (TB_EARLIER_FAILED, set by the
# workflow), taken out of the script like the functions.
sed -n '/^LABEL=\${TB_LABEL/,/^esac/p' "$script" > "$tt/label.sh"
check "the label's lines are found" 'grep -q "^esac" "$tt/label.sh"'
# shellcheck disable=SC1091  # written above, from the script
label_for() { ( TB_LABEL=$1; TB_EARLIER_FAILED=$2; . "$tt/label.sh"; echo "$LABEL" ) 2>&1; }
check "no failed row: the label is as given" '[ "$(label_for cur "")" = cur ]'
check "one failed row" '[ "$(label_for cur 2)" = "cur; after failed row 2" ]'
check "two failed rows" '[ "$(label_for cur "2 5")" = "cur; after failed rows 2,5" ]'
check "no label of its own" '[ "$(label_for "" 3)" = "after failed row 3" ]'
check "anything but row numbers is refused" '( TB_LABEL=x; TB_EARLIER_FAILED="1;x"; . "$tt/label.sh" ) 2> /dev/null; [ $? = 64 ]'

# 7. The settle after the copy (skz_settle), on a status in a file. The
# clock moves 10 s a call and sleep does nothing, so 600 s pass at once.
# Since M1.3t the status may have no log keys: absent counts as at rest,
# but only with held 0 and the write count still; a status that cannot
# be read never counts.
settle() { # status lines -> rc; what it said in $tt/settle-said
    printf '%s\n' "$@" > "$tt/status"; echo 0 > "$tt/clock"; : > "$tt/log"; : > "$OUT/t.skzinfo"
    # shellcheck disable=SC2317  # the stand-ins are called by skz_settle
    (
        TOPO=t
        skz_status() { cat "$tt/status"; }
        sleep() { :; }
        date() { local n; n=$(( $(cat "$tt/clock") + 10 )); echo "$n" > "$tt/clock"; echo "$n"; }
        skz_settle
    ) 2> "$tt/settle-said"
}
setup; SKYZFSD_ARGS=""
settle "writes 7" "held 0"; rc=$?
check "no log keys, nothing held, writes still: at rest" '[ "$rc" = 0 ] && grep -q "the status has no log" "$OUT/t.skzinfo"'
settle "writes 7" "held 0" "checkpoint_step none" "unstored 0" "log_live 0"; rc=$?
check "the log keys at rest: at rest, under the low mark" '[ "$rc" = 0 ] && grep -q "log_live 0 under the low mark" "$OUT/t.skzinfo"'
settle "writes 7" "held 1"; rc=$?
check "a request held: never at rest" '[ "$rc" = 1 ] && grep -q "did not come to rest" "$tt/settle-said"'
settle "writes 7" "held 0" "checkpoint_step under way" "unstored 0" "log_live 0"; rc=$?
check "a step under way: never at rest" '[ "$rc" = 1 ]'
settle ""; rc=$?
check "a status that cannot be read: never at rest" '[ "$rc" = 1 ] && grep -q "did not come to rest" "$tt/settle-said"'

if [ "$fails" = 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi

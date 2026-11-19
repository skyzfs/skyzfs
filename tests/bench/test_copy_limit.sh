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
for f in skz_create skz_hang_evidence skz_copy skz_copy_repeat skz_low_bytes skz_settle skz_first_upload skz_seed_watch_stop skz_minio_resume skz_minio_freeze; do
    awk -v f="$f" '$0 ~ "^" f "\\(\\) \\{" { on = 1 } on { print } on && /^}/ { exit }' "$script"
done > "$tt/funcs.sh"
for f in skz_create skz_hang_evidence skz_copy skz_copy_repeat skz_low_bytes skz_settle skz_first_upload skz_seed_watch_stop skz_minio_resume skz_minio_freeze; do
    grep -q "^$f() {" "$tt/funcs.sh" || { echo "FAIL $f not found in $script"; exit 1; }
done

# The world the functions expect.
setup() {
    WORK=$tt/work; OUT=$tt/out; rm -rf -- "$WORK" "$OUT"; mkdir -p "$WORK" "$OUT"
    head -c 3145728 /dev/zero > "$WORK/zfs-origin.img"
    SKZ_DIR=$WORK/vol; SKZ_RUN=$WORK/run; SKZ_PID=$$; SKZ_CGROUP=""; SKZ_DEV=/dev/null; SKZ_UP=""
    SKYZFSD_ARGS="--io-threads,8"; SKYZFSD_CREATE_ARGS=""; SKYZFSD_MEM=1G; TB_SKYZFSD_REF="test"
    SKZ_UPLOAD=""; SKZ_VOLUME_NAME=""
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

# 5b. TB_SKZ_UPLOAD (M2.4c): the volume is made in object storage too,
# under a new name each create, after the create's own arguments.
setup; SKZ_UPLOAD=1; SKYZFSD_CREATE_ARGS="--zero-fill"
TB_S3_ENDPOINT=http://127.0.0.1:9000; TB_S3_BUCKET=tb-9-1
skz_create 4096 2> /dev/null; first=$SKZ_VOLUME_NAME
check "an upload's create names the place, after the create's arguments" \
    'grep -qE -- "--size 4096 --zero-fill --endpoint http://127.0.0.1:9000 --bucket tb-9-1 --volume tb-[0-9]+$" "$tt/create-args"'
check "the volume's name is the one the create was given" 'grep -q -- "--volume $first$" "$tt/create-args"'
rm -rf -- "$SKZ_DIR"; skz_create 4096 2> /dev/null
check "each create a new name" '[ -n "$first" ] && [ "$SKZ_VOLUME_NAME" != "$first" ]'

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
    # shellcheck disable=SC2317,SC2329  # the stand-ins are called by skz_settle
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

# 8. TB_SKZ_UPLOAD: the wait for the first upload before the windows
# (skz_first_upload), on a status that changes with the clock. The clock
# moves 10 s a call; the status reads uploaded 0 until UP_AT s.
# DEAD_UNTIL: the status is not read before that time (a daemon gone).
# BEGAN: the time the workload began (run_workload's file), or none.
first_upload() { # up_at wait_s [watch] -> rc; the row's line in $OUT/t.skzinfo
    echo "$1" > "$tt/up_at"; echo "${DEAD_UNTIL:-0}" > "$tt/dead"; echo 0 > "$tt/clock"; : > "$tt/log"; : > "$OUT/t.skzinfo"
    rm -f "$tt/began"; [ -z "${BEGAN:-}" ] || echo "$BEGAN" > "$tt/began"
    # shellcheck disable=SC2317,SC2329  # the stand-ins are called by skz_first_upload
    (
        TOPO=t; SKZ_UPLOAD_WAIT_S=$2; SKZ_SEED_BEGAN=$tt/began
        skz_status() {
            local up=0
            [ "$(cat "$tt/clock")" -ge "$(cat "$tt/dead")" ] || return 1
            [ "$(cat "$tt/clock")" -lt "$(cat "$tt/up_at")" ] || up=1
            printf 'uploaded %s\nupload_state reading\nupload_cuts_lost 4\nupload_cut 9\ncuts 12\n' "$up"
        }
        sleep() { :; }
        date() { local n; n=$(( $(cat "$tt/clock") + 10 )); echo "$n" > "$tt/clock"; echo "$n"; }
        skz_first_upload "${3:-}"
    ) 2> /dev/null
}
setup
first_upload 100 900; rc=$?
check "the first upload lands: the row says when" \
    '[ "$rc" = 0 ] && grep -qE "^first_upload after [0-9]+ s \(uploaded 1, cuts lost 4\)$" "$OUT/t.skzinfo"'
first_upload 100000 300; rc=$?
check "no upload in the limit: the row says so, and the run goes on" \
    '[ "$rc" = 0 ] && grep -qx "first_upload NONE in 300 s (state reading, uploading cut 9 of 12, cuts lost 4)" "$OUT/t.skzinfo"'
# The wait comes after the pool's import and a sync: a cut is taken at
# a flush, and none comes while the image is copied on.
check "the wait follows the import and a sync, on the daemon's upload rows only" \
    'awk "/^    zpool import -N/ { i = NR } /^        lim 120 zpool sync/ && !y { y = NR } /^        skz_first_upload\$/ { w = NR }
          /if \\[ \"\\\$1\" = skyzfsd \\] && \\[ \"\\\$SKZ_UPLOAD\" = 1 \\]/ && !c { c = NR }
          END { exit !(i && c > i && y == c + 1 && w == y + 1) }" "$script"'
check "the wait is called nowhere else" '[ "$(grep -c "^ *skz_first_upload$" "$script")" = 1 ]'
# With no wait the watcher runs beside the row (run 44372469282: the
# seed committed during the setup, while the line said "under the
# workload"). The clock reads 10 at the start; each round takes the
# read's start, reads, and takes its end: reads at 20, 40, ..., 100, so
# up at 100 is seen at the read of 100, after the read of 80 showed
# nothing, and written at 110. The commit came in (80, 110].
first_upload 100 3600 watch; rc=$?
check "the seed watched: it says first that nothing has committed" \
    '[ "$rc" = 0 ] && [ "$(head -n 1 "$OUT/t.skzinfo")" = "first_upload not committed by the window'"'"'s end (watched from the import)" ]'
check "  and with no workload begun, that it committed before the workload" \
    '[ "$(tail -n 1 "$OUT/t.skzinfo")" = "first_upload after 100 s from the import, before the workload began (uploaded 1, cuts lost 4)" ]'
BEGAN=50 first_upload 100 3600 watch; rc=$?
check "  a workload begun before the read before the commit: under it, and how long after" \
    '[ "$(tail -n 1 "$OUT/t.skzinfo")" = "first_upload after 100 s from the import, under the workload, 30 to 60 s after its warm-up began (uploaded 1, cuts lost 4)" ]'
BEGAN=79 first_upload 100 3600 watch; rc=$?
check "  begun the second before that read's start: under it" \
    'grep -qx "first_upload after 100 s from the import, under the workload, 1 to 31 s after its warm-up began (uploaded 1, cuts lost 4)" "$OUT/t.skzinfo"'
# Review's N1 on #123: a mark between the read before and the read that
# saw the commit does not say whether the commit came under the load.
# Nor does a mark in the read before's own second (review's D2 on #128):
# times are to the second, and that read may have begun before it.
for b in 80 81 90 110; do
    BEGAN=$b first_upload 100 3600 watch; rc=$?
    check "  begun at $b, between the two reads: as the workload began" \
        'grep -qx "first_upload after 100 s from the import, as the workload began (within 30 s of its warm-up) (uploaded 1, cuts lost 4)" "$OUT/t.skzinfo"'
done
BEGAN=111 first_upload 100 3600 watch; rc=$?
check "  begun after the read that saw it: before" \
    'grep -qx "first_upload after 100 s from the import, before the workload began (uploaded 1, cuts lost 4)" "$OUT/t.skzinfo"'
# A round whose status was not read shows nothing: the read before the
# commit is the last one that was read. Reads at 20 and 40 unread, at 60
# read and up: the commit came in (10, 70], the watcher's start to the
# read's end, and the mark at 30 lies inside.
DEAD_UNTIL=45 BEGAN=30 first_upload 60 3600 watch; rc=$?
check "  unread rounds before the commit do not count as reads that showed none" \
    'grep -qx "first_upload after 60 s from the import, as the workload began (within 60 s of its warm-up) (uploaded 1, cuts lost 4)" "$OUT/t.skzinfo"'
BEGAN=500 first_upload 100 3600 watch; rc=$?
check "  a workload that began after the commit: before it" \
    'grep -qx "first_upload after 100 s from the import, before the workload began (uploaded 1, cuts lost 4)" "$OUT/t.skzinfo"'
BEGAN=x first_upload 100 3600 watch; rc=$?
check "  a time that is not a number: not under the workload" \
    'grep -q "before the workload began" "$OUT/t.skzinfo"'
BEGAN=50 first_upload 100000 3600 watch; rc=$?
check "a seed that never lands says so" \
    'grep -qx "first_upload NONE in 3600 s from the import (state reading, uploading cut 9 of 12, cuts lost 4)" "$OUT/t.skzinfo"'
DEAD_UNTIL=100000 first_upload 10 900; rc=$?
check "a status not read three times running ends the wait, and says so" \
    '[ "$rc" = 0 ] && grep -qE "^first_upload NONE after [0-9]+ s: the status could not be read three times running$" "$OUT/t.skzinfo"'
DEAD_UNTIL=100000 first_upload 10 3600 watch; rc=$?
check "  the watcher too" \
    'grep -qE "^first_upload NONE after [0-9]+ s from the import: the status could not be read three times running$" "$OUT/t.skzinfo"'
# The clock moves 20 s a round, reads at 20, 40, 60: the first two
# unread, the third read.
DEAD_UNTIL=45 first_upload 10 900; rc=$?
check "  a status unread two rounds running: the wait goes on to the upload" \
    'grep -qE "^first_upload after [0-9]+ s \(uploaded 1, cuts lost 4\)$" "$OUT/t.skzinfo"'
# The row keeps the last value of a key: the watcher's first line until
# the commit's. (test_embedded_python.py runs it on such a record.)
check "a row reads the last first_upload of its .skzinfo" \
    'grep -q "if key: info\[key\] = value" "$script"'
check "with no wait, the watcher runs in the background and the teardown stops it" \
    'grep -q "( SKZ_UPLOAD_WAIT_S=3600; skz_first_upload watch ) &" "$script" && grep -q "kill \"\$SKZ_SEED_WATCH\"" "$script"'
# The teardown's block (review's N2 on #123): a watcher still running
# leaves NONE in the record and is stopped; one that has ended wrote its
# own line, and nothing is added.
seed_stop() { # watcher-pid-or-empty -> the record's lines in $tt/info
    : > "$tt/info"
    ( SKZ_SEED_WATCH=$1; SKZ_SEED_T0=$(( $(date +%s) - 42 )); SKZ_SEED_INFO=$tt/info
      skz_seed_watch_stop; echo "cleared:${SKZ_SEED_WATCH}" > "$tt/cleared" )
}
sleep 60 & live=$!
seed_stop "$live"
check "  a watcher still running at the teardown: the record says NONE by the end of the rows" \
    'grep -qxE "first_upload NONE by the end of the rows \((42|43) s after the import\)" "$tt/info" && [ "$(wc -l < "$tt/info")" = 1 ]'
sleep 1
check "    and it is stopped" '! kill -0 "$live" 2> /dev/null && [ "$(cat "$tt/cleared")" = cleared: ]'
wait "$live" 2> /dev/null
true & ended=$!; wait "$ended"
seed_stop "$ended"
check "  a watcher that has ended: nothing is added" '[ ! -s "$tt/info" ] && [ "$(cat "$tt/cleared")" = cleared: ]'
seed_stop ""
check "  no watcher: nothing is added" '[ ! -s "$tt/info" ]'
check "  the teardown calls it" \
    '[ "$(sed -n "/^teardown_topology() {/,/^}$/p" "$script" | grep -c "^    skz_seed_watch_stop$")" = 1 ]'
rw=$(sed -n '/^run_workload() {/,/^}$/p' "$script")
bz=$(sed -n '/^build_zfs_direct() {/,/^}$/p' "$script")
# TB_SKZ_UPLOAD_FROM_LOAD: MinIO, frozen from after the create, answers
# again once the workload begins, after the seed's watcher has its mark.
minio_resume() { # paused unpause-rc -> "calls|paused after" ; log in $tt/log
    : > "$tt/minio"; : > "$tt/log"
    # shellcheck disable=SC2317,SC2329  # the stand-in is called by skz_minio_resume
    ( SKZ_MINIO_PAUSED=$1
      skz_minio() { echo "$1" >> "$tt/minio"; return "$MINIO_RC"; }
      MINIO_RC=$2 skz_minio_resume; echo "$(tr '\n' ' ' < "$tt/minio")|$SKZ_MINIO_PAUSED" > "$tt/resumed" )
    cat "$tt/resumed"
}
check "from the load: a frozen MinIO is let go as the workload begins" \
    '[ "$(minio_resume 1 0)" = "unpause |" ] && grep -q "MinIO answers again" "$tt/log"'
check "  one that would not go on is said, and left for the teardown" \
    '[ "$(minio_resume 1 1)" = "unpause |1" ] && grep -q "WARNING: MinIO could not be let go" "$tt/log"'
check "  not frozen: nothing is called" '[ "$(minio_resume "" 0)" = "|" ]'
# The line numbers of the first line holding each text, in order.
order() { # text with fixed strings... (stdin: the function) -> "n,n,..."
    local body t; body=$(cat)
    # A text not found gives an empty field, which is not in order.
    for t in "$@"; do printf "%s\n" "$(printf "%s\n" "$body" | grep -n -m1 -F -e "$t" | cut -d: -f1)"; done | tr "\n" ,
}
increasing() { awk -F, '{ for (i = 2; i < NF; i++) if ($i == "" || $i <= $(i - 1)) exit 1; exit ($1 == "") }' <<< "$1"; }
mark_at='date +%s > "$SKZ_SEED_BEGAN"'; warm_at='log "$tag: warm-up'
check "  run_workload lets it go after the mark, before the first warm-up" \
    'increasing "$(printf "%s\n" "$rw" | order "$mark_at" "    skz_minio_resume" "$warm_at")"'
check "  frozen after the create and before the daemon starts, and only from the load" \
    '[ "$(printf "%s\n" "$bz" | grep -n -E "^ *skz_create |skz_minio_freeze|^ *skz_start$" | cut -d: -f2 | sed "s/^ *//" | tr "\n" ,)" = "skz_create \"\$(stat -c %s \"\$WORK/zfs-origin.img\")\" \\,if [ -n \"\$SKZ_UPLOAD_FROM_LOAD\" ]; then skz_minio_freeze || return 1; fi,skz_start," ]'
# The freeze itself (review's T1 on #128), MinIO stood in for: the flag
# before the pause, the record's line, and a failed pause.
minio_freeze() { # pause-rc -> "calls|paused|rc" ; the record in $tt/info
    : > "$tt/minio"; : > "$tt/log"; : > "$tt/info"
    # shellcheck disable=SC2317,SC2329  # the stand-in is called by skz_minio_freeze
    ( SKZ_MINIO_PAUSED=""; OUT=$tt; TOPO=info0
      skz_minio() { echo "$1:${SKZ_MINIO_PAUSED:-unset}" >> "$tt/minio"; return "$MINIO_RC"; }
      MINIO_RC=$1 skz_minio_freeze 2> "$tt/err"; rc=$?
      echo "$(tr '\n' ' ' < "$tt/minio")|$SKZ_MINIO_PAUSED|$rc" > "$tt/froze" )
    mv -f "$tt/info0.skzinfo" "$tt/info" 2> /dev/null || true
    cat "$tt/froze"
}
check "  the freeze sets its flag before the pause and says so in the record" \
    '[ "$(minio_freeze 0)" = "pause:1 |1|0" ] && [ "$(cat "$tt/info")" = "upload_from_load 1" ] && grep -q "MinIO frozen until the workload begins" "$tt/log"'
check "  a pause that fails: the row fails, the flag stays for the teardown, and the record says nothing" \
    '[ "$(minio_freeze 1)" = "pause:1 |1|1" ] && [ ! -s "$tt/info" ] && grep -q "MinIO could not be frozen" "$tt/err"'
unpause_at='if [ -n "$SKZ_MINIO_PAUSED" ]; then skz_minio unpause || true; SKZ_MINIO_PAUSED=""; fi'
check "  the teardown lets a frozen MinIO go" \
    '[ "$(sed -n "/^teardown_topology() {/,/^}$/p" "$script" | grep -cxF -e "    $unpause_at")" = 1 ]'
# The watcher's record and clock are set before it starts (review's W8
# and W9 on #128): otherwise its NONE goes to /dev/null and counts from 0.
check "the watcher's record and clock are set before it starts" \
    'increasing "$(printf "%s\n" "$bz" | order "SKZ_SEED_T0=\$(date +%s); SKZ_SEED_INFO=\"\$OUT/\$TOPO.skzinfo\"" "( SKZ_UPLOAD_WAIT_S=3600; skz_first_upload watch ) &")"'
# The mark of a row before is not read by the next (TB_WORK is the job's):
# it is removed before the watcher starts.
check "  the last row's mark is removed before the watcher starts" \
    '[ "$(printf "%s\n" "$bz" | grep -m1 -F -e "rm -f \"\$SKZ_SEED_BEGAN\"" -e "skz_first_upload watch ) &" | grep -c "rm -f")" = 1 ]'
check "  run_workload marks the start of the workload for it, before the first warm-up" \
    '[ "$(printf "%s\n" "$rw" | grep -m1 -F -e "date +%s > \"\$SKZ_SEED_BEGAN\"" -e "log \"\$tag: warm-up" | grep -c SKZ_SEED_BEGAN)" = 1 ]'

if [ "$fails" = 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi

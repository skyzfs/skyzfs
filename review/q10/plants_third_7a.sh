#!/usr/bin/env bash
# Review scratch (Q10, PR #148, third look, head 7a813d9): each plant alone on a
# fresh copy of the head's files, then the head's unit tests run on it.
# A plant of zfs-cli-record.sh runs tests/kernel/test_zfs_cli_record.sh;
# a plant of dsync_writer.py runs tests/kernel/test_dsync_writer.py and
# then the shell test (which runs the writer for real). KILLED = a test
# exits non-zero; SURVIVED = every test passes with the plant in place.
# Each plant starts from a fresh copy (the restore), and the copy is
# removed at the end whatever happens. Every test run has 600 s.
#
# A plant of txg_probe.py runs tests/kernel/test_txg_probe.py, then the
# shell test.
#
#   bash review/q10/plants_third_7a.sh HEAD WORKDIR
set -uo pipefail
HEAD=${1:?head}
W=${2:?workdir}
REPO=$(git rev-parse --show-toplevel)
mkdir -p "$W"
trap 'rm -rf "${W:?}/t"' EXIT
FILES="tests/kernel/zfs-cli-record.sh tests/kernel/test_zfs_cli_record.sh tests/kernel/dsync_writer.py
tests/kernel/test_dsync_writer.py tests/kernel/txg_probe.py tests/kernel/test_txg_probe.py
tests/lib/lim.sh tests/lib/gnu-dd.sh"
fresh() {
    rm -rf "${W:?}/t" && mkdir -p "$W/t/tests/kernel" "$W/t/tests/lib"
    local f
    for f in $FILES; do git -C "$REPO" show "$HEAD:$f" > "$W/t/$f"; done
}
run_shell() { timeout 600 bash "$W/t/tests/kernel/test_zfs_cli_record.sh" > "$W/$1.log" 2>&1; }
run_py() { timeout 600 python3 "$W/t/tests/kernel/$2.py" > "$W/$1.py.log" 2>&1; }
# plant ID TARGET(sh|py) DESCRIPTION SED-EXPRESSION
plant() {
    local id=$1 target=$2 what=$3 expr=$4 f t="" why=""
    fresh
    case $target in
        sh) f=$W/t/tests/kernel/zfs-cli-record.sh ;;
        py) f=$W/t/tests/kernel/dsync_writer.py; t=test_dsync_writer ;;
        pq) f=$W/t/tests/kernel/txg_probe.py; t=test_txg_probe ;;
    esac
    cp "$f" "$f.orig"
    sed -i -e "$expr" "$f"
    if cmp -s "$f" "$f.orig"; then
        printf '%s\tNOT APPLIED\t%s\n' "$id" "$what"
        return
    fi
    if [ -n "$t" ] && ! run_py "$id" "$t"; then
        why=$(grep -m1 -E '^(FAIL|ERROR):|Error' "$W/$id.py.log" | cut -c1-90)
        printf '%s\tKILLED\t%s\t(python: %s)\n' "$id" "$what" "$why"
        return
    fi
    if run_shell "$id"; then
        printf '%s\tSURVIVED\t%s\n' "$id" "$what"
    else
        why=$(grep -m1 '^FAIL' "$W/$id.log" | cut -c1-100)
        printf '%s\tKILLED\t%s\t(shell: %s)\n' "$id" "$what" "${why:-exit $?}"
    fi
}
fresh
run_shell base && echo "base: shell test passes ($(grep -c '^ok' "$W/base.log") ok)" || echo "base: shell test FAILS"
run_py base test_dsync_writer && echo "base: writer test passes" || echo "base: writer test FAILS"
run_py base2 test_txg_probe && echo "base: probe test passes" || echo "base: probe test FAILS"

# The survivors of the next look, their patterns brought to this head.
plant V2 sh "adapter_env: LC_ALL=C.UTF-8 exported after the pinned line" '/^    export LC_ALL=C$/a\    export LC_ALL=C.UTF-8'
plant V3 sh "rec: the command run with LC_ALL=C.UTF-8" 's/^    lim "\$LIMIT" "\$@" < "\$in" >/    LC_ALL=C.UTF-8 lim "$LIMIT" "$@" < "$in" >/'
plant V4 sh "record_pool: tagged-held without -n" 's/program "\$x-tagged-held" "\$p" tagged.lua -n /program "$x-tagged-held" "$p" tagged.lua /'
plant V5 sh "record_none: members without -n" '/^record_none() {/,/^}/s/members.lua -n /members.lua /'
plant V6 sh "record_pool: hold-2 without -r" 's/\(rec "\$x-hold-2" .*\) zfs hold -r /\1 zfs hold /'
plant V7 sh "record_pool: release-2 without -r" 's/\(rec "\$x-release-2" .*\) zfs release -r /\1 zfs release /'
plant V8 sh "record_none: hold with the old tag" '/^record_none() {/,/^}/s/rec "\$x-hold" zfs hold -r "\$TAG"/rec "$x-hold" zfs hold -r skyzfs/'
plant V9 sh "main: rig-resilvered without its -g partner" 's/^    status rig-resilvered skzq$/    rec rig-resilvered zpool status -P skzq/'
plant V17 sh "scale: members without -n" 's/program "scale-\$n-members" "\$p" members.lua -n /program "scale-$n-members" "$p" members.lua /'
plant U7 sh "catch: an earlier try's rc kept" '/^catch() {/,/^}/s/^        rc=0$/        [ "$i" = 1 ] \&\& rc=0/'
plant V19a sh "scale_limit: ZCR_SCALE_LIMIT line dropped" '/^scale_limit() {/,/^}/{/LIMIT=\${ZCR_SCALE_LIMIT:-1800}/d}'
plant V19b sh "scale: its LIMIT not local (leaks)" 's/^    local LIMIT FILL_SPAN=""$/    local FILL_SPAN=""/'
plant V19c sh "scale_limit: each command gets all that is left, not ZCR_SCALE_LIMIT" 's/^    LIMIT=\${ZCR_SCALE_LIMIT:-1800}$/    LIMIT=$left/'
plant V22a sh "during: a bare wait (waits for what lim gave up on)" 's/^    wait "\$pid"$/    wait/'
plant V22b sh "during: no wait" '/^    wait "\$pid"$/d'
plant V22c sh "during: the background record under LIMIT, not ZCR_CHANGE_LIMIT" 's/^        LIMIT=\${ZCR_CHANGE_LIMIT:-900}$/        :/'
plant V26 sh "keep: argv with spaces, not NUL" "/^keep() {/,/^}/s/printf '%s\\\\0' \"\\\$@\" > \"\\\$d\\/argv\"/printf '%s ' \"\$@\" > \"\$d\\/argv\"/"
plant V29 sh "record_pool: --expect dropped from hold-2" "s/rec \"\\\$x-hold-2\" --expect 'tag already exists' zfs/rec \"\$x-hold-2\" zfs/"
plant V30 sh "main: the rebuild's catch with 'in progress'" "s/during mirror-rebuild mirror-rebuild-status \"\\\$REBUILD_PATTERN\"/during mirror-rebuild mirror-rebuild-status 'in progress'/"
plant V30b sh "REBUILD_PATTERN reverted to 'in progress'" "s/^REBUILD_PATTERN=.*/REBUILD_PATTERN='in progress'/"

# N1: destroy -r.
plant N1a sh "record_pool: destroy-held without -r" 's/rec "\$x-destroy-held" zfs destroy -r /rec "$x-destroy-held" zfs destroy /'
plant N1b sh "record_none: destroy dropped" '/^record_none() {/,/^}/{/rec "\$x-destroy" zfs destroy -r "\$p@t"/d}'
plant N1c sh "record_pool: --expect dropped from destroy-absent" "s/rec \"\\\$x-destroy-absent\" --expect 'could not find any snapshots to destroy' /rec \"\$x-destroy-absent\" /"
plant N1d sh "record_pool: destroy-held's hold on the root, not the child" 's/rec "\$x-destroy-held-hold" zfs hold "\$TAG" "\$p\/child@o2"/rec "$x-destroy-held-hold" zfs hold "$TAG" "$p@o2"/'
plant N1e sh "record_pool: destroy-held's hold with another tag" 's/rec "\$x-destroy-held-hold" zfs hold "\$TAG"/rec "$x-destroy-held-hold" zfs hold skyzfs/'
plant N1f sh "record_pool: destroy with an extra flag (-d)" 's/rec "\$x-destroy" zfs destroy -r "\$p@o0"/rec "$x-destroy" zfs destroy -r -d "$p@o0"/'
plant N1g sh "record_pool: the hold left on @o2 not released" '/^    release_quietly "\$p" o2$/d'

# A second call site of an adapter command that no rule covers.
plant G1 sh "record_pool: get-checkpoint without -p" 's/rec "\$x-get-checkpoint" zpool get -H -p -o /rec "$x-get-checkpoint" zpool get -H -o /'

# The locale.
plant L1 sh "main: adapter_env not called" '/^    adapter_env$/d'
plant L2 sh "main: adapter_env called only after version, nocacheflush and locale" '/^    adapter_env$/d; s/^    rec locale locale$/    rec locale locale\n    adapter_env/'

# N2: scale's bounds and budget.
plant B1 sh "scale_all: budget default 48000 s, not 4800" 's/\${ZCR_SCALE_BUDGET:-4800}/${ZCR_SCALE_BUDGET:-48000}/'
plant B2 sh "scale: the zfs create loop without lim" 's/seq 1 "\$n" | lim "\$LIMIT" xargs/seq 1 "$n" | xargs/'
plant B3 sh "scale: zfs create without -u" 's/xargs -P 8 -I{} zfs create -u /xargs -P 8 -I{} zfs create /'
plant B4 sh "scale_all: four counts" 's/\[ "\$k" -ge 3 \]/[ "$k" -ge 4 ]/'
plant B5 sh "scale_all: the length bound dropped" '/^scale_all() {/,/^}/s/\[ "\${#n}" -gt 6 \] || \[ "\$n" -gt 100000 \]/[ "$n" -gt 100000 ]/'
plant B6 sh "scale_all: the bound 1000000" '/^scale_all() {/,/^}/s/-gt 100000 \]/-gt 1000000 ]/'
plant B7 sh "scale_all: a count twice run twice" 's/case \$seen in \*" \$n "\*)/case $seen in *" never "*)/'
plant B8 sh "scale_limit: nothing left is not refused" 's/^    \[ "\$left" -gt 0 \] || return 1$/    true/'
plant B9 sh "scale: the fallback snapshot dropped" '/rec "scale-\$n-snapshot-cli"/d'
plant B10 sh "scale: df dropped" '/df -h "\$WORK"/d'
plant B11 sh "scale: the start's budget check dropped" '/^scale() {/,/^}/{/^    if ! scale_limit; then$/,/^    fi$/d}'

# The writer beside the takes.
plant W1 sh "writing: the stall's window empty (t0 t0)" 's/stall "\$log.tsv" "\$t0" "\$t1"/stall "$log.tsv" "$t0" "$t0"/'
plant W3 sh "stop_bg: SIGKILL in place of SIGTERM (no end line)" 's/^    kill -TERM "\$1" 2> \/dev\/null$/    kill -KILL "$1" 2> \/dev\/null/'
plant W4 sh "writing: no margin before or after the take" '/^    sleep "\$margin"$/d'
plant W5 sh "writing: no 60 s for the writer to stop" 's/for ((i = 0; i < 600; i++)); do/for ((i = 0; i < 0; i++)); do/'
plant W6 sh "writing: about.txt line dropped" '/echo "scale: writer beside \$name: \$line"/d'
plant W7 sh "scale: no writer beside take-cli" 's/take-cli) writing "scale-\$n-take-cli" "\$wdir" "\$p" rec /take-cli) rec /'
plant W8 sh "writing: the writer's file not in DIR (in OUT)" 's/write "\$dir\/writer.dat"/write "$OUT\/writer.dat"/'
plant K1 sh "keep: secs dropped" '/^keep() {/,/^}/{/"\$secs" > "\$d\/secs"/d}'

# dsync_writer.py.
plant P1 py "stall: only writes that began in the window" 's/if s < t1 and s + lat > t0:/if t0 <= s < t1:/'
plant P2 py "write: O_DSYNC dropped" 's/flags=os.O_DSYNC):/flags=0):/'
plant P3 py "write: no handler for SIGTERM" 's/^        signal.signal(s, lambda \*_: stop.append(1))$/        pass/'
plant P4 py "write: slow is > SLOW, not >=" 's/if lat >= slow:/if lat > slow:/'
plant P5 py "write: the last second's line dropped" 's/^            out.write("sec\\t%d\\t%d\\t%.6f\\n" % (sec, count, longest))$/            pass/'
plant P6 py "stall: before from the seconds dropped" '97s/before = max(before, lat)/pass/'
plant P7 py "write: no end line" 's/^        out.write("end.*$/        pass/'
plant P8 py "write: monotonic clock, not the epoch" 's/time\.time()/time.monotonic()/g'
plant P9 py "write: no wrap at 64 MiB" 's/(n % span) \* block/n * block/'
plant P10 py "stall: any second that overlaps counts" 's/if t0 <= sec and sec + 1 <= t1:/if sec + 1 > t0 and sec < t1:/'
plant P11 py "stall: no start is not refused" 's/^    if not started:$/    if False:/'
plant P12 py "write: 8 KiB writes" 's/^BLOCK = 4096$/BLOCK = 8192/'

# New at 7a813d9: the writer of new data, the probe, ZCR_SCALE_LUA_MAX.
plant T1 sh "writing: the probe reads another kstat (txg, not txgs)" 's/\/\$pool\/txgs" "\$pool"/\/$pool\/txg" "$pool"/'
plant T2 sh "writing: the probe's window empty (t0 t0)" 's/report "\$log.txg.tsv" "\$t0" "\$t1"/report "$log.txg.tsv" "$t0" "$t0"/'
plant T3 sh "writing: the fill's window empty (t0 t0)" 's/stall "\$log.fill.tsv" "\$t0" "\$t1"/stall "$log.fill.tsv" "$t0" "$t0"/'
plant T4 sh "fill_span: the span is the limit, not twice it" 's/FILL_SPAN=\$((dirty \* 2))/FILL_SPAN=$dirty/'
plant T5 sh "fill_span: no 4 GiB cap" '/\[ "\$FILL_SPAN" -gt 4294967296 \] && FILL_SPAN=4294967296/d'
plant T6 sh "fill_span: the history not turned on" '/echo 100 > "\$params\/zfs_txg_history"/d'
plant T7 sh "scale: ZCR_SCALE_LUA_MAX compared with -ge" 's/\[ "\$n" -gt "\$ZCR_SCALE_LUA_MAX" \]/[ "$n" -ge "$ZCR_SCALE_LUA_MAX" ]/'
plant T8 sh "scale_all: ZCR_SCALE_LUA_MAX not checked" '/^scale_all() {/,/^}/{/^    case \$ZCR_SCALE_LUA_MAX in$/,/^    esac$/d}'
plant T9 sh "scale_all: a long ZCR_SCALE_LUA_MAX not capped" '/ZCR_SCALE_LUA_MAX=999999$/d'
plant T10 sh "stop_bg: does not wait for the process it stopped" '/^stop_bg() {/,/^}/{/^    wait "\$1"$/d}'
plant T11 sh "stop_bg: says stopped when it was not" '/^stop_bg() {/,/^}/s/^        return 1$/        return 0/'
plant T12 sh "writing: no writer of new data" 's/^        fpid=\$!$/        fpid=""/'
plant T13 sh "writing: about.txt's txg line dropped" '/echo "scale: txgs beside \$name: \$pline"/d'
plant T14 sh "writing: the probe not stopped (no line)" 's/^    if stop_bg "\$qpid"; then$/    if false; then/'
plant T15 sh "scale: the fill span not reset between counts (no local FILL_SPAN)" 's/^    local LIMIT FILL_SPAN=""$/    local LIMIT/'

plant F1 py "fill: with O_DSYNC" 's/max(1, int(argv\[3\]) \/\/ FILL_BLOCK), 0)/max(1, int(argv[3]) \/\/ FILL_BLOCK), os.O_DSYNC)/'
plant F2 py "fill: 4 KiB writes" 's/^FILL_BLOCK = 1024 \* 1024$/FILL_BLOCK = 4096/'
plant F3 py "fill: span in bytes taken as blocks" 's/max(1, int(argv\[3\]) \/\/ FILL_BLOCK)/max(1, int(argv[3]))/'

plant Q1 pq "probe: a group's end is its birth plus its sync alone" 's/end = (birth + o + q + w + s) \/ 1e9 + off/end = (birth + s) \/ 1e9 + off/'
plant Q2 pq "probe: CLOCK_MONOTONIC, not RAW" 's/time.clock_gettime_ns(time.CLOCK_MONOTONIC_RAW) \/ 1e9/time.clock_gettime_ns(time.CLOCK_MONOTONIC) \/ 1e9/'
plant Q3 pq "rows: wait and sync swapped" 's/int(f\[8\]), int(f\[9\]), int(f\[10\]), int(f\[11\])/int(f[8]), int(f[9]), int(f[11]), int(f[10])/'
plant Q4 pq "report: longest_sync only of groups that ended in the window" 's/if end - s < t1 and end > t0:/if t0 <= end <= t1:/'
plant Q5 pq "report: the start always an anchor" 's/if not points or points\[0\]\[0\] > start:/if True:/'
plant Q6 pq "report: a gap at an anchor not marked open" 's/gap, gap_open = b - a, ao or bo/gap, gap_open = b - a, False/'
plant Q7 pq "report: only zpool syncs that began in the window" 's/if s0 < t1 and s0 + secs > t0:/if t0 <= s0 < t1:/'
plant Q8 pq "probe: no zpool sync" 's/if sync is None and every and now - last_sync >= every:/if False:/'
plant Q9 pq "probe: an open sync not logged at the end" 's/^            out.write("open\\t%.6f\\t%.6f\\n" % (sync\[1\], time.time() - sync\[1\]))$/            pass/'
plant Q10 pq "probe: a group logged at every read" 's/if state == "C" and txg not in seen:/if state == "C":/'
plant Q11 pq "probe: groups not yet committed logged too" 's/if state == "C" and txg not in seen:/if txg not in seen:/'
plant Q12 pq "report: the stop not an anchor" 's/^    points.append((max(stop, points\[-1\]\[0\]), True))$/    pass/'
plant Q13 pq "report: txgs counts every group" 's/if t0 <= end <= t1:/if True:/'
plant Q14 pq "probe: no read after the stop" '117s/^        read()$/        pass/'
